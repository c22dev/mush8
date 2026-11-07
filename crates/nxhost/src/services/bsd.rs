// Mush8 - bsd.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Buffer, Request, Response, Service, Stub};
use std::sync::{Arc, Mutex};

use crate::services::vnet;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

#[derive(Clone)]
enum Fd {
    Host(i32),
    Virt(Arc<vnet::Socket>),
}

static FDS: Mutex<Vec<Option<Fd>>> = Mutex::new(Vec::new());

fn entry(g: i32) -> Option<Fd> {
    usize::try_from(g).ok().and_then(|i| FDS.lock().unwrap().get(i).cloned().flatten())
}

pub(super) fn host(g: i32) -> Option<i32> {
    match entry(g) {
        Some(Fd::Host(h)) => Some(h),
        _ => None,
    }
}

fn virt(g: i32) -> Option<Arc<vnet::Socket>> {
    match entry(g) {
        Some(Fd::Virt(s)) => Some(s),
        _ => None,
    }
}

fn adopt(f: Fd) -> i32 {
    let mut t = FDS.lock().unwrap();
    let i = t.iter().position(|e| e.is_none()).unwrap_or_else(|| {
        t.push(None);
        t.len() - 1
    });
    t[i] = Some(f);
    i as i32
}

fn release(g: i32) -> Option<Fd> {
    usize::try_from(g).ok().and_then(|i| FDS.lock().unwrap().get_mut(i).and_then(|e| e.take()))
}

const PARK_FOR: Duration = Duration::from_secs(5);
const MAX_PARKED: usize = 8;

static PARKED: Mutex<Vec<(u16, i32, Instant)>> = Mutex::new(Vec::new());

fn drop_expired(parked: &mut Vec<(u16, i32, Instant)>) {
    parked.retain(|&(_, fd, until)| {
        let keep = until > Instant::now();
        if !keep {
            unsafe { libc::close(fd) };
        }
        keep
    });
}

fn unconnected_udp_port(fd: i32) -> Option<u16> {
    let mut ty: i32 = 0;
    let mut len = 4 as libc::socklen_t;
    if unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, &mut ty as *mut _ as *mut _, &mut len) } != 0 || ty != libc::SOCK_DGRAM {
        return None;
    }
    let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    if unsafe { libc::getpeername(fd, &mut ss as *mut _ as *mut libc::sockaddr, &mut len) } == 0 {
        return None;
    }
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    if unsafe { libc::getsockname(fd, &mut ss as *mut _ as *mut libc::sockaddr, &mut len) } != 0 || ss.ss_family as i32 != libc::AF_INET {
        return None;
    }
    let port = u16::from_be(unsafe { (*(&ss as *const _ as *const libc::sockaddr_in)).sin_port });
    (port != 0).then_some(port)
}

fn park(fd: i32) -> bool {
    crate::services::nextendo::enabled() && keep(fd)
}

fn unpark(fd: i32, ss: &libc::sockaddr_storage) -> bool {
    crate::services::nextendo::enabled() && reclaim(fd, ss)
}

fn keep(fd: i32) -> bool {
    let Some(port) = unconnected_udp_port(fd) else { return false };
    let mut parked = PARKED.lock().unwrap();
    drop_expired(&mut parked);
    if let Some(i) = parked.iter().position(|p| p.0 == port) {
        unsafe { libc::close(parked.remove(i).1) };
    } else if parked.len() >= MAX_PARKED {
        return false;
    }
    parked.push((port, fd, Instant::now() + PARK_FOR));
    true
}

fn reclaim(fd: i32, ss: &libc::sockaddr_storage) -> bool {
    if ss.ss_family as i32 != libc::AF_INET {
        return false;
    }
    let port = u16::from_be(unsafe { (*(ss as *const _ as *const libc::sockaddr_in)).sin_port });
    let mut parked = PARKED.lock().unwrap();
    drop_expired(&mut parked);
    let Some(i) = parked.iter().position(|p| p.0 == port) else { return false };
    let (_, old, _) = parked.remove(i);
    let nonblock = unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK;
    let fl = unsafe { libc::fcntl(old, libc::F_GETFL) } & !libc::O_NONBLOCK;
    unsafe { libc::fcntl(old, libc::F_SETFL, fl | nonblock) };
    let ok = unsafe { libc::dup2(old, fd) } == fd;
    unsafe { libc::close(old) };
    if ok && trace() {
        eprintln!("[bsd] fd {fd}: reusing the udp/{port} socket the game just closed");
    }
    ok
}

fn fd_isset(set: &libc::fd_set, fd: i32) -> bool {
    let words = unsafe { &*(set as *const _ as *const [u32; 32]) };
    (0..1024).contains(&fd) && words[fd as usize / 32] & (1 << (fd % 32)) != 0
}

fn fd_set(set: &mut libc::fd_set, fd: i32) {
    let words = unsafe { &mut *(set as *mut _ as *mut [u32; 32]) };
    if (0..1024).contains(&fd) {
        words[fd as usize / 32] |= 1 << (fd % 32);
    }
}

