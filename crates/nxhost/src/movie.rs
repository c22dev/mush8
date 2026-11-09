// Mush8 - movie.rs
// Constantin Clerc (c22dev) - 0.1
mod video;
pub mod webm;

use crate::stubs::{self, Args};
use std::collections::{HashMap, VecDeque};
use std::ffi::{CString, c_char};
use std::sync::{Mutex, OnceLock};

const OK: u64 = 0;
const UNKNOWN_ERROR: u64 = 0x1000;
const OUTPUT_NOT_AVAILABLE: u64 = 0x100e;
const INPUT_NOT_AVAILABLE: u64 = 0x1011;
const END_OF_STREAM: u64 = 0x2000;
const TRACK_NOT_FOUND: u64 = 0x3003;
const FLAG_END_OF_STREAM: u32 = 4;

struct Guest {
    set_string: u64,
    set_int32: u64,
    set_int64: u64,
    set_rect: u64,
    find_int32: u64,
    find_int64: u64,
    signal_event: u64,
    operator_delete: u64,
}

static GUEST: OnceLock<Guest> = OnceLock::new();

pub fn init(mods: &[crate::loader::Module]) {
    let s = |n: &str| crate::loader::symbol(mods, n).unwrap_or_else(|| panic!("movie: guest symbol {n} not found"));
    let _ = GUEST.set(Guest {
        set_string: s("_ZN5movie9MediaData9SetStringEPKcS2_"),
        set_int32: s("_ZN5movie9MediaData8SetInt32EPKci"),
        set_int64: s("_ZN5movie9MediaData8SetInt64EPKcl"),
        set_rect: s("_ZN5movie9MediaData7SetRectEPKciiii"),
        find_int32: s("_ZNK5movie9MediaData9FindInt32EPKcPi"),
        find_int64: s("_ZNK5movie9MediaData9FindInt64EPKcPl"),
        signal_event: s("_ZN2nn2os11SignalEventEPNS0_9EventTypeE"),
        operator_delete: s("_ZdlPv"),
    });
}

fn g() -> &'static Guest {
    GUEST.get().expect("movie::init not called")
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn md_set_string(md: u64, k: &str, v: &str) {
    let f: extern "C" fn(u64, *const c_char, *const c_char) = unsafe { std::mem::transmute(g().set_string as usize) };
    let (k, v) = (cstr(k), cstr(v));
    f(md, k.as_ptr(), v.as_ptr());
}
fn md_set_i32(md: u64, k: &str, v: i32) {
    let f: extern "C" fn(u64, *const c_char, i32) = unsafe { std::mem::transmute(g().set_int32 as usize) };
    let k = cstr(k);
    f(md, k.as_ptr(), v);
}
fn md_set_i64(md: u64, k: &str, v: i64) {
    let f: extern "C" fn(u64, *const c_char, i64) = unsafe { std::mem::transmute(g().set_int64 as usize) };
    let k = cstr(k);
    f(md, k.as_ptr(), v);
}
fn md_set_rect(md: u64, k: &str, r: [i32; 4]) {
    let f: extern "C" fn(u64, *const c_char, i32, i32, i32, i32) = unsafe { std::mem::transmute(g().set_rect as usize) };
    let k = cstr(k);
    f(md, k.as_ptr(), r[0], r[1], r[2], r[3]);
}
fn md_find_i32(md: u64, k: &str) -> Option<i32> {
    let f: extern "C" fn(u64, *const c_char, *mut i32) -> bool = unsafe { std::mem::transmute(g().find_int32 as usize) };
    let k = cstr(k);
    let mut v = 0;
    f(md, k.as_ptr(), &mut v).then_some(v)
}
fn md_find_i64(md: u64, k: &str) -> Option<i64> {
    let f: extern "C" fn(u64, *const c_char, *mut i64) -> bool = unsafe { std::mem::transmute(g().find_int64 as usize) };
    let k = cstr(k);
    let mut v = 0;
    f(md, k.as_ptr(), &mut v).then_some(v)
}
fn signal(event: u64) {
    if event != 0 {
        let f: extern "C" fn(u64) = unsafe { std::mem::transmute(g().signal_event as usize) };
        f(event);
    }
}

