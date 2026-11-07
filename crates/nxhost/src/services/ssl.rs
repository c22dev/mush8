// Mush8 - ssl.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Request, Response, Service, Stub, write_buffer};
use crate::services::bsd::{self, bytes, bytes_mut, in_buf, out_buf};
use crate::services::nextendo;
use crate::services::nextrace::{self, Trace};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};
use std::io::{ErrorKind, Read, Write};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, OnceLock};

const fn rc(desc: u32) -> u32 {
    123 | desc << 9
}
const NO_SOCKET: u32 = rc(103);
const INVALID_SOCKET: u32 = rc(106);
const ALREADY_IN_USE: u32 = rc(203);
const WOULD_BLOCK: u32 = rc(204);
const TIMEOUT: u32 = rc(205);
const CONNECTION_RESET: u32 = rc(209);
const CONNECTION_ABORT: u32 = rc(210);

const OPT_DO_NOT_CLOSE_SOCKET: u32 = 0;
const IO_BLOCKING: u32 = 1;
const HANDSHAKE_TIMEOUT_MS: i32 = 30_000;
const WRITE_CHUNK: usize = 16 * 1024;

pub fn open() -> Arc<dyn Service> {
    Arc::new(SslService)
}

struct SslService;

impl Service for SslService {
    fn name(&self) -> String {
        "ssl".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 | 100 => rsp.objects.push(Arc::new(Context)),
            3 => rsp.push_u32(0),
            2 | 5 | 101 => {}
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct Context;

impl Service for Context {
    fn name(&self) -> String {
        "ssl::ISslContext".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => {}
            1 => rsp.push_u32(0),
            2 => rsp.objects.push(Arc::new(Connection(Mutex::new(Conn::default())))),
            3 => rsp.push_u32(1),
            4 | 5 | 8 | 10 => rsp.push_u64(1),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

#[derive(Debug)]
struct AcceptAny;

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

fn config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut c = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring supports the default TLS versions")
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAny))
                .with_no_client_auth();
            c.alpn_protocols = vec![b"http/1.1".to_vec()];
            Arc::new(c)
        })
        .clone()
}

struct FdIo(i32);

impl Read for FdIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = unsafe { libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT) };
        if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(n as usize) }
    }
}

impl Write for FdIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = unsafe { libc::send(self.0, buf.as_ptr().cast(), buf.len(), libc::MSG_DONTWAIT) };
        if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(n as usize) }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn wait(fd: i32, events: i16, timeout_ms: i32) -> bool {
    let mut p = libc::pollfd { fd, events, revents: 0 };
    loop {
        let r = unsafe { libc::poll(&mut p, 1, timeout_ms) };
        if r >= 0 || std::io::Error::last_os_error().kind() != ErrorKind::Interrupted {
            return r > 0;
        }
    }
}

#[derive(Default)]
struct Conn {
    gfd: i32,
    fd: Option<i32>,
    host: String,
    nonblocking: bool,
    verify: u32,
    cache_mode: u32,
    options: [bool; 4],
    alpn: Vec<u8>,
    tls: Option<ClientConnection>,
    plain: Vec<u8>,
    eof: bool,
    trace: Option<Trace>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(fd) = self.fd {
            unsafe { libc::close(fd) };
        }
    }
}

enum Step {
    Done,
    Again,
    Wait(i16),
}

