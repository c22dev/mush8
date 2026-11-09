// Mush8 - kernel.rs
// Constantin Clerc (c22dev) - 0.1
use crate::guest::{self, Entry, GuestThread};
use crate::mem::{self, AddressSpace, Block, perm, state};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering::*};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

pub mod rc {
    pub const OK: u32 = 0;
    pub const INVALID_SIZE: u32 = 0xCA01;
    pub const INVALID_ADDRESS: u32 = 0xCC01;
    pub const INVALID_HANDLE: u32 = 0xE401;
    pub const TIMED_OUT: u32 = 0xEA01;
    pub const CANCELLED: u32 = 0xEC01;
    pub const INVALID_ENUM: u32 = 0xF001;
    pub const NOT_FOUND: u32 = 0xF201;
    pub const INVALID_STATE: u32 = 0xFA01;
}

pub const CURRENT_PROCESS: u32 = 0xFFFF_8000;
pub const CURRENT_THREAD: u32 = 0xFFFF_8001;
const HAS_WAITERS: u32 = 0x4000_0000;

pub struct Thread {
    pub handle: u32,
    pub id: u64,
    pub entry: u64,
    pub arg: u64,
    pub stack_top: u64,
    pub tlr: u64,
    pub prio: AtomicI32,
    pub core: AtomicI32,
    pub affinity: AtomicU64,
    pub started: AtomicBool,
    pub exited: AtomicBool,
    pub cancel: AtomicBool,
    runnable: AtomicBool,
    ready_seq: AtomicU64,
    cv: Condvar,
}

impl Thread {
    fn sched_core(&self) -> i32 {
        self.core.load(Relaxed).max(0)
    }
}

#[derive(Default)]
pub struct Event {
    pub signaled: AtomicBool,
}

pub type SharedMemory = crate::shm::Shm;

#[derive(Clone)]
pub enum Obj {
    Process,
    Thread(Arc<Thread>),
    ReadableEvent(Arc<Event>),
    WritableEvent(Arc<Event>),
    Session(Arc<crate::ipc::Session>),
    ResourceLimit,
    SharedMemory(Arc<SharedMemory>),
    TransferMemory { addr: u64, size: u64 },
}

struct Waiter {
    thread: Option<Arc<Thread>>,
    tag: u32,
    prio: i32,
    mutex: u64,
    woken: AtomicBool,
}

#[derive(Default)]
pub struct State {
    handles: HashMap<u32, Obj>,
    next_handle: u32,
    next_thread_id: u64,
    mutex_waiters: HashMap<u64, Vec<Arc<Waiter>>>,
    cv_waiters: HashMap<u64, Vec<Arc<Waiter>>>,
    addr_waiters: HashMap<u64, Vec<Arc<Waiter>>>,
    tlr_next: u64,
    threads: Vec<Arc<Thread>>,
    next_seq: u64,
    sync_waiters: Vec<(Arc<Thread>, Vec<usize>)>,
}

pub struct Kernel {
    st: Mutex<State>,
    cv: Condvar,
    pub aspace: OnceLock<AddressSpace>,
}

pub static KERNEL: LazyLock<Kernel> = LazyLock::new(|| Kernel {
    st: Mutex::new(State { next_handle: 0x10, next_thread_id: 0x50, ..Default::default() }),
    cv: Condvar::new(),
    aspace: OnceLock::new(),
});

pub fn aspace() -> &'static AddressSpace {
    KERNEL.aspace.get().expect("address space not initialized")
}

fn atomic32(addr: u64) -> &'static AtomicU32 {
    unsafe { AtomicU32::from_ptr(addr as *mut u32) }
}