fn buffer_set(buf: u64, base: *mut u8, capacity: usize, size: usize) {
    unsafe {
        *(buf as *mut i32) = 0;
        *((buf + 4) as *mut i32) = size as i32;
        *((buf + 0x18) as *mut u64) = capacity as u64;
        *((buf + 0x28) as *mut u64) = base as u64;
    }
}

fn wr<T: Copy>(addr: u64, v: T) {
    if addr != 0 {
        unsafe { (addr as *mut T).write_unaligned(v) }
    }
}

const TRACK_KEY: &str = "nx-host-track";

struct Track {
    id: u32,
    video: bool,
    mime: &'static str,
    width: u32,
    height: u32,
    sample_rate: u32,
    channels: u32,
    timescale: u32,
    samples: u32,
    duration_us: i64,
    next: u32,
    selected: bool,
}

enum Samples {
    File(std::io::BufReader<std::fs::File>),
    Memory(std::io::Cursor<Vec<u8>>),
}

impl std::io::Read for Samples {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Samples::File(f) => f.read(b),
            Samples::Memory(m) => m.read(b),
        }
    }
}

impl std::io::Seek for Samples {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Samples::File(f) => f.seek(to),
            Samples::Memory(m) => m.seek(to),
        }
    }
}

struct Extractor {
    reader: Option<mp4::Mp4Reader<Samples>>,
    webm: Option<std::sync::Arc<webm::Movie>>,
    tracks: Vec<Track>,
    host_file: Option<std::path::PathBuf>,
}

struct Decoder {
    video: bool,
    events: u64,
    track: Option<(u64, usize)>,
    width: u32,
    height: u32,
    channels: u32,
    sample_rate: u32,
    frames: Option<video::FrameSource>,
    inputs: Vec<Vec<u8>>,
    outputs: Vec<Vec<u8>>,
    free_outputs: Vec<usize>,
    ready: VecDeque<(usize, usize, i64, u32)>,
    format_sent: bool,
    eos: bool,
}

#[derive(Default)]
struct Objects {
    extractors: HashMap<u64, Extractor>,
    decoders: HashMap<u64, Decoder>,
}

static OBJ: OnceLock<Mutex<Objects>> = OnceLock::new();

fn obj() -> std::sync::MutexGuard<'static, Objects> {
    OBJ.get_or_init(Default::default).lock().unwrap()
}

const DECODER_SLOTS: usize = 19;
const EXTRACTOR_SLOTS: usize = 16;
static VTABLES: OnceLock<(u64, u64)> = OnceLock::new();

pub fn overrides() -> Vec<(&'static str, u64)> {
    let dec: Vec<u64> = (0..DECODER_SLOTS as u64).map(|i| stubs::make(decoder_call, i)).collect();
    let ext: Vec<u64> = (0..EXTRACTOR_SLOTS as u64).map(|i| stubs::make(extractor_call, i)).collect();
    let _ = VTABLES.set((Box::leak(dec.into_boxed_slice()).as_ptr() as u64, Box::leak(ext.into_boxed_slice()).as_ptr() as u64));
    vec![
        ("_ZN5movie7DecoderC1ENS_11DecoderTypeENS_19DecoderOutputFormatENS_11DecoderModeE", stubs::make(decoder_new, 0)),
        ("_ZN5movie9ExtractorC1ENS_13ContainerTypeE", stubs::make(extractor_new, 0)),
    ]
}

fn trace() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| std::env::var_os("NX_TRACE_MOVIE").is_some())
}

fn trace_n() -> bool {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    trace() && N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 300
}