fn addr_str(ss: &libc::sockaddr_storage) -> String {
    if ss.ss_family as i32 != libc::AF_INET {
        return format!("family {}", ss.ss_family);
    }
    let a = unsafe { &*(ss as *const _ as *const libc::sockaddr_in) };
    let ip = a.sin_addr.s_addr.to_ne_bytes();
    format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], u16::from_be(a.sin_port))
}

fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("NX_TRACE_BSD").is_some())
}

fn shout_map() -> &'static Mutex<std::collections::HashMap<i32, libc::sockaddr_storage>> {
    static SHOUT: std::sync::OnceLock<Mutex<std::collections::HashMap<i32, libc::sockaddr_storage>>> = std::sync::OnceLock::new();
    SHOUT.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn v4(ss: &libc::sockaddr_storage) -> Option<[u8; 4]> {
    if ss.ss_family as i32 != libc::AF_INET {
        return None;
    }
    let a = unsafe { &*(ss as *const _ as *const libc::sockaddr_in) };
    Some(a.sin_addr.s_addr.to_ne_bytes())
}

fn set_v4(ss: &mut libc::sockaddr_storage, ip: [u8; 4]) {
    let a = unsafe { &mut *(ss as *mut _ as *mut libc::sockaddr_in) };
    a.sin_addr.s_addr = u32::from_ne_bytes(ip);
}

fn ip_str(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

fn can_broadcast() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(multicast_ok)
}

#[cfg(not(target_os = "ios"))]
fn multicast_ok() -> bool {
    true
}

#[cfg(target_os = "ios")]
fn multicast_ok() -> bool {
    entitlement::present()
}

pub fn note_lan(addr: [u8; 4], mask: [u8; 4]) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mode = if can_broadcast() { "broadcast on" } else { "broadcast off" };
        eprintln!("[lan] {mode}, this device {}", ip_str(addr));
        crate::services::lan::start(addr, mask, can_broadcast());
    });
}

fn is_shout(ip: [u8; 4]) -> bool {
    let Some((addr, mask)) = crate::services::nifm::lan() else { return false };
    if ip == [255, 255, 255, 255] || (224..240).contains(&ip[0]) {
        return true;
    }
    let directed = std::array::from_fn(|i| addr[i] | !mask[i]);
    directed != addr && ip == directed
}

fn send_to(fd: i32, buf: &[u8], flags: i32, ss: &libc::sockaddr_storage, len: libc::socklen_t) -> (i32, u32) {
    res(unsafe { libc::sendto(fd, buf.as_ptr() as *const _, buf.len(), flags, ss as *const _ as *const _, len) })
}

fn deliver(fd: i32, buf: &[u8], flags: i32, ss: &libc::sockaddr_storage, len: libc::socklen_t) -> (i32, u32) {
    let Some(ip) = v4(ss).filter(|&ip| is_shout(ip)) else { return send_to(fd, buf, flags, ss, len) };
    let sent = can_broadcast() && send_to(fd, buf, flags, ss, len).0 >= 0;
    for peer in crate::services::lan::unicast_targets(sent) {
        let mut copy = *ss;
        set_v4(&mut copy, peer);
        send_to(fd, buf, flags, &copy, len);
    }
    if trace() {
        eprintln!("[bsd] broadcast fd {fd} to {}: {}", ip_str(ip), if sent { "sent" } else { "copies only" });
    }
    (buf.len() as i32, 0)
}

fn emulate_shout(ss: &libc::sockaddr_storage) -> bool {
    v4(ss).is_some_and(is_shout)
}

fn read_udp(fd: i32, buf: &mut [u8], flags: i32) -> (i32, u32, libc::sockaddr_storage, libc::socklen_t) {
    let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    loop {
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let n = unsafe { libc::recvfrom(fd, buf.as_mut_ptr() as *mut _, buf.len(), flags, &mut ss as *mut _ as *mut _, &mut len) };
        if n < 0 && spurious_udp_reset(fd) {
            let nonblocking = flags & libc::MSG_DONTWAIT != 0 || unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK != 0;
            if nonblocking {
                return (-1, linux_errno(libc::EAGAIN), ss, 0);
            }
            continue;
        }
        let r = res(n as isize);
        return (r.0, r.1, ss, if r.0 >= 0 { len } else { 0 });
    }
}

fn spurious_udp_reset(fd: i32) -> bool {
    let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if !matches!(e, libc::ECONNREFUSED | libc::ECONNRESET | libc::ENETRESET) || !crate::services::nextendo::enabled() {
        return false;
    }
    let mut ty: i32 = 0;
    let mut len = 4 as libc::socklen_t;
    unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, &mut ty as *mut _ as *mut _, &mut len) == 0 && ty == libc::SOCK_DGRAM }
}

const NX_O_NONBLOCK: i32 = 0x800;
const EBADF: u32 = 9;