impl Conn {
    fn peer(fd: i32) -> Option<Ipv4Addr> {
        let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        let ok = unsafe { libc::getpeername(fd, &mut sa as *mut _ as *mut libc::sockaddr, &mut len) } == 0;
        (ok && sa.sin_family as i32 == libc::AF_INET).then(|| Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)))
    }

    fn server_name(&self, peer: Ipv4Addr) -> ServerName<'static> {
        let mut host = self.host.clone();
        if host.is_empty() || host.parse::<Ipv4Addr>().is_ok() {
            host = nextendo::host_for(peer).unwrap_or_else(|| peer.to_string());
        }
        ServerName::try_from(host).unwrap_or_else(|_| ServerName::try_from("nintendo.net").unwrap())
    }

    fn pump(&mut self) -> Result<Step, u32> {
        let fd = self.fd.ok_or(NO_SOCKET)?;
        let tls = self.tls.as_mut().ok_or(NO_SOCKET)?;
        if tls.wants_write() {
            return match tls.write_tls(&mut FdIo(fd)) {
                Ok(_) => Ok(Step::Again),
                Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(Step::Wait(libc::POLLOUT)),
                Err(e) => {
                    eprintln!("[ssl] {}: send failed: {e}", self.host);
                    Err(CONNECTION_RESET)
                }
            };
        }
        if !tls.wants_read() {
            return Ok(Step::Done);
        }
        match tls.read_tls(&mut FdIo(fd)) {
            Ok(0) => {
                self.eof = true;
                Ok(Step::Done)
            }
            Ok(_) => match tls.process_new_packets() {
                Ok(_) => Ok(Step::Again),
                Err(e) => {
                    eprintln!("[ssl] {}: {e}", self.host);
                    Err(CONNECTION_ABORT)
                }
            },
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(Step::Wait(libc::POLLIN)),
            Err(e) => {
                eprintln!("[ssl] {}: receive failed: {e}", self.host);
                Err(CONNECTION_RESET)
            }
        }
    }

    fn drain(&mut self) {
        let Some(tls) = self.tls.as_mut() else { return };
        let mut tmp = [0u8; 16384];
        loop {
            match tls.reader().read(&mut tmp) {
                Ok(0) => {
                    self.eof = true;
                    return;
                }
                Ok(n) => self.plain.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(_) => {
                    self.eof = true;
                    return;
                }
            }
        }
    }
}

struct Connection(Mutex<Conn>);

impl Connection {
    fn handshake(&self) -> u32 {
        let mut c = self.0.lock().unwrap();
        let Some(fd) = c.fd else { return NO_SOCKET };
        let Some(peer) = Conn::peer(fd).filter(|&p| nextendo::is_server(p)) else {
            eprintln!("[ssl] {}: refused, not a Nextendo server", c.host);
            return CONNECTION_ABORT;
        };
        let name = c.server_name(peer);
        match ClientConnection::new(config(), name.clone()) {
            Ok(tls) => c.tls = Some(tls),
            Err(e) => {
                eprintln!("[ssl] {}: {e}", c.host);
                return CONNECTION_ABORT;
            }
        }
        drop(c);
        loop {
            let mut c = self.0.lock().unwrap();
            let busy = c.tls.as_ref().is_some_and(|t| t.is_handshaking());
            if !busy {
                let alpn = c.tls.as_ref().and_then(|t| t.alpn_protocol()).map(|p| String::from_utf8_lossy(p).into_owned());
                eprintln!("[ssl] connected to {} ({})", name.to_str(), alpn.as_deref().unwrap_or("no alpn"));
                if nextrace::enabled() {
                    c.trace = Some(Trace::default());
                }
                return 0;
            }
            if c.eof {
                eprintln!("[ssl] {}: closed during the handshake", name.to_str());
                return CONNECTION_ABORT;
            }
            match c.pump() {
                Ok(Step::Wait(events)) => {
                    drop(c);
                    if !crate::kernel::blocking(|| wait(fd, events, HANDSHAKE_TIMEOUT_MS)) {
                        eprintln!("[ssl] {}: handshake timed out", name.to_str());
                        return TIMEOUT;
                    }
                }
                Ok(_) => {}
                Err(r) => return r,
            }
        }
    }

    fn read(&self, out: &mut [u8], consume: bool) -> Result<usize, u32> {
        loop {
            let mut c = self.0.lock().unwrap();
            c.tls.as_ref().ok_or(NO_SOCKET)?;
            c.drain();
            if !c.plain.is_empty() || out.is_empty() {
                let n = c.plain.len().min(out.len());
                out[..n].copy_from_slice(&c.plain[..n]);
                if consume {
                    if let Some(t) = c.trace.as_mut() {
                        t.received(&out[..n]);
                    }
                    c.plain.drain(..n);
                }
                return Ok(n);
            }
            if c.eof {
                return Ok(0);
            }
            let fd = c.fd.ok_or(NO_SOCKET)?;
            match c.pump()? {
                Step::Wait(events) => {
                    if c.nonblocking {
                        return Err(WOULD_BLOCK);
                    }
                    drop(c);
                    crate::kernel::blocking(|| wait(fd, events, -1));
                }
                Step::Again => {}
                Step::Done if c.eof => {}
                Step::Done => return if c.nonblocking { Err(WOULD_BLOCK) } else { Ok(0) },
            }
        }
    }