fn extractor_new(_: u64, a: &Args) -> u64 {
    let this = a.0[0];
    wr(this, VTABLES.get().unwrap().1);
    obj().extractors.insert(this, Extractor { reader: None, webm: None, tracks: Vec::new(), host_file: None });
    if trace() {
        eprintln!("[movie] Extractor({this:#x}, container {})", a.0[1]);
    }
    this
}

fn read_game_file(path: &str) -> Option<Vec<u8>> {
    static FILES: OnceLock<Option<(nxfs::romfs::RomFs, Vec<nxfs::romfs::File>)>> = OnceLock::new();
    let (romfs, files) = FILES
        .get_or_init(|| {
            let cfg = crate::services::fs::CONFIG.get()?;
            let r = nxfs::romfs::RomFs::open(cfg.romfs.clone()).ok()?;
            let files = r.files();
            Some((r, files))
        })
        .as_ref()?;
    let rel = path.split_once(":/").map_or(path, |(_, p)| p).trim_start_matches('/');
    let f = files.iter().find(|f| f.path.trim_start_matches('/').eq_ignore_ascii_case(rel))?;
    romfs.storage().read_vec(f.offset, f.size as usize).ok()
}

fn open(e: &mut Extractor, name: &str, data: Vec<u8>) -> u64 {
    if !webm::is_webm(&data) {
        return open_mp4(e, name, data);
    }
    let Some(m) = webm::Movie::parse(data) else {
        eprintln!("[movie] {name}: unreadable WebM");
        return UNKNOWN_ERROR;
    };
    e.tracks.push(Track {
        id: 1,
        video: true,
        mime: "video/avc",
        width: m.width,
        height: m.height,
        sample_rate: 0,
        channels: 0,
        timescale: 1_000_000,
        samples: m.frames.len() as u32,
        duration_us: m.duration_us,
        next: 1,
        selected: false,
    });
    e.webm = Some(m);
    OK
}

fn open_mp4(e: &mut Extractor, name: &str, data: Vec<u8>) -> u64 {
    let dir = std::path::Path::new("work/movie_cache");
    let _ = std::fs::create_dir_all(dir);
    let base = std::path::Path::new(name).file_name().unwrap_or_default();
    let file = dir.join(base);
    let size = data.len() as u64;
    if std::fs::metadata(&file).map(|m| m.len()).ok() != Some(size) {
        let mut tmp = base.to_owned();
        tmp.push(".tmp");
        let tmp = dir.join(tmp);
        if std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &file)).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    let file = std::fs::canonicalize(&file).unwrap_or(file);
    let cached = std::fs::File::open(&file).ok().filter(|f| f.metadata().is_ok_and(|m| m.len() == size));
    let samples = match cached {
        Some(f) => {
            drop(data);
            e.host_file = Some(file);
            Samples::File(std::io::BufReader::new(f))
        }
        None => {
            eprintln!("[movie] {name}: cache file unusable, reading from memory");
            Samples::Memory(std::io::Cursor::new(data))
        }
    };
    let reader = match mp4::Mp4Reader::read_header(samples, size) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("[movie] {name}: {err}");
            return UNKNOWN_ERROR;
        }
    };
    let mut ids: Vec<u32> = reader.tracks().keys().copied().collect();
    ids.sort();
    for id in ids {
        let t = &reader.tracks()[&id];
        let (video, mime) = match t.media_type() {
            Ok(mp4::MediaType::H264) => (true, "video/avc"),
            Ok(mp4::MediaType::AAC) => (false, "audio/mp4a-latm"),
            _ => continue,
        };
        e.tracks.push(Track {
            id,
            video,
            mime,
            width: t.width() as u32,
            height: t.height() as u32,
            sample_rate: t.sample_freq_index().map_or(48000, |f| f.freq()),
            channels: t.channel_config().map_or(2, |c| c as u32),
            timescale: t.timescale(),
            samples: t.sample_count(),
            duration_us: t.duration().as_micros() as i64,
            next: 1,
            selected: false,
        });
    }
    if trace() {
        for t in &e.tracks {
            eprintln!("[movie] {name}: track {} {} {}x{} {} Hz {} ch, {} samples", t.id, t.mime, t.width, t.height, t.sample_rate, t.channels, t.samples);
        }
    }
    e.reader = Some(reader);
    OK
}

