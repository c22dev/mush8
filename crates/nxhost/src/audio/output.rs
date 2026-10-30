// Mush8 - output.rs
// Constantin Clerc (c22dev) - 0.1
use super::route::{Route, Watcher};
use super::system::SystemSink;
use objc2_audio_toolbox::{
    AURenderCallbackStruct, AUSpatialMixerOutputType, AUSpatialMixerSourceMode, AUSpatializationAlgorithm, AudioComponentDescription, AudioComponentFindNext, AudioComponentInstanceDispose,
    AudioComponentInstanceNew, AudioOutputUnitStart, AudioOutputUnitStop, AudioUnit, AudioUnitInitialize, AudioUnitPropertyID, AudioUnitRender, AudioUnitRenderActionFlags, AudioUnitScope,
    AudioUnitSetProperty, kAudioUnitManufacturer_Apple, kAudioUnitProperty_AudioChannelLayout, kAudioUnitProperty_ElementCount, kAudioUnitProperty_MaximumFramesPerSlice,
    kAudioUnitProperty_SetRenderCallback, kAudioUnitProperty_SpatialMixerOutputType, kAudioUnitProperty_SpatialMixerSourceMode,
    kAudioUnitProperty_SpatializationAlgorithm, kAudioUnitProperty_StreamFormat, kAudioUnitProperty_UsesInternalReverb, kAudioUnitScope_Global, kAudioUnitScope_Input, kAudioUnitScope_Output,
    kAudioUnitSubType_SpatialMixer, kAudioUnitType_Mixer, kAudioUnitType_Output,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioChannelBitmap, AudioChannelDescription, AudioChannelFlags, AudioChannelLayout, AudioChannelLayoutTag, AudioStreamBasicDescription, AudioTimeStamp,
    kAudioChannelLayoutTag_MPEG_5_1_A, kAudioChannelLayoutTag_Stereo, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
};
use std::ffi::c_void;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const CHANNELS: usize = 6;

pub type Source = Box<dyn FnMut(&mut [f32]) + Send>;

const MAX_FRAMES: usize = 4096;
const FADE_OUT: f64 = 0.005;
pub(super) const FADE_IN: f64 = 0.04;
const MUTE_TIMEOUT: Duration = Duration::from_millis(50);

#[cfg(target_os = "macos")]
const OUTPUT_SUBTYPE: u32 = objc2_audio_toolbox::kAudioUnitSubType_DefaultOutput;
#[cfg(target_os = "ios")]
const OUTPUT_SUBTYPE: u32 = u32::from_be_bytes(*b"rioc");

fn spatial_enabled() -> bool {
    !matches!(std::env::var("NX_SPATIAL").as_deref(), Ok("0" | "off"))
}

pub struct Feed {
    source: Source,
    dump: Option<Wav>,
    frames: usize,
    blocks: Vec<f32>,
    pos: usize,
}

impl Feed {
    fn new(source: Source, frames: usize, dump: Option<Wav>) -> Self {
        let slots = 1 + MAX_FRAMES.div_ceil(frames);
        Feed { source, dump, frames, blocks: vec![0.0; slots * CHANNELS * frames], pos: frames }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn take(&mut self, count: usize) -> &[f32] {
        let stride = CHANNELS * self.frames;
        self.render(count);
        self.pos = self.frames;
        &self.blocks[stride..stride * (1 + count)]
    }

    fn render(&mut self, count: usize) {
        let stride = CHANNELS * self.frames;
        let fresh = &mut self.blocks[stride..stride * (1 + count)];
        (self.source)(fresh);
        if let Some(w) = &mut self.dump {
            w.write(fresh, self.frames);
        }
        // NX_MUTE=1 keeps the audio running but silent, for test runs
        static MUTE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("NX_MUTE").is_some());
        if *MUTE {
            fresh.fill(0.0);
        }
    }