pub struct Bsd(pub &'static str);

pub(super) fn in_buf(req: &Request, i: usize) -> Option<Buffer> {
    req.a.get(i).filter(|b| b.addr != 0 && b.size != 0).or(req.x.get(i).filter(|b| b.addr != 0)).copied()
}

pub(super) fn out_buf(req: &Request, i: usize) -> Option<Buffer> {
    req.b.get(i).filter(|b| b.addr != 0 && b.size != 0).or(req.c.get(i).filter(|b| b.addr != 0)).copied()
}

pub(super) fn bytes<'a>(b: Option<Buffer>) -> &'a [u8] {
    match b {
        Some(b) if b.addr != 0 => unsafe { std::slice::from_raw_parts(b.addr as *const u8, b.size as usize) },
        _ => &[],
    }
}

pub(super) fn bytes_mut<'a>(b: Option<Buffer>) -> &'a mut [u8] {
    match b {
        Some(b) if b.addr != 0 => unsafe { std::slice::from_raw_parts_mut(b.addr as *mut u8, b.size as usize) },
        _ => &mut [],
    }
}

fn sockaddr_in(b: &[u8]) -> (libc::sockaddr_storage, libc::socklen_t) {
    let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let n = b.len().min(std::mem::size_of::<libc::sockaddr_storage>());
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), &mut ss as *mut _ as *mut u8, n) };
    let len = if ss.ss_family as i32 == libc::AF_INET { std::mem::size_of::<libc::sockaddr_in>() } else { n };
    ss.ss_len = len as u8;
    (ss, len as libc::socklen_t)
}

fn put_sockaddr(dst: &mut [u8], ss: &libc::sockaddr_storage, len: libc::socklen_t) {
    let n = dst.len().min(len as usize);
    unsafe { std::ptr::copy_nonoverlapping(ss as *const _ as *const u8, dst.as_mut_ptr(), n) };
    if let Some(b) = dst.first_mut() {
        *b = 0;
    }
}