fn current(e: &mut Extractor) -> Option<(usize, mp4::Mp4Sample)> {
    if let Some(m) = &e.webm {
        let t = e.tracks.first().filter(|t| t.selected && t.next <= t.samples)?;
        let f = &m.frames[t.next as usize - 1];
        let bytes = mp4::Bytes::copy_from_slice(&m.data[f.range.clone()]);
        return Some((0, mp4::Mp4Sample { start_time: f.us as u64, duration: 0, rendering_offset: 0, is_sync: f.key, bytes }));
    }
    let reader = e.reader.as_mut()?;
    let mut best: Option<(usize, mp4::Mp4Sample, f64)> = None;
    for (i, t) in e.tracks.iter().enumerate() {
        if !t.selected || t.next > t.samples {
            continue;
        }
        if let Ok(Some(s)) = reader.read_sample(t.id, t.next) {
            let time = s.start_time as f64 / t.timescale.max(1) as f64;
            if best.as_ref().is_none_or(|b| time < b.2) {
                best = Some((i, s, time));
            }
        }
    }
    best.map(|(i, s, _)| (i, s))
}

fn extractor_call(slot: u64, a: &Args) -> u64 {
    let this = a.0[0];
    let mut source = (slot == 2).then(|| {
        let path = unsafe { std::ffi::CStr::from_ptr(a.0[1] as *const c_char) }.to_string_lossy().into_owned();
        let data = crate::kernel::blocking(|| read_game_file(&path));
        (path, data)
    });
    let mut o = obj();
    let r = (|| {
        let e = o.extractors.get_mut(&this)?;
        Some(match slot {
            0 | 1 => {
                o.extractors.remove(&this);
                if slot == 1 {
                    drop(o);
                    let del: extern "C" fn(u64) = unsafe { std::mem::transmute(g().operator_delete as usize) };
                    del(this);
                    return Some(OK);
                }
                OK
            }
            2 => {
                let (path, data) = source.take()?;
                match data {
                    Some(data) => open(e, &path, data),
                    None => {
                        eprintln!("[movie] SetDataSource({path}): file not found");
                        UNKNOWN_ERROR
                    }
                }
            }
            3 => {
                let data = unsafe { std::slice::from_raw_parts(a.0[1] as *const u8, a.0[2] as u32 as usize) }.to_vec();
                open(e, &format!("memory-{:x}.mp4", a.0[1]), data)
            }
            4 => {
                wr(a.0[1], e.tracks.len() as i32);
                OK
            }
            5 => {
                let Some(t) = e.tracks.get(a.0[1] as usize) else { return Some(TRACK_NOT_FOUND) };
                let md = a.0[2];
                md_set_string(md, "mime", t.mime);
                md_set_i64(md, "durationUs", t.duration_us);
                if t.video {
                    md_set_i32(md, "width", t.width as i32);
                    md_set_i32(md, "height", t.height as i32);
                } else {
                    md_set_i32(md, "sample-rate", t.sample_rate as i32);
                    md_set_i32(md, "channel-count", t.channels as i32);
                }
                md_set_i64(md, TRACK_KEY, (this << 8 | a.0[1] & 0xFF) as i64);
                OK
            }
            6 | 7 => {
                let Some(t) = e.tracks.get_mut(a.0[1] as usize) else { return Some(TRACK_NOT_FOUND) };
                t.selected = slot == 6;
                OK
            }
            8 => {
                let us = a.0[1] as i64;
                for t in &mut e.tracks {
                    let target = (us as f64 * t.timescale as f64 / 1e6) as u64;
                    let mut n = 1;
                    if let Some(m) = &e.webm {
                        n = m.frames.iter().filter(|f| f.us <= us).count().max(1) as u32;
                    }
                    if let Some(r) = e.reader.as_mut() {
                        while n < t.samples {
                            match r.read_sample(t.id, n + 1) {
                                Ok(Some(s)) if s.start_time <= target => n += 1,
                                _ => break,
                            }
                        }
                    }
                    t.next = n;
                }
                video::seek_all(this, us);
                OK
            }
            9 => match current(e) {
                Some((i, _)) => {
                    wr(a.0[1], i as u64);
                    OK
                }
                None => END_OF_STREAM,
            },
            10 => match current(e) {
                Some((_, s)) => {
                    let buf = a.0[1];
                    let (cap, base) = unsafe { (*((buf + 0x18) as *const u64), *((buf + 0x28) as *const u64)) };
                    let n = s.bytes.len().min(cap as usize);
                    unsafe { std::ptr::copy_nonoverlapping(s.bytes.as_ptr(), base as *mut u8, n) };
                    buffer_set(buf, base as *mut u8, cap as usize, n);
                    OK
                }
                None => END_OF_STREAM,
            },
            11 => match current(e) {
                Some((i, _)) => {
                    e.tracks[i].next += 1;
                    OK
                }
                None => END_OF_STREAM,
            },
            12 => match current(e) {
                Some((i, s)) => {
                    let t = &e.tracks[i];
                    wr(a.0[1], ((s.start_time + s.rendering_offset.max(0) as u64) as f64 * 1e6 / t.timescale.max(1) as f64) as i64);
                    OK
                }
                None => END_OF_STREAM,
            },
            13 => OK,
            14 => {
                wr(a.0[1], 0i64);
                wr(a.0[2], e.tracks.iter().map(|t| t.duration_us).max().unwrap_or(0));
                OK
            }
            _ => {
                eprintln!("[movie] Extractor slot {slot} not implemented");
                UNKNOWN_ERROR
            }
        })
    })()
    .unwrap_or(UNKNOWN_ERROR);
    if trace_n() {
        eprintln!("[movie] Extractor {this:#x} slot {slot}({:#x}, {:#x}) -> {r:#x}", a.0[1], a.0[2]);
    }
    r
}