    fn pull(&mut self, out: &mut [&mut [f32]; CHANNELS]) {
        let (n, want) = (self.frames, out[0].len());
        let stride = CHANNELS * n;
        let count = (want + self.pos).saturating_sub(n).div_ceil(n);
        if (1 + count) * stride > self.blocks.len() {
            out.iter_mut().for_each(|c| c.fill(0.0));
            return;
        }
        if count > 0 {
            self.render(count);
        }
        let (mut done, mut slot, mut pos) = (0, 0, self.pos);
        while done < want {
            if pos == n {
                slot += 1;
                pos = 0;
            }
            let len = (n - pos).min(want - done);
            let block = &self.blocks[slot * stride..(slot + 1) * stride];
            for (c, dst) in out.iter_mut().enumerate() {
                dst[done..done + len].copy_from_slice(&block[c * n + pos..c * n + pos + len]);
            }
            pos += len;
            done += len;
        }
        if slot > 0 {
            self.blocks.copy_within(slot * stride..(slot + 1) * stride, 0);
        }
        self.pos = pos;
    }
}

struct Gate {
    muted: AtomicBool,
    silent: AtomicBool,
}

struct Fader {
    mixer: AudioUnit,
    gate: Arc<Gate>,
    gain: f32,
    step_out: f32,
    step_in: f32,
}

impl Fader {
    fn apply(&mut self, channels: &mut [&mut [f32]]) {
        let target = if self.gate.muted.load(Relaxed) { 0.0 } else { 1.0 };
        if self.gain == target {
            if target == 0.0 {
                channels.iter_mut().for_each(|c| c.fill(0.0));
            }
            return;
        }
        let step = if target == 0.0 { -self.step_out } else { self.step_in };
        let mut end = self.gain;
        for c in channels.iter_mut() {
            let mut g = self.gain;
            for s in c.iter_mut() {
                g = (g + step).clamp(0.0, 1.0);
                *s *= g;
            }
            end = g;
        }
        self.gain = end;
        self.gate.silent.store(end == 0.0, Relaxed);
    }
}

unsafe fn buffers<'a, const N: usize>(io: *mut AudioBufferList, frames: u32) -> Option<[&'a mut [f32]; N]> {
    if io.is_null() || unsafe { (*io).mNumberBuffers } as usize != N {
        return None;
    }
    let raw = unsafe { (&raw mut (*io).mBuffers).cast::<AudioBuffer>() };
    Some(std::array::from_fn(|c| unsafe { std::slice::from_raw_parts_mut((*raw.add(c)).mData as *mut f32, frames as usize) }))
}

unsafe extern "C-unwind" fn render_feed(ctx: NonNull<c_void>, _: NonNull<AudioUnitRenderActionFlags>, _: NonNull<AudioTimeStamp>, _: u32, frames: u32, io: *mut AudioBufferList) -> i32 {
    let Some(mut out) = (unsafe { buffers::<CHANNELS>(io, frames) }) else { return -50 };
    match unsafe { ctx.cast::<Mutex<Feed>>().as_ref() }.try_lock() {
        Ok(mut feed) => feed.pull(&mut out),
        Err(_) => out.iter_mut().for_each(|c| c.fill(0.0)),
    }
    0
}

unsafe extern "C-unwind" fn render_output(ctx: NonNull<c_void>, flags: NonNull<AudioUnitRenderActionFlags>, ts: NonNull<AudioTimeStamp>, _: u32, frames: u32, io: *mut AudioBufferList) -> i32 {
    let fader = unsafe { ctx.cast::<Fader>().as_mut() };
    let Some(list) = NonNull::new(io) else { return -50 };
    let status = unsafe { AudioUnitRender(fader.mixer, flags.as_ptr(), ts, 0, frames, list) };
    let Some(mut out) = (unsafe { buffers::<2>(io, frames) }) else { return -50 };
    if status != 0 {
        out.iter_mut().for_each(|c| c.fill(0.0));
        return 0;
    }
    fader.apply(&mut out);
    0
}

