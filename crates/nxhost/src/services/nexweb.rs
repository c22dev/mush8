// Mush8 - nexweb.rs
// Constantin Clerc (c22dev) - 0.1
use crate::services::nextendo;
use block2::RcBlock;
use objc2_foundation::{NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL, NSURLResponse, NSURLSession};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const BASE: &str = "https://nextendo.network";
const CLIENT_ID: &str = "nxc_PZv-XFn79sLo";
pub const MK8: u64 = crate::svc::PROGRAM_ID;
const FRIENDS_TTL: Duration = Duration::from_secs(10);
const PRESENCE_EVERY: Duration = Duration::from_secs(45);
const PRESENCE_MIN_GAP: Duration = Duration::from_secs(3);
const MAX_DEPTH: usize = 64;
const ACCESS_LIFE: Duration = Duration::from_secs(50 * 60);
const RENEW_RETRY: Duration = Duration::from_secs(5 * 60);
const HISTORY_TICK: Duration = Duration::from_secs(15);
const HISTORY_EVERY: Duration = Duration::from_secs(300);
const HISTORY_FIRST: Duration = Duration::from_secs(60);
const MK8_NAME: &str = "Mario Kart 8 Deluxe";

fn signed_in_access() -> String {
    std::env::var("NX_NEXTENDO_ACCESS").unwrap_or_default().trim().to_string()
}

fn active() -> bool {
    nextendo::enabled() && !signed_in_access().is_empty()
}

struct Access {
    token: String,
    since: Option<Instant>,
    failed: Option<Instant>,
}

static ACCESS: Mutex<Access> = Mutex::new(Access { token: String::new(), since: None, failed: None });

#[cfg(target_os = "ios")]
fn renewed_access() -> Option<String> {
    unsafe extern "C" {
        fn mush8_nextendo_access() -> *mut std::ffi::c_char;
    }
    let raw = unsafe { mush8_nextendo_access() };
    if raw.is_null() {
        return None;
    }
    let token = unsafe { std::ffi::CStr::from_ptr(raw) }.to_string_lossy().into_owned();
    unsafe { libc::free(raw.cast()) };
    Some(token)
}

#[cfg(not(target_os = "ios"))]
static REFRESH: Mutex<Option<String>> = Mutex::new(None);

#[cfg(not(target_os = "ios"))]
fn renewed_access() -> Option<String> {
    let mut refresh = REFRESH.lock().unwrap();
    let current = refresh.get_or_insert_with(|| std::env::var("NX_NEXTENDO_REFRESH").unwrap_or_default().trim().to_string()).clone();
    if current.is_empty() {
        return None;
    }
    let form = format!("grant_type=refresh_token&client_id={}&refresh_token={}", form_value(CLIENT_ID), form_value(&current));
    let Ok((200, body)) = request("POST", "/api/oauth/token", None, Some(("application/x-www-form-urlencoded", &form))) else { return None };
    let reply = Json::parse(&String::from_utf8_lossy(&body))?;
    let next = reply.get("refresh_token").str();
    if !next.is_empty() {
        save_refresh(next);
        *refresh = Some(next.to_string());
    }
    Some(reply.get("access_token").str().to_string()).filter(|t| !t.is_empty())
}

#[cfg(not(target_os = "ios"))]
fn save_refresh(token: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let Some(home) = std::env::var_os("HOME") else { return };
    let dir = std::path::Path::new(&home).join("Library/Application Support/Mush8");
    let (tmp, file) = (dir.join("nextendo-refresh.tmp"), dir.join("nextendo-refresh"));
    let saved = std::fs::create_dir_all(&dir).and_then(|_| {
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &file)
    });
    if let Err(e) = saved {
        eprintln!("[nextendo] could not keep the renewed sign-in: {e}");
    }
}

