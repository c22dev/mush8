// Mush8 - nextendo.rs
// Constantin Clerc (c22dev) - 0.1
use crate::services::vnet::{self, Mode};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Mutex, OnceLock};

const SERVER: Ipv4Addr = Ipv4Addr::new(51, 178, 29, 194);
const NAT: Ipv4Addr = Ipv4Addr::new(164, 132, 111, 120);
const BAAS: &str = "e0d67c509fb203858ebcb2fe3f88c2aa.baas.nintendo.com";

pub fn enabled() -> bool {
    vnet::mode() == Mode::Nextendo
}

pub fn pid() -> u64 {
    std::env::var("NX_NEXTENDO_PID").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

fn token() -> String {
    std::env::var("NX_NEXTENDO_TOKEN").unwrap_or_default().trim().to_string()
}

pub fn nsd_resolve(host: &str) -> String {
    let host = host.replace('%', "lp1");
    match host.as_str() {
        "accounts.nintendo.com" | "api.accounts.nintendo.com" => BAAS.into(),
        _ => host,
    }
}

fn matches(pattern: &str, host: &str) -> bool {
    match pattern.split_once('*') {
        Some((pre, post)) => host.len() >= pre.len() + post.len() && host.starts_with(pre) && host.ends_with(post),
        None => pattern == host,
    }
}

static LAST_HOST: Mutex<Option<HashMap<Ipv4Addr, String>>> = Mutex::new(None);

pub fn redirect(host: &str) -> Option<Ipv4Addr> {
    let rules = [
        ("nncs2-*.n.n.srv.nintendo.net", NAT),
        ("g2b309e01-lp1.s.n.srv.nintendo.net", NAT),
        ("*.nintendo.net", SERVER),
        ("*.nintendo.com", SERVER),
        ("*.nintendowifi.net", SERVER),
        ("*.nintendo.co.jp", SERVER),
    ];
    let to = rules.iter().find(|(p, _)| matches(p, host)).map(|(_, a)| *a)?;
    LAST_HOST.lock().unwrap().get_or_insert_with(HashMap::new).insert(to, host.to_string());
    Some(to)
}

pub fn is_server(addr: Ipv4Addr) -> bool {
    addr == SERVER || addr == NAT
}

pub fn host_for(addr: Ipv4Addr) -> Option<String> {
    LAST_HOST.lock().unwrap().as_ref()?.get(&addr).cloned()
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    unsafe { libc::getentropy(b.as_mut_ptr().cast(), N) };
    b
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..=c.len() {
            out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn json_str(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const TOKEN_LIFE: u64 = 3 * 3600;

pub fn id_token() -> String {
    static IDS: OnceLock<(String, String, String)> = OnceLock::new();
    static TOKEN: Mutex<Option<(u64, String)>> = Mutex::new(None);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut cached = TOKEN.lock().unwrap();
    if let Some((_, token)) = cached.as_ref().filter(|(exp, _)| now + 600 < *exp) {
        return token.clone();
    }
    let (sub, di, did) = IDS.get_or_init(|| (hex(&random::<16>()), hex(&random::<16>()), hex(&random::<16>())));
    let u = random::<16>();
    let jti = format!("{}-{}-{}-{}-{}", hex(&u[..4]), hex(&u[4..6]), hex(&u[6..8]), hex(&u[8..10]), hex(&u[10..]));
    let header = r#"{"alg":"RS256","kid":"nextendo-baas-key-1","typ":"id_token"}"#;
    let exp = now + TOKEN_LIFE;
    let mut claims = vec![
        format!("\"sub\":{}", json_str(sub)),
        "\"aud\":\"ed9e2f05d286f7b8\"".to_string(),
        format!("\"iss\":\"https://{BAAS}\""),
        format!("\"iat\":{now},\"nbf\":{now},\"exp\":{exp}"),
        format!("\"jku\":\"https://{BAAS}/1.0.0/certificates\""),
        format!("\"jti\":\"{jti}\""),
        format!("\"di\":\"{di}\""),
        "\"sn\":\"XAW10000000000\"".to_string(),
        format!("\"bs:did\":\"{did}\""),
        "\"hm\":true".to_string(),
        format!("\"tv\":\"{}\"", crate::game::version()),
    ];
    let nex = token();
    if !nex.is_empty() {
        claims.push(format!("\"nnex\":{}", json_str(&nex)));
    }
    let payload = format!("{{{}}}", claims.join(","));
    let token = format!("{}.{}.{}", base64url(header.as_bytes()), base64url(payload.as_bytes()), base64url(&random::<256>()));
    diag_once(&format!("id token {exp}"), || format!("[nextendo] new id token: {}", crate::services::nextrace::describe_jwt(&token, now, pid())));
    *cached = Some((exp, token.clone()));
    token
}

/// Logs a diagnostic line once per key, when NX_NEXTENDO_TRACE=1.
pub fn diag_once(key: &str, line: impl FnOnce() -> String) {
    static SEEN: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);
    if !crate::services::nextrace::enabled() {
        return;
    }
    if SEEN.lock().unwrap().get_or_insert_with(Default::default).insert(key.to_string()) {
        eprintln!("{}", line());
    }
}

pub fn log_start() {
    if !enabled() {
        return;
    }
    match (pid(), token().is_empty()) {
        (0, _) | (_, true) => eprintln!("[nextendo] not signed in : sign in from the launcher to play online"),
        (pid, false) => eprintln!("[nextendo] signed in, pid {pid}"),
    }
    let nex = token();
    if !nex.is_empty() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        eprintln!("[nextendo] game token: {}", crate::services::nextrace::describe_nnex(&nex, now, pid()));
    }
    if let Ok(gate) = std::env::var("NX_NEXTENDO_GATE") {
        eprintln!("[nextendo] account online gate (/api/online-status): {gate}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_rfc4648() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn redirects_follow_prelude() {
        assert_eq!(redirect("g2b309e01-lp1.s.n.srv.nintendo.net"), Some(NAT));
        assert_eq!(redirect("nncs2-lp1.n.n.srv.nintendo.net"), Some(NAT));
        assert_eq!(redirect("nncs1-lp1.n.n.srv.nintendo.net"), Some(SERVER));
        assert_eq!(redirect("e0d67c509fb203858ebcb2fe3f88c2aa.baas.nintendo.com"), Some(SERVER));
        assert_eq!(redirect("example.com"), None);
        assert!(host_for(NAT).is_some());
    }

    #[test]
    fn nsd_fills_in_the_environment() {
        assert_eq!(nsd_resolve("g2b309e01-%.s.n.srv.nintendo.net"), "g2b309e01-lp1.s.n.srv.nintendo.net");
        assert_eq!(nsd_resolve("accounts.nintendo.com"), BAAS);
    }
}