fn check(status: i32) -> Result<(), i32> {
    if status == 0 { Ok(()) } else { Err(status) }
}

struct Unit(AudioUnit);

impl Unit {
    fn new(kind: u32, subtype: u32) -> Result<Self, i32> {
        let desc = AudioComponentDescription { componentType: kind, componentSubType: subtype, componentManufacturer: kAudioUnitManufacturer_Apple, componentFlags: 0, componentFlagsMask: 0 };
        let component = unsafe { AudioComponentFindNext(std::ptr::null_mut(), NonNull::from(&desc)) };
        if component.is_null() {
            return Err(-1);
        }
        let mut unit: AudioUnit = std::ptr::null_mut();
        check(unsafe { AudioComponentInstanceNew(component, NonNull::from(&mut unit)) })?;
        Ok(Unit(unit))
    }

    fn set<T>(&self, id: AudioUnitPropertyID, scope: AudioUnitScope, value: &T) -> Result<(), i32> {
        check(unsafe { AudioUnitSetProperty(self.0, id, scope, 0, value as *const T as *const c_void, size_of::<T>() as u32) })
    }

    fn set_route(&self, route: Route) -> Result<(), i32> {
        let kind = match route {
            Route::Headphones => AUSpatialMixerOutputType::SpatialMixerOutputType_Headphones,
            Route::Speakers => AUSpatialMixerOutputType::SpatialMixerOutputType_BuiltInSpeakers,
            Route::External => AUSpatialMixerOutputType::SpatialMixerOutputType_ExternalSpeakers,
        };
        self.set(kAudioUnitProperty_SpatialMixerOutputType, kAudioUnitScope_Global, &kind)
    }
}

impl Drop for Unit {
    fn drop(&mut self) {
        unsafe { AudioComponentInstanceDispose(self.0) };
    }
}

unsafe impl Send for Unit {}
unsafe impl Sync for Unit {}

fn format(rate: u32, channels: usize) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: rate as f64,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: channels as u32,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

pub(super) fn layout(tag: AudioChannelLayoutTag) -> AudioChannelLayout {
    AudioChannelLayout {
        mChannelLayoutTag: tag,
        mChannelBitmap: AudioChannelBitmap(0),
        mNumberChannelDescriptions: 0,
        mChannelDescriptions: [AudioChannelDescription { mChannelLabel: 0, mChannelFlags: AudioChannelFlags(0), mCoordinates: [0.0; 3] }],
    }
}

fn spatial_mixer(rate: u32, spatial: bool, feed: *const Mutex<Feed>) -> Result<Unit, i32> {
    let mixer = Unit::new(kAudioUnitType_Mixer, kAudioUnitSubType_SpatialMixer)?;
    let source_mode = if !spatial { AUSpatialMixerSourceMode::SpatialMixerSourceMode_Bypass } else { AUSpatialMixerSourceMode::SpatialMixerSourceMode_AmbienceBed };
    let _ = mixer.set(kAudioUnitProperty_ElementCount, kAudioUnitScope_Input, &1u32);
    mixer.set(kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, &(MAX_FRAMES as u32))?;
    mixer.set(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, &format(rate, CHANNELS))?;
    mixer.set(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, &format(rate, 2))?;
    mixer.set(kAudioUnitProperty_AudioChannelLayout, kAudioUnitScope_Input, &layout(kAudioChannelLayoutTag_MPEG_5_1_A))?;
    mixer.set(kAudioUnitProperty_AudioChannelLayout, kAudioUnitScope_Output, &layout(kAudioChannelLayoutTag_Stereo))?;
    mixer.set(kAudioUnitProperty_SpatializationAlgorithm, kAudioUnitScope_Input, &AUSpatializationAlgorithm::SpatializationAlgorithm_UseOutputType)?;
    mixer.set(kAudioUnitProperty_SpatialMixerSourceMode, kAudioUnitScope_Input, &source_mode)?;
    let _ = mixer.set(kAudioUnitProperty_UsesInternalReverb, kAudioUnitScope_Global, &0u32);
    mixer.set(kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, &AURenderCallbackStruct { inputProc: Some(render_feed), inputProcRefCon: feed.cast_mut().cast() })?;
    check(unsafe { AudioUnitInitialize(mixer.0) })?;
    Ok(mixer)
}

