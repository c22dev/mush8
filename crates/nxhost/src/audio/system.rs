// Mush8 - system.rs
// Constantin Clerc (c22dev) - 0.1
use super::output::{CHANNELS, FADE_IN, Feed, layout};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_av_foundation::{AVAudioSpatializationFormats, AVQueuedSampleBufferRendering, AVSampleBufferAudioRenderer, AVSampleBufferRenderSynchronizer};
use objc2_core_audio_types::{AudioStreamBasicDescription, kAudioChannelLayoutTag_MPEG_5_1_A, kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMAudioFormatDescriptionCreate, CMAudioSampleBufferCreateReadyWithPacketDescriptions, CMBlockBuffer, CMFormatDescription, CMTime};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const CHUNK_BLOCKS: usize = 2;
const TICK: Duration = Duration::from_millis(4);
const MARGIN: f64 = 0.05;

struct Renderer {
    audio: Retained<AVSampleBufferAudioRenderer>,
    sync: Retained<AVSampleBufferRenderSynchronizer>,
    format: CFRetained<CMFormatDescription>,
}

unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    fn enqueue(&self, pcm: &[f32], frames: usize, pts: CMTime) -> Option<()> {
        let bytes = size_of_val(pcm);
        let mut block = std::ptr::null_mut();
        let status = unsafe { CMBlockBuffer::create_with_memory_block(None, std::ptr::null_mut(), bytes, None, std::ptr::null(), 0, bytes, 0, NonNull::from(&mut block)) };
        let block = (status == 0).then(|| NonNull::new(block)).flatten().map(|b| unsafe { CFRetained::from_raw(b) })?;
        (unsafe { CMBlockBuffer::replace_data_bytes(NonNull::from(pcm).cast(), &block, 0, bytes) } == 0).then_some(())?;
        let mut sample = std::ptr::null_mut();
        let status = unsafe { CMAudioSampleBufferCreateReadyWithPacketDescriptions(None, &block, &self.format, frames as isize, pts, std::ptr::null(), NonNull::from(&mut sample)) };
        let sample = (status == 0).then(|| NonNull::new(sample)).flatten().map(|s| unsafe { CFRetained::from_raw(s) })?;
        unsafe { self.audio.enqueueSampleBuffer(&sample) };
        Some(())
    }
}

fn format_description(rate: u32) -> Option<CFRetained<CMFormatDescription>> {
    let asbd = AudioStreamBasicDescription {
        mSampleRate: rate as f64,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
        mBytesPerPacket: 4 * CHANNELS as u32,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4 * CHANNELS as u32,
        mChannelsPerFrame: CHANNELS as u32,
        mBitsPerChannel: 32,
        mReserved: 0,
    };
    let channels = layout(kAudioChannelLayoutTag_MPEG_5_1_A);
    let mut out = std::ptr::null();
    let status = unsafe { CMAudioFormatDescriptionCreate(None, NonNull::from(&asbd), size_of_val(&channels), &channels, 0, std::ptr::null(), None, NonNull::from(&mut out)) };
    (status == 0).then(|| NonNull::new(out as *mut CMFormatDescription)).flatten().map(|f| unsafe { CFRetained::from_raw(f) })
}

pub struct SystemSink {
    renderer: Arc<Renderer>,
    feed: Arc<Mutex<Feed>>,
    rate: u32,
    running: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

impl SystemSink {
    pub fn new(feed: Arc<Mutex<Feed>>, rate: u32) -> Option<Self> {
        #[cfg(target_os = "ios")]
        if let Err(e) = unsafe { objc2_avf_audio::AVAudioSession::sharedInstance().setSupportsMultichannelContent_error(true) } {
            eprintln!("[audio] multichannel content refused: {e}");
        }
        let format = format_description(rate)?;
        let audio = unsafe { AVSampleBufferAudioRenderer::new() };
        let sync = unsafe { AVSampleBufferRenderSynchronizer::new() };
        unsafe {
            audio.setAllowedAudioSpatializationFormats(AVAudioSpatializationFormats::Multichannel);
            sync.addRenderer(ProtocolObject::from_ref(&*audio));
        }
        Some(SystemSink { renderer: Arc::new(Renderer { audio, sync, format }), feed, rate, running: None })
    }

