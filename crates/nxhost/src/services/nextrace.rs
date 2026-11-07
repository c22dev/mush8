// Mush8 - nextrace.rs
// Constantin Clerc (c22dev) - 0.1
use crate::services::nextendo;
use std::collections::VecDeque;

const TYPE_SYN: u16 = 0;
const TYPE_CONNECT: u16 = 1;
const TYPE_DATA: u16 = 2;
const TYPE_DISCONNECT: u16 = 3;
const FLAG_ACK: u16 = 1;
const PROTO_AUTH: u16 = 0x0a;
const MAX_BUFFER: usize = 1 << 20;
const SEEN_WINDOW: usize = 1024;
const HTTP_HEADS: [&[u8]; 2] = [b"GET ", b"HTTP/"];

pub fn enabled() -> bool {
    nextendo::enabled() && std::env::var("NX_NEXTENDO_TRACE").is_ok_and(|v| v.trim() == "1")
}

#[derive(Default)]
pub struct Trace {
    sent: Side,
    received: Side,
}

impl Trace {
    pub fn sent(&mut self, data: &[u8]) {
        self.sent.feed(data, true);
    }
    pub fn received(&mut self, data: &[u8]) {
        self.received.feed(data, false);
    }
}

#[derive(Default, Debug, PartialEq)]
enum Framing {
    #[default]
    Unknown,
    WebSocket,
    Raw,
    Off,
}

#[derive(Default)]
struct Side {
    framing: Framing,
    raw: Vec<u8>,
    lite: Vec<u8>,
    message: Vec<u8>,
    seen: VecDeque<PacketId>,
}

type PacketId = (u8, u8, u8, u16);