impl Kernel {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap()
    }

    fn wake_all(&self, g: &State) {
        for t in &g.threads {
            t.cv.notify_one();
        }
        self.cv.notify_all();
    }

    fn wake_core(&self, g: &State, core: i32) {
        for t in &g.threads {
            if t.runnable.load(Relaxed) && t.sched_core() == core {
                t.cv.notify_one();
            }
        }
    }

    fn wake(&self, w: &Waiter) {
        match &w.thread {
            Some(t) => t.cv.notify_one(),
            None => self.cv.notify_all(),
        }
    }

    fn wait_until<'a>(&self, g: MutexGuard<'a, State>, timeout_ns: i64, mut done: impl FnMut(&mut State) -> bool) -> (MutexGuard<'a, State>, bool) {
        let me = current_kthread(&g);
        let (mut g, ok) = self.wait_until_raw(g, timeout_ns, &mut done, me.as_deref());
        if let Some(t) = me {
            g.make_runnable(&t);
            self.wake_core(&g, t.sched_core());
            g = self.wait_scheduled(g, &t);
        }
        (g, ok)
    }

    fn wait_until_raw<'a>(
        &self,
        mut g: MutexGuard<'a, State>,
        timeout_ns: i64,
        done: &mut impl FnMut(&mut State) -> bool,
        me: Option<&Thread>,
    ) -> (MutexGuard<'a, State>, bool) {
        let deadline = (timeout_ns >= 0).then(|| Instant::now() + Duration::from_nanos(timeout_ns as u64));
        if done(&mut g) {
            return (g, true);
        }
        if let Some(t) = me {
            t.runnable.store(false, Relaxed);
            self.wake_core(&g, t.sched_core());
        }
        let cv = me.map_or(&self.cv, |t| &t.cv);
        loop {
            if done(&mut g) {
                return (g, true);
            }
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                return (g, false);
            }
            let wait = deadline.map_or(SAFETY_NET, |d| (d - now).min(SAFETY_NET));
            g = cv.wait_timeout(g, wait).unwrap().0;
        }
    }
}

const SAFETY_NET: Duration = Duration::from_millis(50);

fn sched_enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| std::env::var_os("NX_NO_SCHED").is_none())
}

impl State {
    fn make_runnable(&mut self, t: &Thread) {
        self.next_seq += 1;
        t.ready_seq.store(self.next_seq, Relaxed);
        t.runnable.store(true, Relaxed);
    }

    fn is_scheduled(&self, t: &Thread) -> bool {
        let core = t.sched_core();
        let key = |x: &Thread| (x.prio.load(Relaxed), x.ready_seq.load(Relaxed));
        !self.threads.iter().any(|o| o.runnable.load(Relaxed) && o.sched_core() == core && key(o) < key(t))
    }
}

impl Kernel {
    fn wait_scheduled<'a>(&self, mut g: MutexGuard<'a, State>, t: &Thread) -> MutexGuard<'a, State> {
        if !sched_enabled() {
            return g;
        }
        while !g.is_scheduled(t) {
            let r = t.cv.wait_timeout(g, SAFETY_NET).unwrap();
            g = r.0;
        }
        g
    }
}

thread_local! {
    static KTHREAD: std::cell::RefCell<Option<Arc<Thread>>> = const { std::cell::RefCell::new(None) };
}

fn current_kthread(g: &State) -> Option<Arc<Thread>> {
    if let Some(t) = KTHREAD.with(|k| k.borrow().clone()) {
        return Some(t);
    }
    let h = guest::try_current()?.handle;
    match g.handles.get(&h) {
        Some(Obj::Thread(t)) => {
            KTHREAD.with(|k| *k.borrow_mut() = Some(t.clone()));
            Some(t.clone())
        }
        _ => None,
    }
}

pub fn sched_point() {
    let g = KERNEL.lock();
    if let Some(t) = current_kthread(&g) {
        let _g = KERNEL.wait_scheduled(g, &t);
    }
}

pub fn blocking<R>(f: impl FnOnce() -> R) -> R {
    let me = {
        let g = KERNEL.lock();
        let me = current_kthread(&g);
        if let Some(t) = &me {
            t.runnable.store(false, Relaxed);
            KERNEL.wake_core(&g, t.sched_core());
        }
        me
    };
    let r = f();
    if let Some(t) = me {
        let mut g = KERNEL.lock();
        g.make_runnable(&t);
        KERNEL.wake_core(&g, t.sched_core());
        drop(KERNEL.wait_scheduled(g, &t));
    }
    r
}

fn backoff_cap() -> Duration {
    static CAP: LazyLock<Duration> = LazyLock::new(|| {
        let us = std::env::var("NX_YIELD_BACKOFF").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(250);
        Duration::from_micros(us)
    });
    *CAP
}

const BACKOFF_AFTER: Duration = Duration::from_millis(2);
const BACKOFF_MIN: Duration = Duration::from_micros(10);

fn backoff(waited: Duration) -> Option<Duration> {
    let cap = backoff_cap();
    (waited >= BACKOFF_AFTER && !cap.is_zero()).then(|| (waited / 8).clamp(BACKOFF_MIN, cap.max(BACKOFF_MIN)))
}

