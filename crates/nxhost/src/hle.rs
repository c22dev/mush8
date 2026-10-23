// Mush8 - hle.rs
// Constantin Clerc (c22dev) - 0.1
use std::collections::HashMap;
use std::ffi::{CStr, c_char};

fn cs(p: *const c_char) -> String {
    if p.is_null() { "(null)".into() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

fn abort_report(cond: *const c_char, func: *const c_char, file: *const c_char, line: i32, result: Option<u32>) -> ! {
    eprintln!("[abort] {} in {} at {}:{line}", cs(cond), cs(func), cs(file));
    if let Some(r) = result {
        eprintln!("[abort] result {r:#x} (module {}, description {})", r & 0x1FF, r >> 9);
    }
    let fp: u64;
    unsafe { std::arch::asm!("mov {}, x29", out(reg) fp) };
    crate::diag::backtrace(fp);
    crate::diag::report_and_exit("guest SDK abort");
}

extern "C" fn abort_with_result(cond: *const c_char, func: *const c_char, file: *const c_char, line: i32, r: *const u32) -> ! {
    abort_report(cond, func, file, line, (!r.is_null()).then(|| unsafe { *r }))
}

extern "C" fn abort_plain(cond: *const c_char, func: *const c_char, file: *const c_char, line: i32) -> ! {
    abort_report(cond, func, file, line, None)
}

pub fn table() -> HashMap<&'static str, u64> {
    let mut t: HashMap<&'static str, u64> = HashMap::new();
    t.insert("nvnBootstrapLoader", crate::nvn::bootstrap_loader as *const () as usize as u64);
    crate::resource_compat::register(&mut t);
    for (name, f) in [
        ("_ZN2nn4diag6detail10VAbortImplEPKcS3_S3_iPKNS_6ResultEPKNS_2os17UserExceptionInfoES3_St9__va_list", abort_with_result as *const () as usize),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_iPKNS_6ResultES3_z", abort_with_result as *const () as usize),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_iPKNS_6ResultEPKNS_2os17UserExceptionInfoES3_z", abort_with_result as *const () as usize),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_iS3_z", abort_plain as *const () as usize),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_i", abort_plain as *const () as usize),
    ] {
        t.insert(name, f as u64);
    }
    t
}