const INPUT_BUFFERS: usize = 4;
const INPUT_SIZE: usize = 4 << 20;
const OUTPUT_BUFFERS: usize = 4;

fn decoder_new(_: u64, a: &Args) -> u64 {
    let this = a.0[0];
    wr(this, VTABLES.get().unwrap().0);
    let video = matches!(a.0[1] as u32, 1..=3);
    obj().decoders.insert(
        this,
        Decoder {
            video,
            events: 0,
            track: None,
            width: 0,
            height: 0,
            channels: 2,
            sample_rate: 48000,
            frames: None,
            inputs: (0..INPUT_BUFFERS).map(|_| vec![0u8; INPUT_SIZE]).collect(),
            outputs: vec![Vec::new(); OUTPUT_BUFFERS],
            free_outputs: (0..OUTPUT_BUFFERS).rev().collect(),
            ready: VecDeque::new(),
            format_sent: false,
            eos: false,
        },
    );
    if trace() {
        eprintln!("[movie] Decoder({this:#x}, type {}, format {}, mode {})", a.0[1], a.0[2], a.0[3]);
    }
    this
}

fn event(d: &Decoder, i: u64) -> u64 {
    if d.events == 0 { 0 } else { unsafe { *((d.events + 8 * i) as *const u64) } }
}

fn nv12_layout(w: u32, h: u32) -> (u32, u32) {
    (w.next_multiple_of(16), h.next_multiple_of(16))
}