pub fn spin_wait(site: u64) {
    thread_local! {
        static SPIN: std::cell::Cell<(u64, Option<(Instant, Instant)>)> = const { std::cell::Cell::new((0, None)) };
    }
    let now = Instant::now();
    let (last_site, run) = SPIN.with(|s| s.get());
    let first = match run {
        Some((first, last)) if last_site == site && now - last < Duration::from_micros(100) => first,
        _ => now,
    };
    if let Some(d) = backoff(now - first) {
        blocking(|| std::thread::sleep(d));
    }
    SPIN.with(|s| s.set((site, Some((first, Instant::now())))));
}

pub fn yield_backoff(regs: Option<[u64; 10]>) {
    thread_local! {
        static RUN: std::cell::Cell<Option<(Instant, [u64; 10])>> = const { std::cell::Cell::new(None) };
    }
    let Some(regs) = regs else {
        RUN.with(|r| r.set(None));
        return;
    };
    let now = Instant::now();
    let first = match RUN.with(|r| r.get()) {
        Some((first, prev)) if prev == regs => first,
        _ => now,
    };
    RUN.with(|r| r.set(Some((first, regs))));
    if let Some(d) = backoff(now - first) {
        blocking(|| std::thread::sleep(d));
    }
}

pub fn yield_thread() {
    let mut g = KERNEL.lock();
    if let Some(t) = current_kthread(&g) {
        g.make_runnable(&t);
        KERNEL.wake_core(&g, t.sched_core());
        let _g = KERNEL.wait_scheduled(g, &t);
    }
}

impl State {
    pub fn add(&mut self, o: Obj) -> u32 {
        let h = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(h, o);
        h
    }

    pub fn get(&self, h: u32) -> Option<Obj> {
        match h {
            CURRENT_PROCESS => Some(Obj::Process),
            CURRENT_THREAD => self.handles.get(&guest::current().handle).cloned(),
            _ => self.handles.get(&h).cloned(),
        }
    }

    pub fn close(&mut self, h: u32) -> bool {
        self.handles.remove(&h).is_some()
    }

    fn signaled(&self, h: u32) -> Option<bool> {
        Some(match self.get(h)? {
            Obj::ReadableEvent(e) | Obj::WritableEvent(e) => e.signaled.load(Relaxed),
            Obj::Thread(t) => t.exited.load(Relaxed),
            Obj::Session(_) => false,
            _ => false,
        })
    }

    fn current_thread(&self) -> Arc<Thread> {
        match self.get(CURRENT_THREAD) {
            Some(Obj::Thread(t)) => t,
            _ => panic!("current thread has no kernel object"),
        }
    }

    fn alloc_tlr(&mut self) -> Result<u64> {
        let a = aspace();
        let base = a.base + mem::TLR_OFF;
        let off = self.tlr_next;
        self.tlr_next += 0x200;
        if off % mem::PAGE == 0 {
            a.set(base + off, mem::PAGE, Block { state: state::THREAD_LOCAL, perm: perm::RW, attr: 0 })?;
        }
        Ok(base + off)
    }

    fn unlock_mutex(&mut self, addr: u64) {
        let list = self.mutex_waiters.entry(addr).or_default();
        if list.is_empty() {
            atomic32(addr).store(0, Release);
            mlog(addr, "unlock(no waiters)", guest::current().handle, 0);
            return;
        }
        let i = best(list);
        let w = list.remove(i);
        let v = w.tag | if list.is_empty() { 0 } else { HAS_WAITERS };
        atomic32(addr).store(v, Release);
        w.woken.store(true, Relaxed);
        KERNEL.wake(&w);
        mlog(addr, "unlock(hand-off)", w.tag, v);
    }
}

fn best(list: &[Arc<Waiter>]) -> usize {
    let mut b = 0;
    for (i, w) in list.iter().enumerate() {
        if w.prio < list[b].prio {
            b = i;
        }
    }
    b
}

fn remove(map: &mut HashMap<u64, Vec<Arc<Waiter>>>, key: u64, w: &Arc<Waiter>) -> bool {
    if let Some(l) = map.get_mut(&key) {
        if let Some(i) = l.iter().position(|x| Arc::ptr_eq(x, w)) {
            l.remove(i);
            return true;
        }
    }
    false
}

