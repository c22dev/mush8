// Mush8 - svc.rs
// Constantin Clerc (c22dev) - 0.1
use crate::guest::{self, Frame};
use crate::ipc;
use crate::kernel::{self, KERNEL, Obj, rc};
use crate::mem::{Block, perm, state};
use std::sync::atomic::Ordering::Relaxed;

pub const PROGRAM_ID: u64 = 0x0100_1520_0002_2000;
const MEMORY_SIZE: u64 = 0xCD50_0000;

fn cstr(addr: u64, max: usize) -> String {
    let s = unsafe { std::slice::from_raw_parts(addr as *const u8, max) };
    String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(max)]).into_owned()
}

fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("NX_TRACE_SVC").is_some())
}

thread_local! {
    static GUEST_FP: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub fn current_guest_fp() -> u64 {
    GUEST_FP.with(|c| c.get())
}

#[unsafe(no_mangle)]
pub extern "C" fn dispatch(f: &mut Frame) {
    GUEST_FP.with(|c| c.set(f.x[29]));
    kernel::sched_point();
    dispatch_inner(f);
    kernel::sched_point();
}

fn dispatch_inner(f: &mut Frame) {
    let n = f.x[16] as u32;
    if trace() {
        eprintln!("[svc] {n:#04x} x0={:#x} x1={:#x} x2={:#x} x3={:#x}", f.x[0], f.x[1], f.x[2], f.x[3]);
    }
    let x = f.x;
    let r: u32 = match n {
        0x01 => set_heap_size(f, x[1]),
        0x02 => {
            let a = kernel::aspace();
            let (_, _, cur) = a.map.lock().unwrap().query(x[0]);
            match a.set(x[0], x[1], Block { perm: x[2] as u32, ..cur }) {
                Ok(()) => rc::OK,
                Err(_) => rc::INVALID_ADDRESS,
            }
        }
        0x03 => rc::OK,
        0x04 => map_memory(x[0], x[1], x[2]),
        0x05 => unmap_memory(x[0], x[1], x[2]),
        0x06 => {
            let (base, size, b) = kernel::aspace().map.lock().unwrap().query(x[2]);
            let info = x[0] as *mut u64;
            unsafe {
                *info = base;
                *info.add(1) = size;
                *info.add(2) = b.state as u64 | (b.attr as u64) << 32;
                *info.add(3) = b.perm as u64;
                *info.add(4) = 0;
            }
            f.x[1] = 0;
            rc::OK
        }
        0x07 => {
            eprintln!("[svc] ExitProcess");
            std::process::exit(0);
        }
        0x08 => match kernel::create_thread(x[1], x[2], x[3], x[4] as i32, x[5] as i32) {
            Ok(h) => {
                f.x[1] = h as u64;
                rc::OK
            }
            Err(e) => {
                eprintln!("[svc] CreateThread: {e}");
                0xD201
            }
        },
        0x09 => match kernel::thread(x[0] as u32) {
            Some(t) => {
                let args = [t.arg, t.handle as u64, 0, 0];
                match kernel::start_thread(t, args) {
                    Ok(_) => rc::OK,
                    Err(_) => 0xD201,
                }
            }
            None => rc::INVALID_HANDLE,
        },
        0x0A => {
            let ctx = guest::current();
            unsafe { guest::nx_exit_to_host(ctx) }
        }
        0x0B => {
            kernel::sleep(x[0] as i64);
            rc::OK
        }
        0x0C => match kernel::thread(x[1] as u32) {
            Some(t) => {
                f.x[1] = t.prio.load(Relaxed) as u64;
                rc::OK
            }
            None => rc::INVALID_HANDLE,
        },
        0x0D => match kernel::thread(x[0] as u32) {
            Some(t) => {
                t.prio.store(x[1] as i32, Relaxed);
                kernel::notify_all();
                rc::OK
            }
            None => rc::INVALID_HANDLE,
        },
        0x0E => match kernel::thread(x[2] as u32) {
            Some(t) => {
                f.x[1] = t.core.load(Relaxed) as u64;
                f.x[2] = t.affinity.load(Relaxed);
                rc::OK
            }
            None => rc::INVALID_HANDLE,
        },
        0x0F => match kernel::thread(x[0] as u32) {
            Some(t) => {
                if x[1] as i32 >= 0 {
                    t.core.store(x[1] as i32, Relaxed);
                }
                t.affinity.store(x[2], Relaxed);
                kernel::notify_all();
                rc::OK
            }
            None => rc::INVALID_HANDLE,
        },
        0x10 => {
            f.x[0] = 0;
            return;
        }
        0x11 | 0x12 | 0x17 => {
            let obj = KERNEL.lock().get(x[0] as u32);
            match obj {
                Some(Obj::WritableEvent(e)) | Some(Obj::ReadableEvent(e)) => {
                    if n == 0x11 {
                        kernel::signal_event(&e);
                        rc::OK
                    } else {
                        let was = e.signaled.swap(false, Relaxed);
                        if n == 0x17 && !was { rc::INVALID_STATE } else { rc::OK }
                    }
                }
                _ => rc::INVALID_HANDLE,
            }
        }
        0x13 => map_shared_memory(x[0] as u32, x[1], x[2]),
        0x14 => rc::OK,
        0x15 => {
            let h = KERNEL.lock().add(Obj::TransferMemory { addr: x[1], size: x[2] });
            f.x[1] = h as u64;
            rc::OK
        }
        0x16 => {
            if KERNEL.lock().close(x[0] as u32) { rc::OK } else { rc::INVALID_HANDLE }
        }
        0x18 => {
            let handles = unsafe { std::slice::from_raw_parts(x[1] as *const u32, x[2] as usize) }.to_vec();
            let (r, idx) = kernel::wait_synchronization(&handles, x[3] as i64);
            f.x[1] = idx as u64;
            r
        }
        0x19 => match kernel::thread(x[0] as u32) {
            Some(t) => {
                kernel::cancel_synchronization(&t);
                rc::OK
            }
            None => rc::INVALID_HANDLE,
        },
        0x1A => kernel::arbitrate_lock(x[0] as u32, x[1], x[2] as u32),
        0x1B => kernel::arbitrate_unlock(x[0]),
        0x1C => kernel::wait_process_wide_key(x[0], x[1], x[2] as u32, x[3] as i64),
        0x1D => kernel::signal_process_wide_key(x[0], x[1] as i32),
        0x1E => {
            f.x[0] = system_tick();
            return;
        }
        0x1F => {
            let name = cstr(x[1], 12);
            if name == "sm:" {
                f.x[1] = ipc::new_session_handle(std::sync::Arc::new(ipc::ServiceManager)) as u64;
                rc::OK
            } else {
                eprintln!("[svc] ConnectToNamedPort {name:?}: not found");
                rc::NOT_FOUND
            }
        }
        0x21 => send_sync_request(x[0] as u32, guest::current().tpidrro, 0x100),
        0x22 => send_sync_request(x[2] as u32, x[0], x[1] as usize),
        0x24 => {
            f.x[1] = 0x51;
            rc::OK
        }
        0x25 => match kernel::thread(x[1] as u32) {
            Some(t) => {
                f.x[1] = t.id;
                rc::OK
            }
            None => rc::INVALID_HANDLE,
        },
        0x26 => {
            let caller = unsafe { *((f.sp + 8) as *const u64) };
            eprintln!("[svc] Break(reason={:#x}, info={:#x}, size={:#x}) from {}", x[0], x[1], x[2], crate::diag::symbolize(caller));
            crate::diag::backtrace(x[29]);
            crate::diag::report_and_exit("guest called svcBreak");
        }
        0x27 => {
            let s = unsafe { std::slice::from_raw_parts(x[0] as *const u8, x[1] as usize) };
            eprint!("[guest] {}", String::from_utf8_lossy(s));
            rc::OK
        }
        0x29 => get_info(f, x[1] as u32, x[2] as u32, x[3]),
        0x2A | 0x2B => rc::OK,
        0x2C => {
            match kernel::aspace().set(x[0], x[1], Block { state: state::ALIAS, perm: perm::RW, attr: 0 }) {
                Ok(()) => rc::OK,
                Err(_) => rc::INVALID_ADDRESS,
            }
        }
        0x2D => match kernel::aspace().discard(x[0], x[1]) {
            Ok(()) => rc::OK,
            Err(_) => rc::INVALID_ADDRESS,
        },
        0x30 | 0x31 => {
            f.x[1] = match (n, x[2]) {
                (0x30, 0) => MEMORY_SIZE,
                (0x30, 1) => 256,
                (0x30, 2) => 1000,
                (0x30, 3) => 200,
                (0x30, 4) => 900,
                (_, 0) => *HEAP_SIZE.lock().unwrap(),
                _ => 0,
            };
            rc::OK
        }
        0x32 => rc::OK,
        0x34 => kernel::wait_for_address(x[0], x[1] as u32, x[2] as i32, x[3] as i64),
        0x35 => kernel::signal_to_address(x[0], x[1] as u32, x[2] as i32, x[3] as i32),
        0x45 => {
            let (w, r) = kernel::create_event();
            f.x[1] = w as u64;
            f.x[2] = r as u64;
            rc::OK
        }
        _ => {
            let caller = unsafe { *((f.sp + 8) as *const u64) };
            eprintln!("[svc] UNIMPLEMENTED svc {n:#04x} x0={:#x} x1={:#x} x2={:#x} from {}", x[0], x[1], x[2], crate::diag::symbolize(caller));
            0xF001
        }
    };
    f.x[0] = r as u64;
}

pub fn system_tick() -> u64 {
    let (v, freq): (u64, u64);
    unsafe {
        std::arch::asm!("mrs {}, cntvct_el0", out(reg) v);
        std::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
    }
    ((v as u128 * nxabi::GUEST_TICK_HZ as u128) / freq as u128) as u64
}

static HEAP_SIZE: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

fn set_heap_size(f: &mut Frame, size: u64) -> u32 {
    let a = kernel::aspace();
    if size % 0x20_0000 != 0 || size > a.layout.heap_size {
        return rc::INVALID_SIZE;
    }
    let mut cur = HEAP_SIZE.lock().unwrap();
    let r = if size >= *cur {
        a.set(a.heap() + *cur, size - *cur, Block { state: state::NORMAL, perm: perm::RW, attr: 0 })
    } else {
        a.discard(a.heap() + size, *cur - size)
    };
    if let Err(e) = r {
        eprintln!("[svc] SetHeapSize({size:#x}): {e}");
        return 0xD001;
    }
    if size > *cur {
        crate::gfx::wrap_heap(a.heap(), size);
    }
    *cur = size;
    f.x[1] = a.heap();
    rc::OK
}

fn map_memory(dst: u64, src: u64, size: u64) -> u32 {
    let a = kernel::aspace();
    let st = if (a.stack()..a.stack() + a.layout.stack_size).contains(&dst) { state::STACK } else { state::ALIAS };
    if a.set(dst, size, Block { state: st, perm: perm::RW, attr: 0 }).is_err() {
        return rc::INVALID_ADDRESS;
    }
    unsafe { std::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size as usize) };
    a.map.lock().unwrap().set(src, src + size, Block { state: state::NORMAL, perm: perm::NONE, attr: 1 });
    rc::OK
}