fn arrow(out: bool) -> &'static str {
    if out { "->" } else { "<-" }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl Side {
    fn feed(&mut self, data: &[u8], out: bool) {
        if self.framing == Framing::Off {
            return;
        }
        self.raw.extend_from_slice(data);
        if self.raw.len() > MAX_BUFFER {
            eprintln!("[nextrace] {} buffer overflow, trace stopped", arrow(out));
            self.framing = Framing::Off;
            return;
        }
        if self.framing == Framing::Unknown {
            if HTTP_HEADS.iter().any(|h| self.raw.starts_with(h)) {
                let Some(end) = find(&self.raw, b"\r\n\r\n") else { return };
                let head = String::from_utf8_lossy(&self.raw[..end]).into_owned();
                log_http(out, &head);
                self.raw.drain(..end + 4);
                if !is_websocket_upgrade(&head) {
                    self.raw.clear();
                    self.framing = Framing::Off;
                    return;
                }
                self.framing = Framing::WebSocket;
            } else if self.raw.first() == Some(&0x80) {
                eprintln!("[nextrace] {} raw PRUDP lite stream", arrow(out));
                self.framing = Framing::Raw;
            } else if self.raw.len() >= 4 && !HTTP_HEADS.iter().any(|h| h.starts_with(&self.raw)) {
                let head: String = self.raw.iter().take(8).map(|b| format!("{b:02x}")).collect();
                eprintln!("[nextrace] {} unknown framing ({head}), trace stopped", arrow(out));
                self.framing = Framing::Off;
                return;
            } else {
                return;
            }
        }
        match self.framing {
            Framing::WebSocket => {
                while let Some((used, op, payload)) = ws_frame(&self.raw) {
                    self.raw.drain(..used);
                    match op {
                        0 | 2 => self.lite.extend_from_slice(&payload),
                        8 => {
                            let code = payload.get(..2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                            let reason = payload.get(2..).map(String::from_utf8_lossy).unwrap_or_default();
                            eprintln!("[nextrace] {} websocket close {code:?} {reason:?}", arrow(out));
                        }
                        1 => eprintln!("[nextrace] {} websocket text frame ({} bytes)", arrow(out), payload.len()),
                        _ => {}
                    }
                }
            }
            Framing::Raw => self.lite.append(&mut self.raw),
            _ => return,
        }
        self.packets(out);
    }

    fn packets(&mut self, out: bool) {
        while self.lite.len() >= 12 {
            if self.lite[0] != 0x80 {
                eprintln!("[nextrace] {} unexpected PRUDP lite magic {:#04x}, trace stopped", arrow(out), self.lite[0]);
                self.framing = Framing::Off;
                return;
            }
            let options = self.lite[1] as usize;
            let size = u16::from_le_bytes([self.lite[2], self.lite[3]]) as usize;
            let total = 12 + options + size;
            if self.lite.len() < total {
                return;
            }
            let (stream, source, destination) = (self.lite[4], self.lite[5], self.lite[6]);
            let fragment = self.lite[7];
            let type_flags = u16::from_le_bytes([self.lite[8], self.lite[9]]);
            let id = u16::from_le_bytes([self.lite[10], self.lite[11]]);
            let payload = self.lite[12 + options..total].to_vec();
            self.lite.drain(..total);
            let (ty, flags) = (type_flags & 0xf, type_flags >> 4);
            let ack = if flags & FLAG_ACK != 0 { " ack" } else { "" };
            match ty {
                TYPE_SYN => eprintln!("[nextrace] {} SYN{ack}", arrow(out)),
                TYPE_CONNECT => eprintln!("[nextrace] {} CONNECT{ack}", arrow(out)),
                TYPE_DISCONNECT => eprintln!("[nextrace] {} DISCONNECT{ack}", arrow(out)),
                TYPE_DATA if flags & FLAG_ACK == 0 && !payload.is_empty() => {
                    if !self.first_time((stream, source, destination, id)) {
                        continue;
                    }
                    self.message.extend_from_slice(&payload);
                    if fragment == 0 {
                        let m = std::mem::take(&mut self.message);
                        rmc(out, &m);
                    }
                }
                _ => {}
            }
        }
    }

    fn first_time(&mut self, id: PacketId) -> bool {
        if self.seen.contains(&id) {
            return false;
        }
        if self.seen.len() == SEEN_WINDOW {
            self.seen.pop_front();
        }
        self.seen.push_back(id);
        true
    }
}

fn is_websocket_upgrade(head: &str) -> bool {
    let head = head.to_ascii_lowercase();
    if head.starts_with("http/") {
        return head.split_whitespace().nth(1) == Some("101");
    }
    head.contains("\nupgrade:") || head.contains("\nsec-websocket-key:")
}

fn without_query(line: &str) -> String {
    let Some((path, rest)) = line.split_once('?') else { return line.to_string() };
    let version = rest.rsplit_once(' ').map_or("", |(_, v)| v);
    format!("{path}?<query> {version}")
}

fn log_http(out: bool, head: &str) {
    let mut lines = head.lines();
    let first = without_query(lines.next().unwrap_or_default());
    let keep = ["host", "upgrade", "connection", "sec-websocket-protocol", "sec-websocket-version"];
    let headers: Vec<String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| if keep.contains(&k.trim().to_ascii_lowercase().as_str()) { format!("{}: {}", k.trim(), v.trim()) } else { k.trim().to_string() })
        .collect();
    eprintln!("[nextrace] {} {first} [{}]", arrow(out), headers.join(", "));
}

fn ws_frame(buf: &[u8]) -> Option<(usize, u8, Vec<u8>)> {
    let (&b0, &b1) = (buf.first()?, buf.get(1)?);
    let mut len = (b1 & 0x7f) as u64;
    let mut i = 2;
    if len == 126 {
        len = u16::from_be_bytes(buf.get(2..4)?.try_into().ok()?) as u64;
        i = 4;
    } else if len == 127 {
        len = u64::from_be_bytes(buf.get(2..10)?.try_into().ok()?);
        i = 10;
    }
    let mask: Option<[u8; 4]> = if b1 & 0x80 != 0 {
        let m = buf.get(i..i + 4)?.try_into().ok()?;
        i += 4;
        Some(m)
    } else {
        None
    };
    let end = i.checked_add(usize::try_from(len).ok()?)?;
    let mut payload = buf.get(i..end)?.to_vec();
    if let Some(m) = mask {
        payload.iter_mut().enumerate().for_each(|(k, b)| *b ^= m[k % 4]);
    }
    Some((end, b0 & 0x0f, payload))
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn string(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        let s = self.take(n)?;
        Some(String::from_utf8_lossy(s.strip_suffix(&[0]).unwrap_or(s)).into_owned())
    }
    fn rest(&self) -> &'a [u8] {
        &self.data[self.pos.min(self.data.len())..]
    }
}