fn errno() -> u32 {
    linux_errno(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
}

fn linux_errno(e: i32) -> u32 {
    (match e {
        libc::EAGAIN => 11,
        libc::EDEADLK => 35,
        libc::ENOTSUP => 95,
        libc::EINPROGRESS => 115,
        libc::EALREADY => 114,
        libc::ENOTSOCK => 88,
        libc::EDESTADDRREQ => 89,
        libc::EMSGSIZE => 90,
        libc::EPROTOTYPE => 91,
        libc::ENOPROTOOPT => 92,
        libc::EPROTONOSUPPORT => 93,
        libc::ESOCKTNOSUPPORT => 94,
        libc::EOPNOTSUPP => 95,
        libc::EPFNOSUPPORT => 96,
        libc::EAFNOSUPPORT => 97,
        libc::EADDRINUSE => 98,
        libc::EADDRNOTAVAIL => 99,
        libc::ENETDOWN => 100,
        libc::ENETUNREACH => 101,
        libc::ENETRESET => 102,
        libc::ECONNABORTED => 103,
        libc::ECONNRESET => 104,
        libc::ENOBUFS => 105,
        libc::EISCONN => 106,
        libc::ENOTCONN => 107,
        libc::ESHUTDOWN => 108,
        libc::ETOOMANYREFS => 109,
        libc::ETIMEDOUT => 110,
        libc::ECONNREFUSED => 111,
        libc::ELOOP => 40,
        libc::ENAMETOOLONG => 36,
        libc::EHOSTDOWN => 112,
        libc::EHOSTUNREACH => 113,
        e => e,
    }) as u32
}

fn res(r: isize) -> (i32, u32) {
    if r < 0 { (-1, errno()) } else { (r as i32, 0) }
}

impl Service for Bsd {
    fn name(&self) -> String {
        self.0.into()
    }

    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        let gfd = req.u32(0) as i32;
        if matches!(req.cmd, 8..=22 | 24..=27) {
            if let Some(s) = virt(gfd) {
                return virt_call(&s, gfd, req, rsp);
            }
        }
        if (req.cmd == 5 || req.cmd == 6) && vnet::mode().is_virtual() {
            return virt_wait(req, rsp);
        }
        let fd = host(gfd).unwrap_or(-1);
        let reply = |rsp: &mut Response, (ret, err): (i32, u32)| {
            rsp.push_u32(ret as u32);
            rsp.push_u32(err);
        };
        if matches!(req.cmd, 8..=22 | 24..=27) && fd < 0 {
            reply(rsp, (-1, EBADF));
            if req.cmd == 9 || req.cmd == 12 || req.cmd == 15 || req.cmd == 16 || req.cmd == 17 {
                rsp.push_u32(0);
            }
            return 0;
        }
        let r: (i32, u32) = match req.cmd {
            0 => {
                rsp.push_u32(0);
                return 0;
            }
            1 => return 0,
            2 | 3 => {
                let (domain, ty, proto) = (req.u32(0) as i32, req.u32(4) as i32, req.u32(8) as i32);
                if vnet::mode().is_virtual() && domain == libc::AF_INET && ty & 0xF == libc::SOCK_DGRAM {
                    let r = (adopt(Fd::Virt(vnet::Socket::new())), 0);
                    if trace() {
                        eprintln!("[bsd] socket {} is virtual", r.0);
                    }
                    reply(rsp, r);
                    return 0;
                }
                let s = unsafe { libc::socket(domain, ty & 0xF, proto) };
                if s >= 0 {
                    let one: i32 = 1;
                    let set = |opt| unsafe { libc::setsockopt(s, libc::SOL_SOCKET, opt, &one as *const _ as *const _, 4) };
                    set(libc::SO_NOSIGPIPE);
                    if ty & 0xF == libc::SOCK_DGRAM {
                        set(libc::SO_REUSEPORT);
                    }
                    (adopt(Fd::Host(s)), 0)
                } else {
                    res(-1)
                }
            }
            5 => {
                let gnfds = (req.u32(0) as i32).min(1024);
                let (sec, usec, null) = (req.u64(8) as i64, req.u64(16) as i64, req.data.get(24).copied().unwrap_or(1) != 0);
                let mut gsets: [libc::fd_set; 3] = unsafe { std::mem::zeroed() };
                let mut sets: [libc::fd_set; 3] = unsafe { std::mem::zeroed() };
                let mut nfds = 0;
                for i in 0..3 {
                    let src = bytes(in_buf(req, i));
                    let n = src.len().min(std::mem::size_of::<libc::fd_set>());
                    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), &mut gsets[i] as *mut _ as *mut u8, n) };
                    for g in 0..gnfds {
                        if let Some(h) = host(g).filter(|_| fd_isset(&gsets[i], g)) {
                            fd_set(&mut sets[i], h);
                            nfds = nfds.max(h + 1);
                        }
                    }
                }
                let mut tv = libc::timeval { tv_sec: sec, tv_usec: usec as i32 };
                let tvp = if null { std::ptr::null_mut() } else { &mut tv as *mut _ };
                let [r, w, e] = &mut sets;
                let rv = crate::kernel::blocking(|| unsafe { libc::select(nfds, r, w, e, tvp) });
                let out = res(rv as isize);
                for i in 0..3 {
                    let mut out: libc::fd_set = unsafe { std::mem::zeroed() };
                    for g in 0..gnfds {
                        if host(g).is_some_and(|h| fd_isset(&gsets[i], g) && fd_isset(&sets[i], h)) {
                            fd_set(&mut out, g);
                        }
                    }
                    let dst = bytes_mut(out_buf(req, i));
                    let n = dst.len().min(std::mem::size_of::<libc::fd_set>());
                    unsafe { std::ptr::copy_nonoverlapping(&out as *const _ as *const u8, dst.as_mut_ptr(), n) };
                }
                out
            }
            6 => {
                let (nfds, timeout) = (req.u32(0) as usize, req.u32(4) as i32);
                let src = bytes(in_buf(req, 0));
                let mut fds = vec![libc::pollfd { fd: -1, events: 0, revents: 0 }; nfds.min(src.len() / 8)];
                unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), fds.as_mut_ptr() as *mut u8, fds.len() * 8) };
                let guest: Vec<i32> = fds.iter().map(|p| p.fd).collect();
                for p in fds.iter_mut() {
                    p.fd = host(p.fd).unwrap_or(-1);
                }
                let rv = crate::kernel::blocking(|| unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) });
                for (p, g) in fds.iter_mut().zip(&guest) {
                    p.fd = *g;
                }
                let dst = bytes_mut(out_buf(req, 0));
                let n = dst.len().min(fds.len() * 8);
                unsafe { std::ptr::copy_nonoverlapping(fds.as_ptr() as *const u8, dst.as_mut_ptr(), n) };
                res(rv as isize)
            }
            8 | 25 => {
                let flags = if req.cmd == 8 { req.u32(4) as i32 } else { 0 };
                let b = bytes_mut(out_buf(req, 0));
                let (n, err, _, _) = crate::kernel::blocking(|| read_udp(fd, b, flags));
                (n, err)
            }
            9 => {
                let flags = req.u32(4) as i32;
                let b = bytes_mut(out_buf(req, 0));
                let (n, err, ss, len) = crate::kernel::blocking(|| read_udp(fd, b, flags));
                put_sockaddr(bytes_mut(out_buf(req, 1)), &ss, len);
                if trace() && n >= 0 {
                    eprintln!("[bsd] recvfrom fd {fd}: {n} bytes from {}", addr_str(&ss));
                }
                reply(rsp, (n, err));
                rsp.push_u32(if n >= 0 { len } else { 0 });
                return 0;
            }
            10 | 24 => {
                let flags = if req.cmd == 10 { req.u32(4) as i32 } else { 0 };
                let b = bytes(in_buf(req, 0));
                let shout = shout_map().lock().unwrap().get(&fd).copied();
                if let Some(ss) = shout {
                    let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
                    crate::kernel::blocking(|| deliver(fd, b, flags, &ss, len))
                } else {
                    res(crate::kernel::blocking(|| unsafe { libc::send(fd, b.as_ptr() as *const _, b.len(), flags) }))
                }
            }
            11 => {
                let flags = req.u32(4) as i32;
                let b = bytes(in_buf(req, 0));
                let (ss, len) = sockaddr_in(bytes(in_buf(req, 1)));
                if trace() {
                    let hex: String = b.iter().take(96).map(|x| format!("{x:02x}")).collect();
                    eprintln!("[bsd] sendto fd {fd}: {} bytes to {}: {hex}", b.len(), addr_str(&ss));
                }
                crate::kernel::blocking(|| deliver(fd, b, flags, &ss, len))
            }
            12 | 15 | 16 => {
                if req.cmd == 15 {
                    if let Some(ss) = shout_map().lock().unwrap().get(&fd).copied() {
                        let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
                        put_sockaddr(bytes_mut(out_buf(req, 0)), &ss, len);
                        reply(rsp, (0, 0));
                        rsp.push_u32(len);
                        return 0;
                    }
                }
                let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                let p = &mut ss as *mut _ as *mut libc::sockaddr;
                let r = res(match req.cmd {
                    12 => {
                        let s = crate::kernel::blocking(|| unsafe { libc::accept(fd, p, &mut len) });
                        if s >= 0 { adopt(Fd::Host(s)) as isize } else { -1 }
                    }
                    15 => unsafe { libc::getpeername(fd, p, &mut len) as isize },
                    _ => unsafe { libc::getsockname(fd, p, &mut len) as isize },
                });
                put_sockaddr(bytes_mut(out_buf(req, 0)), &ss, len);
                reply(rsp, r);
                rsp.push_u32(len);
                return 0;
            }
            13 | 14 => {
                let (ss, len) = sockaddr_in(bytes(in_buf(req, 0)));
                if trace() {
                    eprintln!("[bsd] {} fd {fd}: {}", if req.cmd == 13 { "bind" } else { "connect" }, addr_str(&ss));
                }
                let p = &ss as *const _ as *const libc::sockaddr;
                if req.cmd == 14 && emulate_shout(&ss) {
                    shout_map().lock().unwrap().insert(fd, ss);
                    (0, 0)
                } else {
                    if req.cmd == 14 {
                        shout_map().lock().unwrap().remove(&fd);
                    }
                    res(if req.cmd == 13 {
                        if unpark(fd, &ss) { 0 } else { unsafe { libc::bind(fd, p, len) as isize } }
                    } else {
                        crate::kernel::blocking(|| unsafe { libc::connect(fd, p, len) as isize })
                    })
                }
            }
            17 => {
                let b = bytes_mut(out_buf(req, 0));
                let mut len = b.len() as libc::socklen_t;
                let r = res(unsafe { libc::getsockopt(fd, req.u32(4) as i32, req.u32(8) as i32, b.as_mut_ptr() as *mut _, &mut len) } as isize);
                reply(rsp, r);
                rsp.push_u32(len);
                return 0;
            }
            18 => res(unsafe { libc::listen(fd, req.u32(4) as i32) } as isize),
            19 => {
                let mut v: u32 = 0;
                let src = bytes(in_buf(req, 0));
                if src.len() >= 4 {
                    v = u32::from_le_bytes(src[..4].try_into().unwrap());
                }
                let request = req.u32(4) as libc::c_ulong;
                if request != libc::FIONREAD && request != libc::FIONBIO {
                    (-1, 22)
                } else {
                    let r = res(unsafe { libc::ioctl(fd, request, &mut v) } as isize);
                    let dst = bytes_mut(out_buf(req, 0));
                    if dst.len() >= 4 {
                        dst[..4].copy_from_slice(&v.to_le_bytes());
                    }
                    r
                }
            }
            20 => {
                let (cmd, arg) = (req.u32(4) as i32, req.u32(8) as i32);
                match cmd {
                    libc::F_GETFL => {
                        let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                        if fl < 0 { res(-1) } else { (if fl & libc::O_NONBLOCK != 0 { NX_O_NONBLOCK } else { 0 }, 0) }
                    }
                    libc::F_SETFL => {
                        let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) } & !libc::O_NONBLOCK;
                        let nb = if arg & NX_O_NONBLOCK != 0 { libc::O_NONBLOCK } else { 0 };
                        res(unsafe { libc::fcntl(fd, libc::F_SETFL, fl | nb) } as isize)
                    }
                    _ => (0, 0),
                }
            }
            21 => {
                let b = bytes(in_buf(req, 0));
                let r = res(unsafe { libc::setsockopt(fd, req.u32(4) as i32, req.u32(8) as i32, b.as_ptr() as *const _, b.len() as _) } as isize);
                if r.0 < 0 && r.1 == linux_errno(libc::ENOPROTOOPT) { (0, 0) } else { r }
            }
            22 => res(unsafe { libc::shutdown(fd, req.u32(4) as i32) } as isize),
            23 => {
                let all: Vec<Fd> = FDS.lock().unwrap().iter().flatten().cloned().collect();
                for f in all {
                    match f {
                        Fd::Host(s) => unsafe { libc::shutdown(s, req.u32(0) as i32) },
                        Fd::Virt(s) => {
                            s.shutdown();
                            0
                        }
                    };
                }
                (0, 0)
            }
            26 => {
                release(gfd);
                shout_map().lock().unwrap().remove(&fd);
                if park(fd) { (0, 0) } else { res(unsafe { libc::close(fd) } as isize) }
            }
            27 => {
                let d = unsafe { libc::dup(fd) };
                if d >= 0 {
                    let mut shout = shout_map().lock().unwrap();
                    if let Some(ss) = shout.get(&fd).copied() {
                        shout.insert(d, ss);
                    }
                    drop(shout);
                    (adopt(Fd::Host(d)), 0)
                } else {
                    res(-1)
                }
            }
            _ => return Stub(self.name()).call(req, rsp),
        };
        if trace() {
            eprintln!("[bsd] cmd {} fd {gfd} -> {} errno {}", req.cmd, r.0, r.1);
        }
        reply(rsp, r);
        0
    }
}