pub fn create_thread(entry: u64, arg: u64, stack_top: u64, prio: i32, core: i32) -> Result<u32> {
    let mut g = KERNEL.lock();
    let tlr = g.alloc_tlr()?;
    let id = g.next_thread_id;
    g.next_thread_id += 1;
    let handle = g.next_handle;
    let t = Arc::new(Thread {
        handle,
        id,
        entry,
        arg,
        stack_top,
        tlr,
        prio: AtomicI32::new(prio),
        core: AtomicI32::new(core),
        affinity: AtomicU64::new(1 << core.clamp(0, 3)),
        started: AtomicBool::new(false),
        exited: AtomicBool::new(false),
        cancel: AtomicBool::new(false),
        runnable: AtomicBool::new(false),
        ready_seq: AtomicU64::new(0),
        cv: Condvar::new(),
    });
    let h = g.add(Obj::Thread(t));
    debug_assert_eq!(h, handle);
    if std::env::var_os("NX_TRACE_THREADS").is_some() {
        eprintln!("[thread] create {h:#x}: prio {prio} core {core} entry {}", crate::diag::symbolize(entry));
    }
    Ok(h)
}

pub fn thread(h: u32) -> Option<Arc<Thread>> {
    match KERNEL.lock().get(h)? {
        Obj::Thread(t) => Some(t),
        _ => None,
    }
}

pub fn start_thread(t: Arc<Thread>, args: [u64; 4]) -> std::io::Result<std::thread::JoinHandle<()>> {
    t.started.store(true, Relaxed);
    {
        let mut g = KERNEL.lock();
        g.threads.push(t.clone());
        g.make_runnable(&t);
        KERNEL.wake_core(&g, t.sched_core());
    }
    std::thread::Builder::new().name(format!("guest-{:#x}", t.id)).stack_size(1 << 20).spawn(move || {
        if std::env::var_os("NX_GUEST_QOS").is_none_or(|v| v != "0") {
            unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };
        }
        crate::diag::install_altstack();
        let mut ctx = GuestThread::new(t.handle, t.tlr);
        ctx.install();
        let entry = Entry { pc: t.entry, sp: t.stack_top & !15, x: args };
        drop(KERNEL.wait_scheduled(KERNEL.lock(), &t));
        unsafe { guest::nx_enter_guest(&entry, &mut *ctx) };
        let mut g = KERNEL.lock();
        t.exited.store(true, Relaxed);
        t.runnable.store(false, Relaxed);
        g.threads.retain(|x| !Arc::ptr_eq(x, &t));
        let id = Arc::as_ptr(&t) as usize;
        for (w, objs) in &g.sync_waiters {
            if objs.contains(&id) {
                w.cv.notify_one();
            }
        }
        KERNEL.wake_core(&g, t.sched_core());
    })
}

pub fn create_event() -> (u32, u32) {
    let e = Arc::new(Event::default());
    let mut g = KERNEL.lock();
    (g.add(Obj::WritableEvent(e.clone())), g.add(Obj::ReadableEvent(e)))
}

pub fn signal_event(e: &Event) {
    let g = KERNEL.lock();
    e.signaled.store(true, Relaxed);
    let id = std::ptr::from_ref(e) as usize;
    for (t, objs) in &g.sync_waiters {
        if objs.contains(&id) {
            t.cv.notify_one();
        }
    }
    KERNEL.cv.notify_all();
}

pub fn wait_synchronization(handles: &[u32], timeout_ns: i64) -> (u32, u32) {
    let mut g = KERNEL.lock();
    let mut objs = Vec::with_capacity(handles.len());
    for &h in handles {
        match g.get(h) {
            None => return (rc::INVALID_HANDLE, 0),
            Some(Obj::ReadableEvent(e) | Obj::WritableEvent(e)) => objs.push(Arc::as_ptr(&e) as usize),
            Some(Obj::Thread(t)) => objs.push(Arc::as_ptr(&t) as usize),
            Some(_) => {}
        }
    }
    let me = g.current_thread();
    let mut idx = None;
    let mut cancelled = false;
    g.sync_waiters.push((me.clone(), objs));
    let (mut g, ok) = KERNEL.wait_until(g, timeout_ns, |s| {
        if me.cancel.swap(false, Relaxed) {
            cancelled = true;
            return true;
        }
        idx = handles.iter().position(|&h| s.signaled(h) == Some(true));
        idx.is_some()
    });
    if let Some(i) = g.sync_waiters.iter().position(|(t, _)| Arc::ptr_eq(t, &me)) {
        g.sync_waiters.swap_remove(i);
    }
    if cancelled {
        (rc::CANCELLED, 0)
    } else if !ok {
        (rc::TIMED_OUT, 0)
    } else {
        (rc::OK, idx.unwrap() as u32)
    }
}