pub fn result_name(code: u32) -> &'static str {
    if code & 0x8000_0000 == 0 {
        return "success";
    }
    match code & !0x8000_0000 {
        0x0001_0001 => "Core::Unknown",
        0x0001_000a => "Core::InvalidArgument",
        0x0001_000b => "Core::Timeout",
        0x0068_0001 => "Authentication::NASAuthenticateError",
        0x0068_0002 => "Authentication::TokenParseError",
        0x0068_0006 => "Authentication::TokenExpired",
        0x0068_0007 => "Authentication::ValidationFailed",
        0x0068_0008 => "Authentication::InvalidParam",
        0x0068_0009 => "Authentication::PrincipalIdUnmatched",
        0x0068_000b => "Authentication::UnderMaintenance",
        0x0068_000c => "Authentication::UnsupportedVersion",
        0x0068_0013 => "Authentication::ApplicationVersionIsOld",
        0x0068_0015 => "Authentication::NetworkServiceLicenseRequired",
        _ => "?",
    }
}

fn rmc(out: bool, m: &[u8]) {
    if m.len() < 9 {
        return;
    }
    let mut r = Reader::new(m);
    let Some(size) = r.u32() else { return };
    if size as usize != m.len() - 4 {
        eprintln!("[nextrace] {} RMC size {size} but message has {} bytes", arrow(out), m.len() - 4);
    }
    let Some(p) = r.u8() else { return };
    let mut proto = (p & 0x7f) as u16;
    if proto == 0x7f {
        let Some(ext) = r.u16() else { return };
        proto = ext;
    }
    if p & 0x80 != 0 {
        let (Some(call), Some(method)) = (r.u32(), r.u32()) else { return };
        eprintln!("[nextrace] {} request protocol {proto:#x} method {method} call {call} ({} bytes)", arrow(out), r.rest().len());
        if proto == PROTO_AUTH {
            auth_request(method, r.rest());
        }
        return;
    }
    match r.u8() {
        Some(1) => {
            let (Some(call), Some(method)) = (r.u32(), r.u32()) else { return };
            let method = method & 0x7fff;
            eprintln!("[nextrace] {} response protocol {proto:#x} method {method} call {call} ({} bytes)", arrow(out), r.rest().len());
            if proto == PROTO_AUTH {
                auth_response(method, r.rest());
            }
        }
        Some(_) => {
            let (Some(code), Some(call)) = (r.u32(), r.u32()) else { return };
            eprintln!("[nextrace] {} error protocol {proto:#x} call {call}: {code:#010x} {}", arrow(out), result_name(code));
        }
        None => {}
    }
}

fn is_login(method: u32) -> bool {
    matches!(method, 1 | 2 | 6)
}

fn auth_request(method: u32, body: &[u8]) {
    if !is_login(method) {
        return;
    }
    let mut r = Reader::new(body);
    let username = match method {
        1 | 2 => r.string(),
        6 => {
            let _ = (r.u8(), r.u32(), r.u32());
            r.string()
        }
        _ => None,
    };
    let pid = nextendo::pid();
    if let Some(u) = &username {
        let digits = !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit());
        eprintln!("[nextrace]    username: {} chars, decimal {digits}, equals the launcher PID {}", u.len(), u.parse::<u64>().ok() == Some(pid) && pid != 0);
    }
    let rest = r.rest();
    eprintln!("[nextrace]    AuthenticationInfo holder: {}", find(rest, b"AuthenticationInfo\0").is_some());
    let Some(i) = find(rest, b"eyJ") else {
        eprintln!("[nextrace]    no JWT in the login data");
        return;
    };
    let end = rest[i..].iter().position(|&b| !is_jwt_byte(b)).map_or(rest.len(), |n| i + n);
    let jwt = String::from_utf8_lossy(&rest[i..end]).into_owned();
    let framed = i >= 2 && u16::from_le_bytes([rest[i - 2], rest[i - 1]]) as usize == jwt.len() + 1 && rest.get(end) == Some(&0);
    eprintln!("[nextrace]    token string framed as a NEX string with NUL: {framed}");
    eprintln!("[nextrace]    {}", describe_jwt(&jwt, now(), pid));
    if framed {
        let mut t = Reader::new(&rest[end + 1..]);
        if let (Some(ngs), Some(kind), Some(server)) = (t.u32(), t.u8(), t.u32()) {
            eprintln!("[nextrace]    NGS version {ngs}, token type {kind}, server version {server}");
        }
    }
    if method == 6 {
        if let Some(tail) = rest.len().checked_sub(9).and_then(|n| rest.get(n..)) {
            let mut t = Reader::new(tail);
            if let (Some(skip), Some(nex), Some(client)) = (t.u8(), t.u32(), t.u32()) {
                eprintln!("[nextrace]    skip version check {skip}, NEX version {nex}, client version {client}");
            }
        }
    }
}