fn decoder_call(slot: u64, a: &Args) -> u64 {
    let this = a.0[0];
    let mut signals = Vec::new();
    let r = if slot == 2 {
        configure(this, a.0[1])
    } else if slot == 11 {
        queue_input(this, a.0[3] as u32 as usize, a.0[4] as i64, a.0[5] as u32, &mut signals)
    } else {
        let mut o = obj();
        let decoders = &mut o.decoders;
        match decoders.get_mut(&this) {
            None => UNKNOWN_ERROR,
            Some(d) => match slot {
                0 | 1 => {
                    decoders.remove(&this);
                    if slot == 1 {
                        drop(o);
                        let del: extern "C" fn(u64) = unsafe { std::mem::transmute(g().operator_delete as usize) };
                        del(this);
                    }
                    return OK;
                }
                3 => {
                    d.events = a.0[1];
                    OK
                }
                4 | 18 => {
                    let md = if slot == 4 { a.0[1] } else { a.0[2] };
                    output_format(d, md);
                    OK
                }
                5 => {
                    signals.push(event(d, 0));
                    OK
                }
                6 | 7 => OK,
                8 => {
                    while let Some((i, ..)) = d.ready.pop_front() {
                        d.free_outputs.push(i);
                    }
                    d.eos = false;
                    signals.push(event(d, 0));
                    OK
                }
                9 => {
                    wr(a.0[1], 0i32);
                    wr(a.0[2], INPUT_BUFFERS as i32);
                    if d.eos || d.free_outputs.is_empty() { INPUT_NOT_AVAILABLE } else { OK }
                }
                10 => {
                    let Some(b) = d.inputs.get_mut(a.0[1] as usize) else { return UNKNOWN_ERROR };
                    buffer_set(a.0[2], b.as_mut_ptr(), b.len(), 0);
                    OK
                }
                12 => match d.ready.pop_front() {
                    Some((i, _, pts, flags)) => {
                        wr(a.0[1], i as i32);
                        wr(a.0[2], pts);
                        wr(a.0[3], flags);
                        wr(a.0[4], d.ready.len() as i32);
                        OK
                    }
                    None => OUTPUT_NOT_AVAILABLE,
                },
                13 | 17 => {
                    let i = a.0[1] as usize;
                    let Some(b) = d.outputs.get_mut(i) else { return UNKNOWN_ERROR };
                    let len = b.len();
                    let buf = a.0[2];
                    let (cap, base) = unsafe { (*((buf + 0x18) as *const u64) as usize, *((buf + 0x28) as *const u64)) };
                    if base != 0 && cap > 0 {
                        let n = len.min(cap);
                        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), base as *mut u8, n) };
                        buffer_set(buf, base as *mut u8, cap, n);
                    } else {
                        buffer_set(buf, b.as_mut_ptr(), len, len);
                    }
                    if slot == 17 && a.0[3] != 0 {
                        output_format(d, a.0[3]);
                    }
                    OK
                }
                14 => {
                    let size = if d.video {
                        let (stride, rows) = nv12_layout(d.width, d.height);
                        stride * rows * 3 / 2
                    } else {
                        0x2000
                    };
                    wr(a.0[1], size as i32);
                    wr(a.0[2], 1i32);
                    OK
                }
                15 => {
                    let i = a.0[1] as usize;
                    if i < OUTPUT_BUFFERS && !d.free_outputs.contains(&i) {
                        d.free_outputs.push(i);
                    }
                    signals.push(event(d, 0));
                    OK
                }
                16 => OK,
                _ => {
                    eprintln!("[movie] Decoder slot {slot} not implemented");
                    UNKNOWN_ERROR
                }
            },
        }
    };
    if trace_n() {
        eprintln!("[movie] Decoder {this:#x} slot {slot}({:#x}, {:#x}, {:#x}) -> {r:#x}", a.0[1], a.0[2], a.0[3]);
    }
    for e in signals {
        signal(e);
    }
    r
}