fn guest_v4(b: &[u8]) -> Option<SocketAddrV4> {
    (b.len() >= 8 && i32::from(b[1]) == libc::AF_INET).then(|| SocketAddrV4::new(Ipv4Addr::new(b[4], b[5], b[6], b[7]), u16::from_be_bytes([b[2], b[3]])))
}

fn put_v4(dst: &mut [u8], a: SocketAddrV4) -> u32 {
    let mut b = [0u8; 16];
    b[1] = libc::AF_INET as u8;
    b[2..4].copy_from_slice(&a.port().to_be_bytes());
    b[4..8].copy_from_slice(&a.ip().octets());
    let n = dst.len().min(b.len());
    dst[..n].copy_from_slice(&b[..n]);
    b.len() as u32
}

const NX_SOL_SOCKET: u32 = 0xFFFF;
const NX_MSG_PEEK: i32 = 2;
const NX_MSG_DONTWAIT: i32 = 0x80;

fn virt_call(s: &Arc<vnet::Socket>, gfd: i32, req: &Request, rsp: &mut Response) -> u32 {
    let flags = if matches!(req.cmd, 8 | 9) { req.u32(4) as i32 } else { 0 };
    let mut out_len = 0;
    let r: Result<i32, u32> = match req.cmd {
        8 | 9 | 25 => s.recv_from(bytes_mut(out_buf(req, 0)), flags & NX_MSG_PEEK != 0, flags & NX_MSG_DONTWAIT != 0).map(|(n, from)| {
            if req.cmd == 9 {
                out_len = put_v4(bytes_mut(out_buf(req, 1)), from);
            }
            n as i32
        }),
        10 | 24 => s.send_to(bytes(in_buf(req, 0)), None).map(|n| n as i32),
        11 => s.send_to(bytes(in_buf(req, 0)), guest_v4(bytes(in_buf(req, 1)))).map(|n| n as i32),
        12 | 18 => Err(linux_errno(libc::EOPNOTSUPP)),
        13 => guest_v4(bytes(in_buf(req, 0))).ok_or(linux_errno(libc::EAFNOSUPPORT)).and_then(|a| s.bind(a)).map(|_| 0),
        14 => s.connect(guest_v4(bytes(in_buf(req, 0)))).map(|_| 0),
        15 => s.peer().map(|a| {
            out_len = put_v4(bytes_mut(out_buf(req, 0)), a);
            0
        }),
        16 => {
            out_len = put_v4(bytes_mut(out_buf(req, 0)), s.local());
            Ok(0)
        }
        17 => {
            let v: i32 = match (req.u32(4), req.u32(8) as i32) {
                (NX_SOL_SOCKET, libc::SO_TYPE) => libc::SOCK_DGRAM,
                (NX_SOL_SOCKET, libc::SO_RCVBUF | libc::SO_SNDBUF) => 0x10000,
                _ => 0,
            };
            let dst = bytes_mut(out_buf(req, 0));
            let n = dst.len().min(4);
            dst[..n].copy_from_slice(&v.to_le_bytes()[..n]);
            out_len = 4;
            Ok(0)
        }
        19 => {
            match req.u32(4) as libc::c_ulong {
                libc::FIONREAD => {
                    let dst = bytes_mut(out_buf(req, 0));
                    if dst.len() >= 4 {
                        dst[..4].copy_from_slice(&(s.pending() as u32).to_le_bytes());
                    }
                }
                libc::FIONBIO => s.set_nonblocking(bytes(in_buf(req, 0)).first_chunk::<4>().is_some_and(|v| u32::from_le_bytes(*v) != 0)),
                _ => {}
            }
            Ok(0)
        }
        20 => match req.u32(4) as i32 {
            libc::F_GETFL => Ok(if s.nonblocking() { NX_O_NONBLOCK } else { 0 }),
            libc::F_SETFL => {
                s.set_nonblocking(req.u32(8) as i32 & NX_O_NONBLOCK != 0);
                Ok(0)
            }
            _ => Ok(0),
        },
        21 => {
            let b = bytes(in_buf(req, 0));
            if req.u32(4) == NX_SOL_SOCKET && req.u32(8) as i32 == libc::SO_RCVTIMEO && b.len() >= 16 {
                let (sec, usec) = (u64::from_le_bytes(b[..8].try_into().unwrap()), u64::from_le_bytes(b[8..16].try_into().unwrap()));
                let t = Duration::from_secs(sec) + Duration::from_micros(usec);
                s.set_timeout((!t.is_zero()).then_some(t));
            }
            Ok(0)
        }
        22 => {
            if req.u32(4) as i32 != libc::SHUT_WR {
                s.shutdown();
            }
            Ok(0)
        }
        26 => {
            release(gfd);
            s.close();
            Ok(0)
        }
        _ => {
            s.dup();
            Ok(adopt(Fd::Virt(s.clone())))
        }
    };
    let (ret, err) = match r {
        Ok(n) => (n, 0),
        Err(e) => (-1, e),
    };
    if trace() {
        match req.cmd {
            9 if ret >= 0 => eprintln!("[bsd] vrecvfrom fd {gfd}: {ret} bytes"),
            11 => eprintln!("[bsd] vsendto fd {gfd}: {} bytes to {:?} -> {ret} errno {err}", bytes(in_buf(req, 0)).len(), guest_v4(bytes(in_buf(req, 1)))),
            13 | 14 => eprintln!("[bsd] v{} fd {gfd}: {:?} -> {ret} errno {err}", if req.cmd == 13 { "bind" } else { "connect" }, guest_v4(bytes(in_buf(req, 0)))),
            21 => eprintln!("[bsd] vsetsockopt fd {gfd}: level {:#x} opt {:#x}", req.u32(4), req.u32(8)),
            9 | 8 | 25 if err == 11 => {}
            _ => eprintln!("[bsd] vcmd {} fd {gfd} -> {ret} errno {err}", req.cmd),
        }
    }
    rsp.push_u32(ret as u32);
    rsp.push_u32(err);
    if matches!(req.cmd, 9 | 15 | 16 | 17) {
        rsp.push_u32(if ret >= 0 { out_len } else { 0 });
    }
    0
}

