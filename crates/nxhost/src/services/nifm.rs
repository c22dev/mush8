// Mush8 - nifm.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Act, Request, Response, Service, Stub, Table, new_event, write_buffer};
use crate::services::vnet::{self, Mode};
use std::sync::Arc;

const NO_INTERNET_CONNECTION: u32 = (300 << 9) | 110;
const REQUEST_STATE_ERROR: u32 = 1;
const REQUEST_STATE_ACCEPTED: u32 = 3;

pub fn lan() -> Option<([u8; 4], [u8; 4])> {
    match vnet::mode() {
        Mode::Off => None,
        Mode::Lan | Mode::Nextendo => host_lan(),
        _ => vnet::network().map(|(addr, mask, _)| (addr, mask)),
    }
}

fn host_lan() -> Option<([u8; 4], [u8; 4])> {
    static LAN: std::sync::OnceLock<Option<([u8; 4], [u8; 4])>> = std::sync::OnceLock::new();
    let found = *LAN.get_or_init(|| {
        let want = std::env::var("NX_LAN_IF").ok();
        let mut found = Vec::new();
        unsafe {
            let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
            if libc::getifaddrs(&mut ifa) != 0 {
                return None;
            }
            let mut p = ifa;
            while !p.is_null() {
                let i = &*p;
                let up = i.ifa_flags & (libc::IFF_UP | libc::IFF_RUNNING) as u32 == (libc::IFF_UP | libc::IFF_RUNNING) as u32;
                let lo = i.ifa_flags & libc::IFF_LOOPBACK as u32 != 0;
                if up && !lo && !i.ifa_addr.is_null() && (*i.ifa_addr).sa_family as i32 == libc::AF_INET && !i.ifa_netmask.is_null() {
                    let name = std::ffi::CStr::from_ptr(i.ifa_name).to_string_lossy().into_owned();
                    let a = (*(i.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr.to_ne_bytes();
                    let m = (*(i.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr.to_ne_bytes();
                    found.push((name, a, m));
                }
                p = i.ifa_next;
            }
            libc::freeifaddrs(ifa);
        }
        let pick = match &want {
            Some(w) => found.iter().find(|(n, _, _)| n == w),
            None => found.iter().find(|(n, _, _)| n == "en0").or(found.first()),
        };
        match pick {
            Some((n, a, m)) => {
                eprintln!("[nifm] LAN play on {n}: {}.{}.{}.{}", a[0], a[1], a[2], a[3]);
                Some((*a, *m))
            }
            None => {
                eprintln!("[nifm] LAN play: no active IPv4 interface{}", want.map(|w| format!(" named {w}")).unwrap_or_default());
                None
            }
        }
    });
    if let Some((addr, mask)) = found {
        crate::services::bsd::note_lan(addr, mask);
    }
    found
}

pub fn open() -> Arc<dyn Service> {
    Arc::new(Table { name: "nifm", cmds: &[(4, Act::Obj(general_service)), (5, Act::Obj(general_service))] })
}

fn general_service() -> Arc<dyn Service> {
    Arc::new(GeneralService)
}

struct GeneralService;

impl Service for GeneralService {
    fn name(&self) -> String {
        "nifm::IGeneralService".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        let Some((addr, mask)) = lan() else {
            return match req.cmd {
                1 => 0,
                4 => {
                    rsp.objects.push(Arc::new(RequestService));
                    0
                }
                12 | 15 | 18 => NO_INTERNET_CONNECTION,
                21 => {
                    rsp.push_u32(0);
                    0
                }
                _ => Stub(self.name()).call(req, rsp),
            };
        };
        let net: [u8; 4] = std::array::from_fn(|i| addr[i] & mask[i]);
        let gateway = vnet::network().map_or([net[0], net[1], net[2], net[3] | 1], |n| n.2);
        match req.cmd {
            1 => {}
            4 => rsp.objects.push(Arc::new(RequestService)),
            12 => rsp.data.extend_from_slice(&addr),
            15 => {
                rsp.data.push(1);
                rsp.data.extend_from_slice(&addr);
                rsp.data.extend_from_slice(&mask);
                rsp.data.extend_from_slice(&gateway);
                rsp.data.push(1);
                rsp.data.extend_from_slice(&gateway);
                rsp.data.extend_from_slice(&[0; 4]);
            }
            5 => {
                let mut p = [0u8; 0x17C];
                p[0] = 1;
                p[1..5].copy_from_slice(&addr);
                p[5..9].copy_from_slice(&mask);
                p[9..13].copy_from_slice(&gateway);
                p[0x0D] = 1;
                p[0x0E..0x12].copy_from_slice(&gateway);
                p[0xC0..0xC2].copy_from_slice(&1500u16.to_le_bytes());
                p[0xC2..0xD2].copy_from_slice(b"Mush8 network\0\0\0");
                p[0xD2..0xD7].copy_from_slice(b"Mush8");
                p[0x113] = 2;
                p[0x114] = 1;
                if let Some(b) = req.out_buffer() {
                    write_buffer(&b, &p);
                }
            }
            18 => rsp.data.extend_from_slice(&[2, 3, 4]),
            20..=22 => rsp.push_u32(1),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct RequestService;

impl Service for RequestService {
    fn name(&self) -> String {
        "nifm::IRequest".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        let online = lan().is_some();
        match req.cmd {
            0 => rsp.push_u32(if online { REQUEST_STATE_ACCEPTED } else { REQUEST_STATE_ERROR }),
            1 => return if online { 0 } else { NO_INTERNET_CONNECTION },
            2 => rsp.copy_handles.extend([new_event(online), new_event(false)]),
            3 | 4 | 6 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 => {}
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}