    fn write(&self, data: &[u8]) -> Result<usize, u32> {
        let mut c = self.0.lock().unwrap();
        let fd = c.fd.ok_or(NO_SOCKET)?;
        let mut taken = None;
        loop {
            if !c.tls.as_ref().ok_or(NO_SOCKET)?.wants_write() {
                if let Some(n) = taken {
                    return Ok(n);
                }
                let chunk = &data[..data.len().min(WRITE_CHUNK)];
                let n = c.tls.as_mut().ok_or(NO_SOCKET)?.writer().write(chunk).map_err(|_| CONNECTION_ABORT)?;
                if let Some(t) = c.trace.as_mut() {
                    t.sent(&chunk[..n]);
                }
                taken = Some(n);
                continue;
            }
            if let Step::Wait(events) = c.pump()? {
                if c.nonblocking && taken.is_none() {
                    return Err(WOULD_BLOCK);
                }
                drop(c);
                if !crate::kernel::blocking(|| wait(fd, events, HANDSHAKE_TIMEOUT_MS)) {
                    return Err(TIMEOUT);
                }
                c = self.0.lock().unwrap();
            }
        }
    }
}

impl Service for Connection {
    fn name(&self) -> String {
        "ssl::ISslConnection".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        let result = |rsp: &mut Response, r: Result<usize, u32>| match r {
            Ok(n) => {
                rsp.push_u32(n as u32);
                0
            }
            Err(e) => e,
        };
        match req.cmd {
            0 => {
                let mut c = self.0.lock().unwrap();
                if c.fd.is_some() {
                    return ALREADY_IN_USE;
                }
                let gfd = req.u32(0) as i32;
                let Some(host) = bsd::host(gfd) else { return INVALID_SOCKET };
                let fd = unsafe { libc::dup(host) };
                if fd < 0 {
                    return INVALID_SOCKET;
                }
                c.fd = Some(fd);
                c.gfd = gfd;
                rsp.push_u32(if c.options[OPT_DO_NOT_CLOSE_SOCKET as usize] { u32::MAX } else { gfd as u32 });
            }
            1 => {
                let raw = bytes(in_buf(req, 0));
                let host = String::from_utf8_lossy(&raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())]).into_owned();
                self.0.lock().unwrap().host = host;
            }
            2 => self.0.lock().unwrap().verify = req.u32(0),
            3 => self.0.lock().unwrap().nonblocking = req.u32(0) != IO_BLOCKING,
            4 => rsp.push_u32(self.0.lock().unwrap().gfd as u32),
            5 => {
                let host = self.0.lock().unwrap().host.clone();
                if let Some(b) = out_buf(req, 0) {
                    write_buffer(&b, host.as_bytes());
                }
                rsp.push_u32(host.len() as u32);
            }
            6 => rsp.push_u32(self.0.lock().unwrap().verify),
            7 => rsp.push_u32(if self.0.lock().unwrap().nonblocking { 2 } else { IO_BLOCKING }),
            8 => return self.handshake(),
            9 => {
                let r = self.handshake();
                if r == 0 {
                    rsp.push_u32(0);
                    rsp.push_u32(0);
                }
                return r;
            }
            10 => return result(rsp, self.read(bytes_mut(out_buf(req, 0)), true)),
            11 => return result(rsp, self.write(bytes(in_buf(req, 0)))),
            12 => {
                let mut c = self.0.lock().unwrap();
                c.drain();
                rsp.push_u32(c.plain.len() as u32);
            }
            13 => return result(rsp, self.read(bytes_mut(out_buf(req, 0)), false)),
            15 => {}
            16 => rsp.push_u32(0),
            17 => self.0.lock().unwrap().cache_mode = req.u32(0),
            18 => rsp.push_u32(self.0.lock().unwrap().cache_mode),
            19 | 20 => {}
            21 => rsp.push_u32(0),
            22 => {
                let (value, option) = (req.u32(0) != 0, req.u32(4));
                if let Some(o) = self.0.lock().unwrap().options.get_mut(option as usize) {
                    *o = value;
                }
            }
            23 => rsp.data.push(self.0.lock().unwrap().options.get(req.u32(0) as usize).copied().unwrap_or(false) as u8),
            24 => {
                rsp.push_u32(0);
                rsp.push_u32(0);
            }
            26 => self.0.lock().unwrap().alpn = bytes(in_buf(req, 0)).to_vec(),
            27 => {
                let c = self.0.lock().unwrap();
                let chosen = c.tls.as_ref().and_then(|t| t.alpn_protocol()).map(<[u8]>::to_vec).unwrap_or_default();
                if let Some(b) = out_buf(req, 0) {
                    write_buffer(&b, &chosen);
                }
                rsp.push_u32(chosen.len() as u32);
            }
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}
