// Mush8 - account.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Act, Request, Response, Service, Stub, Table, write_buffer};
use crate::services::am::USER_ID;
use crate::services::nextendo;
use std::sync::Arc;

const NICKNAME: &str = "Player";
const NICKNAME_CHARS: usize = 10;
const NAS_COUNTRY: usize = 0x20;

pub struct Account;

fn profile_base(out: &mut Vec<u8>) {
    out.extend_from_slice(&USER_ID);
    out.extend_from_slice(&0u64.to_le_bytes());
    let mut n = [0u8; 0x20];
    let name = nickname();
    n[..name.len()].copy_from_slice(name.as_bytes());
    out.extend_from_slice(&n);
}

fn nickname() -> String {
    let set = std::env::var("NX_NICKNAME").unwrap_or_default();
    let set = set.trim();
    let name = if set.is_empty() { NICKNAME } else { set };
    let mut out = String::new();
    for c in name.chars().take(NICKNAME_CHARS) {
        if out.len() + c.len_utf8() > 0x1F {
            break;
        }
        out.push(c);
    }
    out
}

fn country() -> Option<[u8; 2]> {
    let c = std::env::var("NX_COUNTRY").ok()?.trim().to_ascii_uppercase();
    let c: [u8; 2] = c.as_bytes().try_into().ok()?;
    c.iter().all(u8::is_ascii_uppercase).then_some(c)
}

impl Service for Account {
    fn name(&self) -> String {
        "acc:u0".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.push_u32(1),
            1 => rsp.data.push((req.data.get(..16) == Some(&USER_ID)) as u8),
            2 | 3 | 131 | 141 => {
                if let Some(b) = req.out_buffer() {
                    let mut list = vec![0u8; b.size as usize];
                    list[..16].copy_from_slice(&USER_ID);
                    write_buffer(&b, &list);
                }
            }
            4 => rsp.data.extend_from_slice(&USER_ID),
            5 => rsp.objects.push(Arc::new(Profile)),
            50 => rsp.data.push(0),
            51 => rsp.data.extend_from_slice(&USER_ID),
            100 | 140 | 110 | 111 | 130 => {}
            101 => rsp.objects.push(Arc::new(ManagerForApplication)),
            150 => rsp.data.push(0),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct Profile;

impl Service for Profile {
    fn name(&self) -> String {
        "account::IProfile".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => {
                if let Some(b) = req.out_buffer() {
                    write_buffer(&b, &[0u8; 0x80]);
                }
                profile_base(&mut rsp.data);
            }
            1 => profile_base(&mut rsp.data),
            10 => rsp.push_u32(0),
            11 => rsp.push_u32(0),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

const NOT_LINKED: u32 = 124 | 341 << 9;

struct ManagerForApplication;

impl Service for ManagerForApplication {
    fn name(&self) -> String {
        "account::IManagerForApplication".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        if nextendo::enabled() && nextendo::pid() != 0 {
            return linked(req, rsp);
        }
        match req.cmd {
            0 | 2 | 3 | 130 | 170 => NOT_LINKED,
            1 => {
                rsp.push_u64(u64::from_le_bytes(USER_ID[..8].try_into().unwrap()));
                0
            }
            160 => 0,
            _ => Stub(self.name()).call(req, rsp),
        }
    }
}

const SUBSCRIBED: u32 = 1;

fn async_done() -> Arc<dyn Service> {
    Arc::new(Table {
        name: "account::IAsyncContext",
        cmds: &[(0, Act::Event(true)), (1, Act::Ok), (2, Act::Data(&[1])), (3, Act::Ok), (100, Act::U32(SUBSCRIBED))],
    })
}

fn linked(req: &Request, rsp: &mut Response) -> u32 {
    match req.cmd {
        0 | 160 => {}
        1 => rsp.push_u64(nextendo::pid()),
        2 | 170 => rsp.objects.push(async_done()),
        3 | 4 => {
            let token = nextendo::id_token();
            let token = token.as_bytes();
            if let Some(b) = req.out_buffer() {
                let mut out = vec![0u8; b.size as usize];
                let n = token.len().min(out.len());
                out[..n].copy_from_slice(&token[..n]);
                write_buffer(&b, &out);
            }
            rsp.push_u32(token.len() as u32);
        }
        130 => {
            let mut base = [0u8; 0x68];
            base[..10].copy_from_slice(b"2000-01-01");
            if let Some(c) = country() {
                base[NAS_COUNTRY..NAS_COUNTRY + 2].copy_from_slice(&c);
            }
            if let Some(c) = req.c.iter().find(|b| b.addr != 0 && b.size >= 0x68) {
                write_buffer(c, &base);
            }
            if let Some(b) = req.b.iter().find(|b| b.addr != 0) {
                write_buffer(b, &vec![0u8; b.size as usize]);
            }
            rsp.push_u64(nextendo::pid());
        }
        _ => return Stub("account::IManagerForApplication".into()).call(req, rsp),
    }
    0
}