fn configure(this: u64, md: u64) -> u64 {
    let track = md_find_i64(md, TRACK_KEY).map(|key| ((key as u64) >> 8, (key & 0xFF) as usize));
    let (width, height) = (md_find_i32(md, "width").unwrap_or(0) as u32, md_find_i32(md, "height").unwrap_or(0) as u32);
    let channels = md_find_i32(md, "channel-count").unwrap_or(2) as u32;
    let sample_rate = md_find_i32(md, "sample-rate").unwrap_or(48000) as u32;
    let (source, extractor) = {
        let mut o = obj();
        let Objects { decoders, extractors } = &mut *o;
        let Some(d) = decoders.get_mut(&this) else { return UNKNOWN_ERROR };
        if track.is_some() {
            d.track = track;
        }
        (d.width, d.height, d.channels, d.sample_rate) = (width, height, channels, sample_rate);
        if !d.video {
            return OK;
        }
        let source = d.track.and_then(|(e, _)| extractors.get(&e)).and_then(|e| match &e.webm {
            Some(m) => Some(video::Source::Webm(m.clone())),
            None => e.host_file.clone().map(video::Source::File),
        });
        (source, d.track.map_or(0, |t| t.0))
    };
    let (stride, rows) = nv12_layout(width, height);
    let Some(src) = source.and_then(|s| crate::kernel::blocking(|| video::FrameSource::open(s, extractor, stride as usize, rows as usize))) else {
        eprintln!("[movie] decoder {this:#x}: cannot open the video for decoding");
        return UNKNOWN_ERROR;
    };
    match obj().decoders.get_mut(&this) {
        Some(d) => {
            d.frames = Some(src);
            OK
        }
        None => UNKNOWN_ERROR,
    }
}

fn queue_input(this: u64, size: usize, pts: i64, flags: u32, signals: &mut Vec<u64>) -> u64 {
    let (out, mut data, mut src) = {
        let mut o = obj();
        let Some(d) = o.decoders.get_mut(&this) else { return UNKNOWN_ERROR };
        if flags & FLAG_END_OF_STREAM != 0 && size == 0 {
            d.eos = true;
        }
        let Some(out) = d.free_outputs.pop() else { return OK };
        let src = if d.video && !d.eos { d.frames.take() } else { None };
        (out, std::mem::take(&mut d.outputs[out]), src)
    };
    let produced = src.as_mut().is_some_and(|s| s.next_frame(&mut data));
    let mut o = obj();
    let Some(d) = o.decoders.get_mut(&this) else { return OK };
    if let Some(s) = src {
        d.frames.get_or_insert(s);
    }
    let produced = produced || !d.video && size > 0;
    if !d.video && size > 0 {
        data.clear();
        data.resize(1024 * d.channels as usize * 2, 0);
    }
    if !produced && d.eos {
        data.clear();
    }
    let len = data.len();
    d.outputs[out] = data;
    if produced || d.eos {
        if !d.format_sent {
            d.format_sent = true;
            signals.push(event(d, 2));
        }
        d.ready.push_back((out, len, pts, if d.eos { FLAG_END_OF_STREAM } else { 0 }));
        signals.push(event(d, 1));
    } else {
        d.free_outputs.push(out);
    }
    if !d.free_outputs.is_empty() {
        signals.push(event(d, 0));
    }
    OK
}

fn output_format(d: &Decoder, md: u64) {
    if d.video {
        let (stride, rows) = nv12_layout(d.width, d.height);
        md_set_string(md, "mime", "video/raw");
        md_set_i32(md, "width", d.width as i32);
        md_set_i32(md, "height", d.height as i32);
        md_set_i32(md, "stride", stride as i32);
        md_set_i32(md, "slice-height", rows as i32);
        md_set_rect(md, "crop", [0, 0, d.width as i32 - 1, d.height as i32 - 1]);
    } else {
        md_set_string(md, "mime", "audio/raw");
        md_set_i32(md, "sample-rate", d.sample_rate as i32);
        md_set_i32(md, "channel-count", d.channels as i32);
    }
}