fn unmap_memory(dst: u64, src: u64, size: u64) -> u32 {
    let a = kernel::aspace();
    a.map.lock().unwrap().set(src, src + size, Block { state: state::NORMAL, perm: perm::RW, attr: 0 });
    unsafe { std::ptr::copy_nonoverlapping(dst as *const u8, src as *mut u8, size as usize) };
    let _ = a.discard(dst, size);
    rc::OK
}

fn map_shared_memory(h: u32, addr: u64, size: u64) -> u32 {
    let obj = KERNEL.lock().get(h);
    let Some(Obj::SharedMemory(sm)) = obj else { return rc::INVALID_HANDLE };
    let r = unsafe {
        libc::mmap(addr as *mut _, size as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED | libc::MAP_FIXED, sm.fd, 0)
    };
    if r as u64 != addr {
        return rc::INVALID_ADDRESS;
    }
    kernel::aspace().map.lock().unwrap().set(addr, addr + size, Block { state: state::SHARED, perm: perm::RW, attr: 0 });
    rc::OK
}

fn send_sync_request(h: u32, buf: u64, size: usize) -> u32 {
    let obj = KERNEL.lock().get(h);
    let Some(Obj::Session(s)) = obj else {
        eprintln!("[svc] SendSyncRequest on non-session handle {h:#x}");
        return rc::INVALID_HANDLE;
    };
    let b = unsafe { std::slice::from_raw_parts_mut(buf as *mut u8, size) };
    ipc::handle(&s, h, b);
    rc::OK
}