fn output_unit(rate: u32, fader: *mut Fader) -> Result<Unit, i32> {
    let out = Unit::new(kAudioUnitType_Output, OUTPUT_SUBTYPE)?;
    out.set(kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, &format(rate, 2))?;
    out.set(kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, &AURenderCallbackStruct { inputProc: Some(render_output), inputProcRefCon: fader.cast() })?;
    check(unsafe { AudioUnitInitialize(out.0) })?;
    Ok(out)
}

enum Event {
    RouteChanged,
    Stop,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sink {
    Mixer(Route),
    System(u32),
}

struct Director {
    out: Arc<Unit>,
    mixer: Arc<Unit>,
    gate: Arc<Gate>,
    system: Option<SystemSink>,
    sink: Option<Sink>,
}

impl Director {
    fn pick(&self, route: Route, watcher: &Watcher) -> Sink {
        match route {
            Route::Headphones if self.system.is_some() => Sink::System((watcher.latency() * 1000.0).round() as u32),
            r => Sink::Mixer(r),
        }
    }

    fn switch(&mut self, to: Sink) {
        match self.sink {
            Some(Sink::Mixer(_)) => {
                self.gate.muted.store(true, Relaxed);
                let deadline = Instant::now() + MUTE_TIMEOUT;
                while !self.gate.silent.load(Relaxed) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if matches!(to, Sink::System(_)) {
                    unsafe { AudioOutputUnitStop(self.out.0) };
                }
            }
            Some(Sink::System(_)) => self.system.as_mut().unwrap().stop(),
            None => {}
        }
        match to {
            Sink::Mixer(route) => {
                if let Err(e) = self.mixer.set_route(route) {
                    eprintln!("[audio] can't render for {route:?} ({e})");
                }
                if !matches!(self.sink, Some(Sink::Mixer(_))) {
                    unsafe { AudioOutputUnitStart(self.out.0) };
                }
                self.gate.muted.store(false, Relaxed);
                eprintln!("[audio] output: {route:?} through the mixer");
            }
            Sink::System(latency) => {
                self.system.as_mut().unwrap().start(latency as f64 / 1000.0);
                eprintln!("[audio] output: headphones through the system renderer");
            }
        }
        self.sink = Some(to);
    }