#[cfg(any(not(target_os = "ios"), test))]
fn form_value(v: &str) -> String {
    v.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn access(renew: bool) -> String {
    let mut a = ACCESS.lock().unwrap();
    if a.since.is_none() {
        a.token = signed_in_access();
        a.since = Some(Instant::now());
    }
    let due = renew || a.since.is_some_and(|s| s.elapsed() >= ACCESS_LIFE);
    let waiting = a.failed.is_some_and(|f| f.elapsed() < RENEW_RETRY);
    if due && !waiting {
        match renewed_access() {
            Some(token) => {
                a.token = token;
                a.since = Some(Instant::now());
                a.failed = None;
            }
            None => {
                if a.failed.is_none() {
                    eprintln!("[nextendo] could not renew the sign-in for friends and presence");
                }
                a.failed = Some(Instant::now());
            }
        }
    }
    a.token.clone()
}

fn http(method: &str, path: &str, body: Option<&str>) -> Result<(isize, Vec<u8>), String> {
    let sent = access(false);
    let reply = send(&sent, method, path, body);
    if !matches!(reply, Ok((401, _))) {
        return reply;
    }
    let fresh = match access(false) {
        same if same == sent => access(true),
        newer => newer,
    };
    if fresh == sent { reply } else { send(&fresh, method, path, body) }
}

fn send(token: &str, method: &str, path: &str, body: Option<&str>) -> Result<(isize, Vec<u8>), String> {
    request(method, path, Some(token), body.map(|b| ("application/json", b)))
}

fn request(method: &str, path: &str, token: Option<&str>, body: Option<(&str, &str)>) -> Result<(isize, Vec<u8>), String> {
    let rx = objc2::rc::autoreleasepool(|_| {
        let url = NSURL::URLWithString(&NSString::from_str(&format!("{BASE}{path}"))).ok_or("bad url")?;
        let req = NSMutableURLRequest::requestWithURL(&url);
        req.setHTTPMethod(&NSString::from_str(method));
        req.setTimeoutInterval(10.0);
        let header = |k: &str, v: &str| req.setValue_forHTTPHeaderField(Some(&NSString::from_str(v)), &NSString::from_str(k));
        if let Some(t) = token {
            header("Authorization", &format!("Bearer {t}"));
        }
        header("User-Agent", "Mush8");
        header("X-Nextendo-Client-Id", CLIENT_ID);
        if let Some((kind, b)) = body {
            header("Content-Type", kind);
            req.setHTTPBody(Some(&NSData::with_bytes(b.as_bytes())));
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let done = RcBlock::new(move |data: *mut NSData, resp: *mut NSURLResponse, err: *mut NSError| {
            let reply = match unsafe { err.as_ref() } {
                Some(e) => Err(format!("{} ({})", e.localizedDescription(), e.code())),
                None => {
                    let status = unsafe { resp.as_ref() }.and_then(|r| r.downcast_ref::<NSHTTPURLResponse>()).map_or(0, |r| r.statusCode());
                    Ok((status, unsafe { data.as_ref() }.map(|d| d.to_vec()).unwrap_or_default()))
                }
            };
            let _ = tx.try_send(reply);
        });
        let task = unsafe { NSURLSession::sharedSession().dataTaskWithRequest_completionHandler(&req, &done) };
        task.resume();
        Ok::<_, String>(rx)
    })?;
    rx.recv_timeout(Duration::from_secs(15)).map_err(|_| "no answer in 15s".to_string())?
}

fn why(r: &Result<(isize, Vec<u8>), String>) -> String {
    match r {
        Ok((status, body)) => {
            let text = String::from_utf8_lossy(&body[..body.len().min(160)]).replace('\n', " ");
            format!("HTTP {status} {}", text.trim())
        }
        Err(e) => e.clone(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(s: &str) -> Option<Json> {
        let mut p = Parser { s: s.as_bytes(), i: 0, depth: 0 };
        let v = p.value()?;
        p.ws();
        (p.i == p.s.len()).then_some(v)
    }
    pub fn get(&self, key: &str) -> &Json {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map_or(&Json::Null, |(_, v)| v),
            _ => &Json::Null,
        }
    }
    pub fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            _ => "",
        }
    }
    pub fn u64(&self) -> u64 {
        match self {
            Json::Num(n) => n.parse().ok().or_else(|| n.parse::<f64>().ok().filter(|f| *f >= 0.0).map(|f| f as u64)).unwrap_or(0),
            _ => 0,
        }
    }
    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(a) => a,
            _ => &[],
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }
    fn eat(&mut self, lit: &str) -> bool {
        let ok = self.s[self.i..].starts_with(lit.as_bytes());
        if ok {
            self.i += lit.len();
        }
        ok
    }
    fn value(&mut self) -> Option<Json> {
        self.ws();
        self.depth += 1;
        let v = if self.depth > MAX_DEPTH { None } else { self.node() };
        self.depth -= 1;
        v
    }
    fn node(&mut self) -> Option<Json> {
        match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut kv = Vec::new();
                self.ws();
                if self.eat("}") {
                    return Some(Json::Obj(kv));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.eat(":").then_some(())?;
                    kv.push((k, self.value()?));
                    self.ws();
                    if self.eat("}") {
                        return Some(Json::Obj(kv));
                    }
                    self.eat(",").then_some(())?;
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Some(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    if self.eat("]") {
                        return Some(Json::Arr(a));
                    }
                    self.eat(",").then_some(())?;
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.eat("true").then_some(Json::Bool(true)),
            b'f' => self.eat("false").then_some(Json::Bool(false)),
            b'n' => self.eat("null").then_some(Json::Null),
            _ => {
                let start = self.i;
                while self.s.get(self.i).is_some_and(|c| matches!(c, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')) {
                    self.i += 1;
                }
                (self.i > start).then(|| Json::Num(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
        }
    }
    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }
    fn string(&mut self) -> Option<String> {
        self.eat("\"").then_some(())?;
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) && self.eat("\\u") {
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)
                            } else {
                                hi
                            };
                            char::from_u32(code).unwrap_or('\u{FFFD}')
                        }
                        e => e as char,
                    };
                    let mut b = [0; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                }
                c => out.push(c),
            }
        }
    }
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

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