fn ready_now(wants: &[(i32, i16)]) -> Vec<i16> {
    let mut hosts: Vec<libc::pollfd> = wants.iter().map(|&(g, events)| libc::pollfd { fd: host(g).unwrap_or(-1), events, revents: 0 }).collect();
    if hosts.iter().any(|p| p.fd >= 0) {
        unsafe { libc::poll(hosts.as_mut_ptr(), hosts.len() as _, 0) };
    }
    wants.iter().zip(&hosts).map(|(&(g, events), p)| match entry(g) {
        Some(Fd::Virt(s)) => s.poll(events),
        Some(Fd::Host(_)) => p.revents,
        None => vnet::POLLNVAL,
    }).collect()
}

fn virt_wait(req: &Request, rsp: &mut Response) -> u32 {
    let mut pollfds = Vec::new();
    let mut gsets: [libc::fd_set; 3] = unsafe { std::mem::zeroed() };
    let (wants, timeout): (Vec<(i32, i16)>, Option<Duration>) = if req.cmd == 6 {
        let (nfds, ms) = (req.u32(0) as usize, req.u32(4) as i32);
        let src = bytes(in_buf(req, 0));
        pollfds = vec![libc::pollfd { fd: -1, events: 0, revents: 0 }; nfds.min(src.len() / 8)];
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), pollfds.as_mut_ptr() as *mut u8, pollfds.len() * 8) };
        (pollfds.iter().map(|p| (p.fd, p.events)).collect(), (ms >= 0).then(|| Duration::from_millis(ms as u64)))
    } else {
        let gnfds = (req.u32(0) as i32).min(1024);
        let (sec, usec, null) = (req.u64(8), req.u64(16), req.data.get(24).copied().unwrap_or(1) != 0);
        for (i, set) in gsets.iter_mut().enumerate() {
            let src = bytes(in_buf(req, i));
            let n = src.len().min(std::mem::size_of::<libc::fd_set>());
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), set as *mut _ as *mut u8, n) };
        }
        let wants = (0..gnfds)
            .map(|g| (g, [vnet::POLLIN, vnet::POLLOUT, libc::POLLPRI].iter().zip(&gsets).filter(|(_, set)| fd_isset(set, g)).fold(0, |m, (e, _)| m | e)))
            .filter(|&(_, e)| e != 0)
            .collect();
        (wants, (!null).then(|| Duration::from_secs(sec) + Duration::from_micros(usec)))
    };
    let deadline = timeout.map(|t| Instant::now() + t);
    let has_host = wants.iter().any(|&(g, _)| host(g).is_some());
    let revents = loop {
        let seen = vnet::epoch();
        let r = ready_now(&wants);
        if r.iter().any(|&e| e != 0) || deadline.is_some_and(|d| Instant::now() >= d) {
            break r;
        }
        let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        let step = if has_host { Some(left.map_or(Duration::from_millis(10), |l| l.min(Duration::from_millis(10)))) } else { left };
        crate::kernel::blocking(|| vnet::wait_change(seen, step));
    };
    let mut count = 0;
    if req.cmd == 6 {
        for (p, r) in pollfds.iter_mut().zip(&revents) {
            p.revents = *r;
            count += i32::from(*r != 0);
        }
        let dst = bytes_mut(out_buf(req, 0));
        let n = dst.len().min(pollfds.len() * 8);
        unsafe { std::ptr::copy_nonoverlapping(pollfds.as_ptr() as *const u8, dst.as_mut_ptr(), n) };
    } else {
        let mut out: [libc::fd_set; 3] = unsafe { std::mem::zeroed() };
        let readable = vnet::POLLIN | libc::POLLERR | libc::POLLHUP | vnet::POLLNVAL;
        for (&(g, _), &r) in wants.iter().zip(&revents) {
            for (i, bits) in [readable, vnet::POLLOUT, libc::POLLPRI].into_iter().enumerate() {
                if r & bits != 0 && fd_isset(&gsets[i], g) {
                    fd_set(&mut out[i], g);
                    count += 1;
                }
            }
        }
        for (i, set) in out.iter().enumerate() {
            let dst = bytes_mut(out_buf(req, i));
            let n = dst.len().min(std::mem::size_of::<libc::fd_set>());
            unsafe { std::ptr::copy_nonoverlapping(set as *const _ as *const u8, dst.as_mut_ptr(), n) };
        }
    }
    rsp.push_u32(count as u32);
    rsp.push_u32(0);
    0
}