    fn run(mut self, tx: Sender<Event>, rx: Receiver<Event>) {
        let mut watcher = Watcher::new(Box::new(move || {
            let _ = tx.send(Event::RouteChanged);
        }));
        loop {
            let to = self.pick(watcher.route(), &watcher);
            if self.sink != Some(to) {
                self.switch(to);
            }
            if !matches!(rx.recv(), Ok(Event::RouteChanged)) {
                break;
            }
        }
        if let Some(s) = &mut self.system {
            s.stop();
        }
        unsafe { AudioOutputUnitStop(self.out.0) };
    }
}

struct Engine {
    router: JoinHandle<()>,
    events: Sender<Event>,
}

pub struct Output {
    engine: Option<Engine>,
    _feed: Arc<Mutex<Feed>>,
    fader: *mut Fader,
    stop: Arc<AtomicBool>,
}

unsafe impl Send for Output {}
unsafe impl Sync for Output {}

pub fn start(rate: u32, frames: usize, source: Source) -> Output {
    let dump = std::env::var_os("NX_DUMP_AUDIO").and_then(|p| match Wav::create(&p, rate) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("[audio] can't create {}: {e}", p.to_string_lossy());
            None
        }
    });
    let spatial = spatial_enabled();
    let feed = Arc::new(Mutex::new(Feed::new(source, frames, dump)));
    let gate = Arc::new(Gate { muted: AtomicBool::new(true), silent: AtomicBool::new(true) });
    let fader = Box::into_raw(Box::new(Fader {
        mixer: std::ptr::null_mut(),
        gate: gate.clone(),
        gain: 0.0,
        step_out: (1.0 / (FADE_OUT * rate as f64)) as f32,
        step_in: (1.0 / (FADE_IN * rate as f64)) as f32,
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let engine = spatial_mixer(rate, spatial, Arc::as_ptr(&feed)).and_then(|mixer| {
        unsafe { (*fader).mixer = mixer.0 };
        let out = output_unit(rate, fader)?;
        let (tx, rx) = channel();
        let (f, t) = (feed.clone(), tx.clone());
        let router = std::thread::Builder::new()
            .name("audio-route".into())
            .spawn(move || {
                let system = if spatial { SystemSink::new(f, rate) } else { None };
                Director { out: Arc::new(out), mixer: Arc::new(mixer), gate, system, sink: None }.run(t, rx)
            })
            .unwrap();
        Ok(Engine { router, events: tx })
    });
    match engine {
        Ok(engine) => {
            eprintln!("[audio] 5.1 output, spatial audio {}", if spatial { "on" } else { "off" });
            Output { engine: Some(engine), _feed: feed, fader, stop }
        }
        Err(status) => {
            eprintln!("[audio] no output device (AudioUnit error {status}); rendering without output");
            let (s, f) = (stop.clone(), feed.clone());
            std::thread::Builder::new()
                .name("audio-out".into())
                .spawn(move || {
                    let period = Duration::from_secs_f64(frames as f64 / rate as f64);
                    let mut next = Instant::now();
                    while !s.load(Relaxed) {
                        f.lock().unwrap().render(1);
                        next += period;
                        std::thread::sleep(next.saturating_duration_since(Instant::now()));
                    }
                })
                .unwrap();
            Output { engine: None, _feed: feed, fader, stop }
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(engine) = self.engine.take() {
            let _ = engine.events.send(Event::Stop);
            let _ = engine.router.join();
        }
        drop(unsafe { Box::from_raw(self.fader) });
    }
}

struct Wav {
    file: File,
    bytes: u32,
}

impl Wav {
    fn create(path: &std::ffi::OsStr, rate: u32) -> std::io::Result<Self> {
        let mut file = File::create(path)?;
        let align = 2 * CHANNELS as u32;
        let mut h = Vec::with_capacity(44);
        h.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        for v in [16u32, 1 | (CHANNELS as u32) << 16, rate, rate * align, align | 16 << 16] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(b"data\0\0\0\0");
        file.write_all(&h)?;
        Ok(Wav { file, bytes: 0 })
    }

    fn write(&mut self, blocks: &[f32], frames: usize) {
        let mut data = Vec::with_capacity(blocks.len() * 2);
        for block in blocks.chunks_exact(CHANNELS * frames) {
            for j in 0..frames {
                for c in 0..CHANNELS {
                    data.extend_from_slice(&((block[c * frames + j] * 32768.0).clamp(-32768.0, 32767.0) as i16).to_le_bytes());
                }
            }
        }
        self.bytes += data.len() as u32;
        let _ = (|| -> std::io::Result<()> {
            self.file.write_all(&data)?;
            self.file.seek(SeekFrom::Start(4))?;
            self.file.write_all(&(36 + self.bytes).to_le_bytes())?;
            self.file.seek(SeekFrom::Start(40))?;
            self.file.write_all(&self.bytes.to_le_bytes())?;
            self.file.seek(SeekFrom::End(0)).map(|_| ())
        })();
    }
}