fn auth_response(method: u32, body: &[u8]) {
    if !is_login(method) {
        return;
    }
    let mut r = Reader::new(body);
    if method == 6 {
        let _ = (r.u8(), r.u32());
        let pid = r.u64();
        eprintln!("[nextrace]    ticket result: PID is the account {}", pid == Some(nextendo::pid()));
        return;
    }
    let (Some(retval), Some(pid)) = (r.u32(), r.u64()) else { return };
    let verdict = if retval & 0x8000_0000 != 0 { "REJECTED" } else { "accepted" };
    eprintln!("[nextrace]    login {verdict}: retval {retval:#010x} {}, PID is the account {}", result_name(retval), pid == nextendo::pid() && pid != 0);
    let after_ticket = r.u32().and_then(|n| r.take(n as usize)).map_or(&[][..], |_| r.rest());
    let strings = nex_strings(after_ticket);
    if !strings.is_empty() {
        eprintln!("[nextrace]    response strings: {strings:?}");
    }
}

fn nex_strings(body: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < body.len() {
        let n = u16::from_le_bytes([body[i], body[i + 1]]) as usize;
        if (2..=512).contains(&n) && body.get(i + 2 + n - 1) == Some(&0) {
            let s = &body[i + 2..i + 2 + n - 1];
            if s.iter().all(|b| (0x20..0x7f).contains(b)) {
                let text = String::from_utf8_lossy(s).into_owned();
                let hexish = text.len() >= 16 && text.bytes().all(|b| b.is_ascii_hexdigit());
                out.push(if hexish { format!("<{} hex chars>", text.len()) } else { text });
                i += 2 + n;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn is_jwt_byte(b: u8) -> bool {
    b == b'.' || b == b'-' || b == b'_' || b.is_ascii_alphanumeric()
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

pub fn flat_json(s: &str) -> Option<Vec<(String, String)>> {
    let b = s.trim().as_bytes();
    if b.first() != Some(&b'{') || b.last() != Some(&b'}') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 1;
    let string = |i: &mut usize| -> Option<String> {
        if b.get(*i) != Some(&b'"') {
            return None;
        }
        *i += 1;
        let mut v = String::new();
        while let Some(&c) = b.get(*i) {
            *i += 1;
            match c {
                b'"' => return Some(v),
                b'\\' => {
                    let e = *b.get(*i)?;
                    *i += 1;
                    v.push(match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'u' => {
                            *i += 4;
                            '?'
                        }
                        e => e as char,
                    });
                }
                c => v.push(c as char),
            }
        }
        None
    };
    loop {
        while b.get(i).is_some_and(|c| c.is_ascii_whitespace() || *c == b',') {
            i += 1;
        }
        if b.get(i) == Some(&b'}') {
            return Some(out);
        }
        let key = string(&mut i)?;
        while b.get(i).is_some_and(|c| c.is_ascii_whitespace() || *c == b':') {
            i += 1;
        }
        let value = if b.get(i) == Some(&b'"') {
            string(&mut i)?
        } else {
            let start = i;
            while b.get(i).is_some_and(|c| *c != b',' && *c != b'}') {
                i += 1;
            }
            String::from_utf8_lossy(&b[start..i]).trim().to_string()
        };
        out.push((key, value));
    }
}

pub fn describe_nnex(token: &str, now: u64, pid: u64) -> String {
    let Some(rest) = token.strip_prefix("nx2.") else { return format!("not an nx2 token ({} chars)", token.len()) };
    let Some((payload, mac)) = rest.split_once('.') else { return "nx2 token without a signature".into() };
    let Some(raw) = base64url_decode(payload).and_then(|p| String::from_utf8(p).ok()) else { return "nx2 payload is not base64url text".into() };
    let Some((p, (user, exp))) = raw.split_once('.').and_then(|(p, tail)| Some((p, tail.rsplit_once('.')?))) else {
        return format!("nx2 payload has {} fields, expected pid.username.expiry", raw.split('.').count());
    };
    let pid_ok = p.parse::<u64>().ok() == Some(pid) && pid != 0;
    let expiry = match exp.parse::<i64>() {
        Ok(e) => format!("{:+.1} days", (e - now as i64) as f64 / 86400.0),
        Err(_) => "unreadable".into(),
    };
    let mac_ok = mac.len() == 43 && base64url_decode(mac).is_some_and(|m| m.len() == 32);
    format!("nx2 PID matches the launcher PID {pid_ok}, username {} chars, expires in {expiry}, HMAC-SHA256 shape {mac_ok}", user.len())
}

pub fn describe_jwt(jwt: &str, now: u64, pid: u64) -> String {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return format!("JWT {} chars with {} segments (expected 3: truncated?)", jwt.len(), parts.len());
    }
    let header = base64url_decode(parts[0]).and_then(|h| String::from_utf8(h).ok()).unwrap_or_else(|| "<undecodable>".into());
    let Some(claims) = base64url_decode(parts[1]).and_then(|p| String::from_utf8(p).ok()).and_then(|p| flat_json(&p)) else {
        return format!("JWT {} chars, header {header}, payload undecodable", jwt.len());
    };
    let get = |k: &str| claims.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let keys: Vec<&str> = claims.iter().map(|(k, _)| k.as_str()).collect();
    let rel = |k: &str| get(k).and_then(|v| v.parse::<i64>().ok()).map_or("missing".into(), |t| format!("{:+}s", t - now as i64));
    let sig = base64url_decode(parts[2]).map_or(0, |s| s.len());
    let nnex = get("nnex").map_or("nnex MISSING".into(), |t| describe_nnex(t, now, pid));
    format!(
        "JWT {} chars, header {header}, claims {keys:?}, aud {:?} iss {:?} tv {:?} hm {:?}, iat {} nbf {} exp {}, signature {sig} bytes; {nnex}",
        jwt.len(),
        get("aud").unwrap_or("-"),
        get("iss").unwrap_or("-"),
        get("tv").unwrap_or("-"),
        get("hm").unwrap_or("-"),
        rel("iat"),
        rel("nbf"),
        rel("exp"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for c in s.chunks(3) {
            let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
            for i in 0..=c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    }

    #[test]
    fn base64url_round_trip() {
        for s in [&b""[..], b"f", b"fo", b"foo", b"foobar", &[0xfb, 0xff, 0x00]] {
            assert_eq!(base64url_decode(&b64(s)).unwrap(), s);
        }
        assert!(base64url_decode("a+b").is_none());
    }

    #[test]
    fn flat_json_reads_claims() {
        let c = flat_json(r#"{"sub":"ab","iat":12,"hm":true,"nnex":"nx2.x.y","q":"a\"b"}"#).unwrap();
        assert_eq!(c[1], ("iat".into(), "12".into()));
        assert_eq!(c[2], ("hm".into(), "true".into()));
        assert_eq!(c[4], ("q".into(), "a\"b".into()));
        assert!(flat_json("[]").is_none());
    }

    #[test]
    fn nnex_description_is_redacted_and_checks_pid() {
        let mac = b64(&[7u8; 32]);
        let tok = format!("nx2.{}.{mac}", b64(b"1800000042.Secret.1000086400"));
        let d = describe_nnex(&tok, 1_000_000_000, 1800000042);
        assert!(d.contains("PID matches the launcher PID true"), "{d}");
        assert!(d.contains("+1.0 days") && d.contains("shape true"), "{d}");
        assert!(!d.contains("Secret") && !d.contains(&mac), "{d}");
        assert!(describe_nnex(&tok, 1_000_000_000, 1).contains("launcher PID false"));
    }

    #[test]
    fn jwt_description_flags_truncation() {
        assert!(describe_jwt("eyJhbGciOi.eyJ", 0, 0).contains("truncated"));
    }

    #[test]
    fn decodes_a_websocket_login_request() {
        let mut body = Vec::new();
        let user = b"1800000042\0";
        body.extend_from_slice(&(user.len() as u16).to_le_bytes());
        body.extend_from_slice(user);
        let mut rmc = vec![0x80 | 0x0a];
        rmc.extend_from_slice(&1u32.to_le_bytes());
        rmc.extend_from_slice(&2u32.to_le_bytes());
        rmc.extend_from_slice(&body);
        let mut msg = (rmc.len() as u32).to_le_bytes().to_vec();
        msg.extend_from_slice(&rmc);
        let mut lite = vec![0x80, 0, 0, 0, 0x11, 1, 1, 0];
        lite[2..4].copy_from_slice(&(msg.len() as u16).to_le_bytes());
        lite.extend_from_slice(&(TYPE_DATA | 2 << 4).to_le_bytes());
        lite.extend_from_slice(&1u16.to_le_bytes());
        lite.extend_from_slice(&msg);
        let mut ws = vec![0x82, 0x80 | lite.len() as u8, 1, 2, 3, 4];
        ws.extend(lite.iter().enumerate().map(|(i, b)| b ^ [1, 2, 3, 4][i % 4]));
        let mut t = Trace::default();
        t.sent(b"GET / HTTP/1.1\r\nHost: x\r\nSec-WebSocket-Key: k\r\n\r\n");
        t.sent(&ws[..5]);
        assert_eq!(t.sent.framing, Framing::WebSocket);
        t.sent(&ws[5..]);
        assert!(t.sent.raw.is_empty() && t.sent.lite.is_empty());
        assert!(t.sent.seen.contains(&(0x11, 1, 1, 1)));
        assert_eq!(result_name(0x8068_0007), "Authentication::ValidationFailed");
        assert_eq!(result_name(0x0001_0001), "success");
        assert_eq!(result_name(0x8001_000a), "Core::InvalidArgument");
        assert_eq!(result_name(0x8001_000b), "Core::Timeout");
    }

    #[test]
    fn split_http_head_keeps_the_trace_running() {
        let mut t = Trace::default();
        t.received(b"HTTP");
        assert_eq!(t.received.framing, Framing::Unknown);
        t.received(b"/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n");
        assert_eq!(t.received.framing, Framing::WebSocket);
    }

    #[test]
    fn unknown_bytes_still_stop_the_trace() {
        let mut t = Trace::default();
        t.received(b"nope");
        assert_eq!(t.received.framing, Framing::Off);
    }

    #[test]
    fn only_a_real_upgrade_switches_to_websocket() {
        for head in [&b"HTTP/1.1 403 Forbidden\r\n\r\nHello"[..], b"HTTP/1.1 200 OK\r\n\r\nHello", b"GET /index.html HTTP/1.1\r\nHost: x\r\n\r\n"] {
            let mut t = Trace::default();
            t.received(head);
            assert_eq!(t.received.framing, Framing::Off, "{}", String::from_utf8_lossy(head));
            assert!(t.received.raw.is_empty() && t.received.lite.is_empty());
        }
    }

    #[test]
    fn query_strings_are_not_logged() {
        assert_eq!(without_query("GET /ws?token=secret HTTP/1.1"), "GET /ws?<query> HTTP/1.1");
        assert_eq!(without_query("GET /ws HTTP/1.1"), "GET /ws HTTP/1.1");
    }

    #[test]
    fn retransmits_are_dropped_per_stream_and_ids_can_wrap() {
        let mut s = Side::default();
        assert!(s.first_time((0x11, 1, 1, 7)));
        assert!(!s.first_time((0x11, 1, 1, 7)));
        assert!(s.first_time((0x11, 1, 2, 7)));
        for id in 0..SEEN_WINDOW as u16 {
            s.first_time((0x11, 1, 1, 1000 + id));
        }
        assert_eq!(s.seen.len(), SEEN_WINDOW);
        assert!(s.first_time((0x11, 1, 1, 7)));
    }

    #[test]
    fn only_login_methods_are_parsed_as_logins() {
        assert!(is_login(1) && is_login(2) && is_login(6));
        assert!(!is_login(3) && !is_login(4) && !is_login(5));
    }

    #[test]
    fn nnex_expiry_survives_a_dotted_username() {
        let tok = format!("nx2.{}.{}", b64(b"1800000042.john.doe.1000086400"), b64(&[7u8; 32]));
        let d = describe_nnex(&tok, 1_000_000_000, 1800000042);
        assert!(d.contains("username 8 chars") && d.contains("+1.0 days"), "{d}");
        assert!(describe_nnex(&format!("nx2.{}.x", b64(b"1800000042.1000086400")), 0, 0).contains("2 fields"));
    }

    #[test]
    fn finds_nex_strings_in_a_response() {
        let mut b = vec![9, 9];
        b.extend_from_slice(&6u16.to_le_bytes());
        b.extend_from_slice(b"hello\0");
        b.extend_from_slice(&33u16.to_le_bytes());
        b.extend_from_slice(&[b'a'; 32]);
        b.push(0);
        assert_eq!(nex_strings(&b), vec!["hello".to_string(), "<32 hex chars>".to_string()]);
    }
}