fn unbase64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Friend {
    pub pid: u64,
    pub name: String,
    pub status: u32,
    pub app_id: u64,
    pub app_field: Vec<u8>,
    pub favorite: bool,
}

impl Friend {
    pub fn online(&self) -> bool {
        self.status > 0
    }
    pub fn in_mk8(&self) -> bool {
        self.online() && self.app_id == MK8
    }
}

pub fn parse_friends(body: &str) -> Option<Vec<Friend>> {
    let root = Json::parse(body)?;
    let list = root.get("friends");
    matches!(list, Json::Arr(_)).then_some(())?;
    Some(
        list.arr()
            .iter()
            .filter_map(|f| {
                let pres = f.get("presence");
                let friend = Friend {
                    pid: f.get("pid").u64(),
                    name: f.get("name").str().to_string(),
                    status: pres.get("status").u64() as u32,
                    app_id: u64::from_str_radix(pres.get("app_id").str(), 16).unwrap_or(0),
                    app_field: unbase64(pres.get("app_field").str()).unwrap_or_default(),
                    favorite: *f.get("favorite") == Json::Bool(true),
                };
                (friend.pid != 0).then_some(friend)
            })
            .collect(),
    )
}

#[derive(Default)]
struct Cache {
    list: Vec<Friend>,
    loaded: bool,
    tried: Option<Instant>,
    loading: bool,
}

static FRIENDS: Mutex<Cache> = Mutex::new(Cache { list: Vec::new(), loaded: false, tried: None, loading: false });
static FRIENDS_READY: Condvar = Condvar::new();

fn refresh_friends() {
    let reply = http("GET", "/api/friends", None);
    let fetched = match &reply {
        Ok((200, body)) => parse_friends(&String::from_utf8_lossy(body)),
        _ => None,
    };
    let mut c = FRIENDS.lock().unwrap();
    match fetched {
        Some(list) => {
            if !c.loaded || c.list.len() != list.len() {
                eprintln!("[nextendo] {} friend(s), {} in Mario Kart", list.len(), list.iter().filter(|f| f.in_mk8()).count());
            }
            c.list = list;
            c.loaded = true;
        }
        None if !c.loaded => eprintln!("[nextendo] could not read the friend list: {}", why(&reply)),
        None => {}
    }
    c.tried = Some(Instant::now());
    c.loading = false;
    FRIENDS_READY.notify_all();
}

pub fn friends(wait: Duration) -> Vec<Friend> {
    if !active() {
        return Vec::new();
    }
    let mut c = FRIENDS.lock().unwrap();
    let first = c.tried.is_none();
    if !c.loading && c.tried.is_none_or(|t| t.elapsed() >= FRIENDS_TTL) {
        c.loading = true;
        std::thread::spawn(refresh_friends);
    }
    if first && c.loading && !wait.is_zero() {
        c = FRIENDS_READY.wait_timeout_while(c, wait, |c| c.loading).unwrap().0;
    }
    c.list.clone()
}