pub fn cancel_synchronization(t: &Thread) {
    let _g = KERNEL.lock();
    t.cancel.store(true, Relaxed);
    t.cv.notify_one();
}

static MUTEX_LOG: Mutex<Vec<(u64, &'static str, u32, u32)>> = Mutex::new(Vec::new());

fn mlog(addr: u64, op: &'static str, thread: u32, word: u32) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("NX_MUTEX_LOG").is_some()) {
        return;
    }
    let mut l = MUTEX_LOG.lock().unwrap();
    if l.len() > 20000 {
        l.drain(..10000);
    }
    l.push((addr, op, thread, word));
}

fn dump_mutex_log(addr: u64) {
    let l = MUTEX_LOG.lock().unwrap();
    let ev: Vec<_> = l.iter().filter(|e| e.0 == addr).collect();
    eprintln!("[kernel] history of mutex {addr:#x} (last {}):", ev.len().min(30));
    for (_, op, t, w) in ev.iter().rev().take(30).rev() {
        eprintln!("[kernel]   {op:24} thread {t:#x} word after {w:#x}");
    }
}

pub fn arbitrate_lock(owner: u32, addr: u64, tag: u32) -> u32 {
    let mut g = KERNEL.lock();
    if atomic32(addr).load(Acquire) != owner | HAS_WAITERS {
        return rc::OK;
    }
    let me = g.current_thread();
    if owner == tag {
        eprintln!("[kernel] thread {tag:#x} waits for mutex {addr:#x} that it owns");
        dump_mutex_log(addr);
        let fp: u64 = crate::svc::current_guest_fp();
        crate::diag::backtrace(fp);
    }
    mlog(addr, "ArbitrateLock(wait)", tag, atomic32(addr).load(Relaxed));
    let w = Arc::new(Waiter { thread: Some(me.clone()), tag, prio: me.prio.load(Relaxed), mutex: addr, woken: AtomicBool::new(false) });
    g.mutex_waiters.entry(addr).or_default().push(w.clone());
    let _ = KERNEL.wait_until(g, -1, |_| w.woken.load(Relaxed));
    mlog(addr, "ArbitrateLock(acquired)", tag, atomic32(addr).load(Relaxed));
    rc::OK
}

pub fn arbitrate_unlock(addr: u64) -> u32 {
    let mut g = KERNEL.lock();
    g.unlock_mutex(addr);
    rc::OK
}

pub fn wait_process_wide_key(mutex: u64, key: u64, tag: u32, timeout_ns: i64) -> u32 {
    let mut g = KERNEL.lock();
    let me = g.current_thread();
    mlog(mutex, "cv wait: release", tag, atomic32(mutex).load(Relaxed));
    g.unlock_mutex(mutex);
    let w = Arc::new(Waiter { thread: Some(me.clone()), tag, prio: me.prio.load(Relaxed), mutex, woken: AtomicBool::new(false) });
    g.cv_waiters.entry(key).or_default().push(w.clone());
    atomic32(key).store(1, Release);
    let (mut g, ok) = KERNEL.wait_until(g, timeout_ns, |_| w.woken.load(Relaxed));
    if ok {
        return rc::OK;
    }
    if remove(&mut g.cv_waiters, key, &w) {
        if g.cv_waiters.get(&key).is_none_or(|l| l.is_empty()) {
            atomic32(key).store(0, Release);
        }
        mlog(mutex, "cv wait: timed out", tag, atomic32(mutex).load(Relaxed));
        return rc::TIMED_OUT;
    }
    let _ = KERNEL.wait_until(g, -1, |_| w.woken.load(Relaxed));
    rc::OK
}