    pub fn start(&mut self, latency: f64) {
        self.stop();
        let lead = std::env::var("NX_AUDIO_LEAD_MS").ok().and_then(|v| v.parse::<f64>().ok()).map_or(latency + MARGIN, |ms| ms / 1000.0);
        let stop = Arc::new(AtomicBool::new(false));
        let (r, f, s, rate) = (self.renderer.clone(), self.feed.clone(), stop.clone(), self.rate);
        let feeder = std::thread::Builder::new().name("audio-system".into()).spawn(move || pump(&r, &f, rate, lead, &s)).unwrap();
        self.running = Some((stop, feeder));
        eprintln!("[audio] system renderer, {:.0} ms ahead", lead * 1000.0);
    }

    pub fn stopped(&self) -> bool {
        self.running.as_ref().is_some_and(|(_, feeder)| !feeder.is_finished()) && unsafe { self.renderer.sync.rate() } == 0.0
    }

    pub fn stop(&mut self) {
        let Some((stop, feeder)) = self.running.take() else { return };
        stop.store(true, Relaxed);
        let _ = feeder.join();
        unsafe {
            self.renderer.sync.setRate(0.0);
            self.renderer.audio.flush();
        }
    }
}

impl Drop for SystemSink {
    fn drop(&mut self) {
        self.stop();
    }
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

fn realtime(period: Duration) {
    let mut tb = Timebase { numer: 0, denom: 0 };
    unsafe { mach_timebase_info(&mut tb) };
    let ticks = |d: Duration| (d.as_nanos() as u64 * tb.denom as u64 / tb.numer.max(1) as u64) as u32;
    let mut policy = libc::thread_time_constraint_policy { period: ticks(period), computation: ticks(period / 4), constraint: ticks(period), preemptible: 1 };
    let status = unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as u32,
            (&raw mut policy).cast(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    if status != 0 {
        eprintln!("[audio] feeder stays non-realtime ({status})");
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };
    }
}

fn pump(r: &Renderer, feed: &Mutex<Feed>, rate: u32, lead: f64, stop: &AtomicBool) {
    realtime(TICK);
    let block = feed.lock().unwrap().frames();
    let chunk = CHUNK_BLOCKS * block;
    let ahead = (lead * rate as f64) as i64;
    let late = ahead - (MARGIN * rate as f64) as i64;
    let fade = (FADE_IN * rate as f64) as usize;
    let mut pcm = vec![0f32; chunk * CHANNELS];
    let (mut written, mut faded) = (0i64, 0usize);
    unsafe {
        r.audio.flush();
        r.sync.setRate_time(1.0, CMTime::new(-ahead, rate as i32));
    }
    while !stop.load(Relaxed) {
        let played = (unsafe { r.sync.currentTime().seconds() } * rate as f64) as i64;
        if written < played + late {
            eprintln!("[audio] system renderer starved {:.0} ms, resyncing", (played + ahead - written) as f64 * 1000.0 / rate as f64);
            written = played + ahead;
            faded = 0;
        }
        while written < played + ahead && unsafe { r.audio.isReadyForMoreMediaData() } {
            {
                let mut f = feed.lock().unwrap();
                let blocks = f.take(CHUNK_BLOCKS);
                for (k, planar) in blocks.chunks_exact(CHANNELS * block).enumerate() {
                    for j in 0..block {
                        for c in 0..CHANNELS {
                            pcm[(k * block + j) * CHANNELS + c] = planar[c * block + j];
                        }
                    }
                }
            }
            for frame in pcm.chunks_exact_mut(CHANNELS).take(fade.saturating_sub(faded)) {
                let gain = faded as f32 / fade as f32;
                frame.iter_mut().for_each(|s| *s *= gain);
                faded += 1;
            }
            if r.enqueue(&pcm, chunk, unsafe { CMTime::new(written, rate as i32) }).is_none() {
                eprintln!("[audio] system renderer refused a buffer");
            }
            written += chunk as i64;
        }
        std::thread::sleep(TICK);
    }
}
