// Mush8 - mod.rs
// Constantin Clerc (c22dev) - 0.1
mod account;
mod aoc;
pub mod set;
mod audio;
pub mod am;
mod binder;
mod bsd;
mod friend;
pub mod fs;
pub mod hid;
mod lan;
mod lanplay;
mod lm;
pub mod mii;
mod nfp;
pub mod nifm;
pub mod nextendo;
mod nextrace;
pub mod nexweb;
mod nsd;
mod pl;
mod sfdnsres;
mod ssl;
mod time;
pub mod vi;
mod wireless;
pub mod vnet;

use crate::ipc::{Act, Service, Stub, Table};
use std::sync::Arc;

pub fn open(name: &str) -> Arc<dyn Service> {
    match name {
        "lm" => Arc::new(lm::Lm),
        "fsp-srv" => Arc::new(fs::FspSrv),
        "hid" => Arc::new(hid::Hid),
        "pl:u" | "pl:s" => Arc::new(pl::Pl),
        "audren:u" => Arc::new(audio::AudRen),
        "mii:e" | "mii:u" => Arc::new(mii::MiiService),
        "acc:u0" | "acc:u1" => Arc::new(account::Account),
        "time:u" | "time:a" | "time:s" => Arc::new(time::TimeService),
        "vi:u" | "vi:s" | "vi:m" => vi::open(name),
        "appletOE" => am::open(),
        "nifm:u" | "nifm:a" | "nifm:s" => nifm::open(),
        "bsd:u" => bsd::open("bsd:u"),
        "csrng" => Arc::new(Csrng),
        "bsd:s" => bsd::open("bsd:s"),
        "friend:u" | "friend:v" | "friend:m" | "friend:s" | "friend:a" => friend::open(),
        "aoc:u" => Arc::new(aoc::Aoc),
        "sfdnsres" if nextendo::enabled() => Arc::new(sfdnsres::Resolver),
        "nsd:u" | "nsd:a" if nextendo::enabled() => Arc::new(nsd::Nsd),
        "ssl" if nextendo::enabled() => ssl::open(),
        "nfp:user" => nfp::open(),
        "set" => Arc::new(set::Settings),
        "pctl" | "pctl:s" | "pctl:a" | "pctl:r" => Arc::new(Table {
            name: "pctl",
            cmds: &[(0, Act::Obj(pctl_service)), (1, Act::Obj(pctl_service))],
        }),
        "apm" | "apm:p" => Arc::new(Table {
            name: "apm",
            cmds: &[(0, Act::Obj(|| Arc::new(Table { name: "apm::ISession", cmds: &[(0, Act::Ok), (1, Act::U32(0x0001_0001))] }))), (1, Act::U32With(|| vi::docked() as u32))],
        }),
        _ => Arc::new(Stub(name.to_string())),
    }
}

fn pctl_service() -> Arc<dyn Service> {
    Arc::new(Table {
        name: "pctl::IParentalControlService",
        cmds: &[
            (1, Act::Ok),
            (1001, Act::Ok),
            (1013, Act::Ok),
            (1018, Act::U32(1)),
            (1031, Act::U32(0)),
            (1032, Act::U32(0)),
            (1403, Act::U32(0)),
            (1432, Act::Event(false)),
            (1456, Act::Event(false)),
            (1473, Act::Event(false)),
        ],
    })
}

struct Csrng;

impl Service for Csrng {
    fn name(&self) -> String {
        "csrng".into()
    }
    fn call(&self, req: &crate::ipc::Request, rsp: &mut crate::ipc::Response) -> u32 {
        match req.cmd {
            0 => {
                if let Some(b) = req.out_buffer() {
                    let out = unsafe { std::slice::from_raw_parts_mut(b.addr as *mut u8, b.size as usize) };
                    for chunk in out.chunks_mut(256) {
                        unsafe { libc::getentropy(chunk.as_mut_ptr() as *mut _, chunk.len()) };
                    }
                }
                0
            }
            _ => Stub(self.name()).call(req, rsp),
        }
    }
}