struct Presence {
    app_field: Vec<u8>,
    detail: String,
    dirty: bool,
}

static PRESENCE: Mutex<Presence> = Mutex::new(Presence { app_field: Vec::new(), detail: String::new(), dirty: true });
static PRESENCE_CHANGED: Condvar = Condvar::new();

pub fn presence_body(app_field: &[u8], detail: &str) -> String {
    let end = app_field.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    format!(
        "{{\"status\":2,\"app_field\":{},\"app_id\":{},\"app_detail\":{}}}",
        json_str(&base64(&app_field[..end])),
        json_str(&format!("{MK8:016X}")),
        json_str(detail)
    )
}

fn publish_loop() {
    let mut last: Option<Instant> = None;
    let mut logged: Option<bool> = None;
    loop {
        {
            let p = PRESENCE.lock().unwrap();
            drop(PRESENCE_CHANGED.wait_timeout_while(p, PRESENCE_EVERY, |p| !p.dirty).unwrap());
        }
        if let Some(at) = last {
            std::thread::sleep(PRESENCE_MIN_GAP.saturating_sub(at.elapsed()));
        }
        let body = {
            let mut p = PRESENCE.lock().unwrap();
            p.dirty = false;
            presence_body(&p.app_field, &p.detail)
        };
        let reply = http("POST", "/api/presence", Some(&body));
        let ok = matches!(reply, Ok((200..=299, _)));
        if logged != Some(ok) {
            match ok {
                true => eprintln!("[nextendo] presence published"),
                false => eprintln!("[nextendo] could not publish presence: {}", why(&reply)),
            }
            logged = Some(ok);
        }
        last = Some(Instant::now());
    }
}

pub fn set_detail(detail: &str) {
    if !active() {
        return;
    }
    let detail: String = detail.trim().chars().take(64).collect();
    let mut p = PRESENCE.lock().unwrap();
    if p.detail == detail {
        return;
    }
    p.detail = detail;
    p.dirty = true;
    PRESENCE_CHANGED.notify_all();
}

pub fn set_presence(app_field: &[u8]) {
    if !active() {
        return;
    }
    let mut p = PRESENCE.lock().unwrap();
    p.app_field = app_field.to_vec();
    p.dirty = true;
    PRESENCE_CHANGED.notify_all();
}

pub fn history_seconds(body: &str) -> Option<u64> {
    let root = Json::parse(body)?;
    let list = root.get("history");
    matches!(list, Json::Arr(_)).then_some(())?;
    Some(list.arr().iter().find(|h| u64::from_str_radix(h.get("title_id").str(), 16).ok() == Some(MK8)).map_or(0, |h| h.get("seconds").u64()))
}

pub fn history_body(seconds: u64, last_played: &str) -> String {
    format!(
        "{{\"history\":[{{\"title_id\":{},\"name\":{},\"icon\":\"\",\"seconds\":{seconds},\"last_played\":{}}}]}}",
        json_str(&format!("{MK8:016x}")),
        json_str(MK8_NAME),
        json_str(last_played)
    )
}