pub fn open(name: &'static str) -> Arc<dyn Service> {
    Arc::new(Bsd(name))
}

#[cfg(target_os = "ios")]
mod entitlement {
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecTaskCreateFromSelf(allocator: *const core::ffi::c_void) -> *mut core::ffi::c_void;
        fn SecTaskCopyValueForEntitlement(task: *mut core::ffi::c_void, entitlement: *const core::ffi::c_void, error: *mut *mut core::ffi::c_void) -> *const core::ffi::c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(alloc: *const core::ffi::c_void, cstr: *const i8, encoding: u32) -> *const core::ffi::c_void;
        fn CFRelease(cf: *const core::ffi::c_void);
        fn CFGetTypeID(cf: *const core::ffi::c_void) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(boolean: *const core::ffi::c_void) -> u8;
    }

    const UTF8: u32 = 0x0800_0100;

    pub fn present() -> bool {
        unsafe {
            let task = SecTaskCreateFromSelf(core::ptr::null());
            if task.is_null() {
                return false;
            }
            let name = std::ffi::CString::new("com.apple.developer.networking.multicast").unwrap();
            let key = CFStringCreateWithCString(core::ptr::null(), name.as_ptr(), UTF8);
            let value = if key.is_null() { core::ptr::null() } else { SecTaskCopyValueForEntitlement(task, key, core::ptr::null_mut()) };
            let ok = !value.is_null() && CFGetTypeID(value) == CFBooleanGetTypeID() && CFBooleanGetValue(value) != 0;
            if !value.is_null() {
                CFRelease(value);
            }
            if !key.is_null() {
                CFRelease(key);
            }
            CFRelease(task);
            ok
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn udp(port: u16) -> (i32, libc::sockaddr_storage) {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let sin = unsafe { &mut *(&mut ss as *mut _ as *mut libc::sockaddr_in) };
        sin.sin_len = 16;
        sin.sin_family = libc::AF_INET as u8;
        sin.sin_port = port.to_be();
        sin.sin_addr.s_addr = u32::from(Ipv4Addr::LOCALHOST).to_be();
        (fd, ss)
    }

    #[test]
    fn a_rebound_udp_port_keeps_its_socket() {
        let (first, ss) = udp(0);
        assert_eq!(unsafe { libc::bind(first, &ss as *const _ as *const libc::sockaddr, 16) }, 0);
        let port = unconnected_udp_port(first).unwrap();
        assert!(keep(first));
        let (second, ss) = udp(port);
        assert!(reclaim(second, &ss));
        assert_eq!(unconnected_udp_port(second), Some(port));
        let (peer, _) = udp(0);
        let sent = unsafe { libc::sendto(peer, b"hi".as_ptr().cast(), 2, 0, &ss as *const _ as *const libc::sockaddr, 16) };
        assert_eq!(sent, 2);
        let mut buf = [0u8; 4];
        let got = unsafe { libc::recv(second, buf.as_mut_ptr().cast(), 4, 0) };
        assert_eq!(&buf[..got as usize], b"hi");
        assert!(!reclaim(second, &ss));
        unsafe {
            libc::close(second);
            libc::close(peer);
        }
    }
}