fn get_info(f: &mut Frame, id0: u32, handle: u32, id1: u64) -> u32 {
    let a = kernel::aspace();
    let v = match id0 {
        0 => 0xF,
        1 => 0xFFFF_FFFF_FFFF_FFFF,
        2 => a.alias(),
        3 => a.layout.alias_size,
        4 => a.heap(),
        5 => a.layout.heap_size,
        6 | 21 => MEMORY_SIZE,
        7 | 22 => *HEAP_SIZE.lock().unwrap() + 0x0400_0000,
        8 => 0,
        9 => KERNEL.lock().add(Obj::ResourceLimit) as u64,
        10 => 0,
        11 => {
            let mut b = [0u8; 8];
            let _ = getrandom(&mut b);
            u64::from_le_bytes(b) ^ id1
        }
        12 => a.base,
        13 => a.layout.total(),
        14 => a.stack(),
        15 => a.layout.stack_size,
        16 | 17 => 0,
        18 => PROGRAM_ID,
        20 => 0,
        23 => 1,
        24 => 200,
        25 | 0xF000_0002 => 0,
        26..=28 => 0,
        _ => {
            eprintln!("[svc] GetInfo unknown id {id0} (handle {handle:#x}, sub {id1})");
            return rc::INVALID_ENUM;
        }
    };
    f.x[1] = v;
    rc::OK
}

fn getrandom(b: &mut [u8]) -> bool {
    unsafe { libc::getentropy(b.as_mut_ptr() as *mut _, b.len()) == 0 }
}