pub fn iso_utc(unix: u64) -> String {
    let (days, secs) = ((unix / 86400) as i64, unix % 86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

fn history_loop() {
    let base = loop {
        match http("GET", "/api/history", None) {
            Ok((200, body)) => match history_seconds(&String::from_utf8_lossy(&body)) {
                Some(seconds) => break seconds,
                None => eprintln!("[nextendo] could not read play time: unexpected reply"),
            },
            reply => eprintln!("[nextendo] could not read play time: {}", why(&reply)),
        }
        std::thread::sleep(HISTORY_EVERY);
    };
    let mut played = Duration::ZERO;
    let mut tick = Instant::now();
    let mut pushed = Instant::now();
    let mut wait = HISTORY_FIRST.min(HISTORY_EVERY);
    let mut logged: Option<bool> = None;
    loop {
        std::thread::sleep(HISTORY_TICK);
        played += tick.elapsed().min(HISTORY_TICK * 2);
        tick = Instant::now();
        if pushed.elapsed() < wait {
            continue;
        }
        pushed = Instant::now();
        wait = HISTORY_EVERY;
        let body = history_body(base + played.as_secs(), &iso_utc(crate::services::nextrace::now()));
        let reply = send(&nextendo::token(), "PUT", "/api/history", Some(&body));
        if matches!(reply, Ok((401 | 403, _))) {
            eprintln!("[nextendo] Nextendo does not let Mush8 save play time: {}", why(&reply));
            return;
        }
        let ok = matches!(reply, Ok((200..=299, _)));
        if logged != Some(ok) {
            match ok {
                true => eprintln!("[nextendo] play time saved ({} min this session)", played.as_secs() / 60),
                false => eprintln!("[nextendo] could not save play time: {}", why(&reply)),
            }
            logged = Some(ok);
        }
    }
}

pub fn start() {
    if !active() {
        return;
    }
    std::thread::spawn(publish_loop);
    std::thread::spawn(history_loop);
    friends(Duration::ZERO);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_friend_list() {
        let body = r#"{"friends":[
            {"pid":1750000123,"name":"Luigi é\"x\"","favorite":true,"since":1700000000,
             "presence":{"status":2,"app_id":"0100152000022000","app_field":"AQID","jeu":"Mario Kart 8 Deluxe","plateforme":"Ryujinx Nextendo"}},
            {"pid":42,"name":"Peach","presence":{"status":0,"app_id":"","app_field":""}},
            {"pid":0,"name":"nobody"}
        ],"requests":[],"sent":[]}"#;
        let list = parse_friends(body).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], Friend { pid: 1750000123, name: "Luigi é\"x\"".into(), status: 2, app_id: MK8, app_field: vec![1, 2, 3], favorite: true });
        assert!(list[0].in_mk8());
        assert!(!list[1].online());
        assert!(parse_friends("{\"error\":\"unauthorized\"}").is_none());
    }

    #[test]
    fn json_handles_nesting_and_escapes() {
        let v = Json::parse(r#" { "a" : [1, -2.5e3, true, null, {"b":"😀\n"}] } "#).unwrap();
        assert_eq!(v.get("a").arr()[0].u64(), 1);
        assert_eq!(v.get("a").arr()[4].get("b").str(), "😀\n");
        assert!(Json::parse("{\"a\":1,}").is_none());
        assert!(Json::parse("[1] x").is_none());
        assert!(Json::parse(&format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH))).is_some());
        assert!(Json::parse(&format!("{}{}", "[".repeat(100_000), "]".repeat(100_000))).is_none());
    }

    #[test]
    fn presence_body_trims_the_app_field() {
        let mut field = vec![0u8; 0xC0];
        field[..3].copy_from_slice(b"Hi\0");
        field[3] = 7;
        assert_eq!(presence_body(&field, "Worldwide"), r#"{"status":2,"app_field":"SGkABw==","app_id":"0100152000022000","app_detail":"Worldwide"}"#);
        assert_eq!(presence_body(&[0; 0xC0], ""), r#"{"status":2,"app_field":"","app_id":"0100152000022000","app_detail":""}"#);
    }

    #[test]
    fn play_time_reads_and_writes_mk8() {
        let body = r#"{"history":[{"title_id":"01006a800016e000","seconds":50},{"title_id":"0100152000022000","name":"Mario Kart 8 Deluxe","seconds":4200,"last_played":"2026-11-07T22:00:00Z"}]}"#;
        assert_eq!(history_seconds(body), Some(4200));
        assert_eq!(history_seconds(r#"{"history":[]}"#), Some(0));
        assert_eq!(history_seconds(r#"{"error":"x"}"#), None);
        assert_eq!(history_body(4320, "2026-11-08T01:02:03Z"), r#"{"history":[{"title_id":"0100152000022000","name":"Mario Kart 8 Deluxe","icon":"","seconds":4320,"last_played":"2026-11-08T01:02:03Z"}]}"#);
    }

    #[test]
    fn iso_dates_are_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951782400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1794099723), "2026-11-08T01:02:03Z");
    }

    #[test]
    fn form_values_are_percent_encoded() {
        assert_eq!(form_value("nxc_PZv-XFn79sLo"), "nxc_PZv-XFn79sLo");
        assert_eq!(form_value("a+b/c=d e"), "a%2Bb%2Fc%3Dd%20e");
    }

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0x00]] {
            assert_eq!(unbase64(&base64(data)).unwrap(), data);
        }
    }
}