fn acquire_or_enqueue(g: &mut State, w: &Arc<Waiter>) -> bool {
    let m = atomic32(w.mutex);
    loop {
        let v = m.load(Acquire);
        if v == 0 {
            if m.compare_exchange(0, w.tag, AcqRel, Acquire).is_ok() {
                w.woken.store(true, Relaxed);
                KERNEL.wake(w);
                mlog(w.mutex, "cv reacquire(free)", w.tag, w.tag);
                return true;
            }
        } else if v & HAS_WAITERS != 0 || m.compare_exchange(v, v | HAS_WAITERS, AcqRel, Acquire).is_ok() {
            g.mutex_waiters.entry(w.mutex).or_default().push(w.clone());
            mlog(w.mutex, "cv reacquire(queued)", w.tag, m.load(Relaxed));
            return false;
        }
    }
}

pub fn signal_process_wide_key(key: u64, count: i32) -> u32 {
    let mut g = KERNEL.lock();
    let mut n = 0;
    while count <= 0 || n < count {
        let Some(list) = g.cv_waiters.get_mut(&key).filter(|l| !l.is_empty()) else { break };
        let w = list.remove(best(list));
        acquire_or_enqueue(&mut g, &w);
        n += 1;
    }
    if g.cv_waiters.get(&key).is_none_or(|l| l.is_empty()) {
        atomic32(key).store(0, Release);
    }
    rc::OK
}

pub fn wait_for_address(addr: u64, kind: u32, value: i32, timeout_ns: i64) -> u32 {
    let mut g = KERNEL.lock();
    let a = atomic32(addr);
    let cur = a.load(Acquire) as i32;
    match kind {
        0 if cur < value => {}
        1 if cur < value => {
            a.fetch_sub(1, AcqRel);
        }
        2 if cur == value => {}
        0..=2 => return rc::INVALID_STATE,
        _ => return rc::INVALID_ENUM,
    }
    if timeout_ns == 0 {
        return rc::TIMED_OUT;
    }
    let me = g.current_thread();
    let w = Arc::new(Waiter { thread: Some(me.clone()), tag: 0, prio: me.prio.load(Relaxed), mutex: 0, woken: AtomicBool::new(false) });
    g.addr_waiters.entry(addr).or_default().push(w.clone());
    let (mut g, ok) = KERNEL.wait_until(g, timeout_ns, |_| w.woken.load(Relaxed));
    if ok {
        rc::OK
    } else {
        remove(&mut g.addr_waiters, addr, &w);
        rc::TIMED_OUT
    }
}

pub fn signal_to_address(addr: u64, kind: u32, value: i32, count: i32) -> u32 {
    let mut g = KERNEL.lock();
    let a = atomic32(addr);
    let waiting = g.addr_waiters.get(&addr).map_or(0, |l| l.len()) as i32;
    match kind {
        0 => {}
        1 => {
            if a.compare_exchange(value as u32, value.wrapping_add(1) as u32, AcqRel, Acquire).is_err() {
                return rc::INVALID_STATE;
            }
        }
        2 => {
            let others = (waiting - 1).min(count.saturating_add(1));
            let new = if waiting == 0 {
                value.wrapping_add(1)
            } else if count <= 0 {
                value.wrapping_sub(2)
            } else if others == 0 {
                value.wrapping_add(1)
            } else if others <= count {
                value.wrapping_sub(1)
            } else {
                value
            };
            if a.compare_exchange(value as u32, new as u32, AcqRel, Acquire).is_err() {
                return rc::INVALID_STATE;
            }
        }
        _ => return rc::INVALID_ENUM,
    }
    let list = g.addr_waiters.entry(addr).or_default();
    let mut n = 0;
    while !list.is_empty() && (count <= 0 || n < count) {
        let w = list.remove(best(list));
        w.woken.store(true, Relaxed);
        KERNEL.wake(&w);
        n += 1;
    }
    rc::OK
}

pub fn notify_all() {
    let g = KERNEL.lock();
    KERNEL.wake_all(&g);
}

pub fn sleep(ns: i64) {
    if ns <= 0 {
        yield_thread();
        return;
    }
    let g = KERNEL.lock();
    let _ = KERNEL.wait_until(g, ns, |_| false);
}

pub fn describe_current_thread() {
    let h = crate::guest::try_current().map(|c| c.handle);
    if let Some(h) = h {
        if let Ok(st) = KERNEL.st.try_lock() {
            if let Some(Obj::Thread(t)) = st.handles.get(&h) {
                eprintln!("        guest thread {h:#x} (id {:#x}) prio {} core {} entry {}", t.id, t.prio.load(Relaxed), t.core.load(Relaxed), crate::diag::symbolize(t.entry));
            }
        }
    }
}
