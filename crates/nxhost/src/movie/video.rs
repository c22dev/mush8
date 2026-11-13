// Mush8 - video.rs
// Constantin Clerc (c22dev) - 0.1
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::*;
use objc2_core_foundation::CFString;
use objc2_core_media::*;
use objc2_core_video::*;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, channel, sync_channel};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

static SEEKS: LazyLock<Mutex<HashMap<u64, i64>>> = LazyLock::new(Default::default);

pub fn seek_all(extractor: u64, us: i64) {
    SEEKS.lock().unwrap().insert(extractor, us);
}

const AHEAD: usize = 4;
const FIRST_WAIT: Duration = Duration::from_millis(250);
const FRAME_WAIT: Duration = Duration::from_millis(8);

struct Worker {
    frames: Receiver<Vec<u8>>,
    recycle: Sender<Vec<u8>>,
}

#[derive(Clone)]
pub enum Source {
    File(PathBuf),
    Webm(std::sync::Arc<super::webm::Movie>),
}

pub struct FrameSource {
    source: Source,
    extractor: u64,
    layout: (usize, usize),
    worker: Worker,
    first: Option<Vec<u8>>,
    behind: usize,
}

impl FrameSource {
    pub fn open(source: Source, extractor: u64, stride: usize, rows: usize) -> Option<Self> {
        let worker = spawn(source.clone(), 0, stride, rows)?;
        let first = worker.frames.recv_timeout(FIRST_WAIT).ok();
        Some(Self { source, extractor, layout: (stride, rows), worker, first, behind: 0 })
    }

    fn wait(&mut self) -> Result<Vec<u8>, RecvTimeoutError> {
        if let Some(f) = self.first.take() {
            return Ok(f);
        }
        match self.worker.frames.try_recv() {
            Ok(f) => Ok(f),
            Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
            Err(TryRecvError::Empty) => crate::kernel::blocking(|| self.worker.frames.recv_timeout(FRAME_WAIT)),
        }
    }

    pub fn next_frame(&mut self, out: &mut Vec<u8>) -> bool {
        if let Some(us) = SEEKS.lock().unwrap().remove(&self.extractor) {
            match crate::kernel::blocking(|| spawn(self.source.clone(), us, self.layout.0, self.layout.1)) {
                Some(w) => {
                    self.worker = w;
                    self.first = None;
                    self.behind = 0;
                }
                None => return false,
            }
        }
        let mut frame = match self.wait() {
            Ok(f) => f,
            Err(RecvTimeoutError::Disconnected) => return false,
            Err(RecvTimeoutError::Timeout) => {
                self.behind = (self.behind + 1).min(AHEAD);
                static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("[movie] video decoder is late: showing the previous frame");
                }
                let (stride, rows) = self.layout;
                if out.len() != stride * rows * 3 / 2 {
                    out.clear();
                    out.resize(stride * rows * 3 / 2, 0);
                    out[stride * rows..].fill(128);
                }
                return true;
            }
        };
        while self.behind > 0 {
            let Ok(newer) = self.worker.frames.try_recv() else { break };
            let _ = self.worker.recycle.send(std::mem::replace(&mut frame, newer));
            self.behind -= 1;
        }
        std::mem::swap(out, &mut frame);
        let _ = self.worker.recycle.send(frame);
        true
    }
}

fn spawn(source: Source, us: i64, stride: usize, rows: usize) -> Option<Worker> {
    let (tx, frames) = sync_channel(AHEAD);
    let (recycle, spare) = channel();
    let (ready_tx, ready) = sync_channel(1);
    std::thread::Builder::new()
        .name("movie-decode".into())
        .spawn(move || match source {
            Source::File(path) => {
                let reader = objc2::rc::autoreleasepool(|_| Reader::open(&path, us));
                let _ = ready_tx.send(reader.is_some());
                if let Some(reader) = reader {
                    reader.run(&tx, &spare, stride, rows);
                }
            }
            Source::Webm(movie) => {
                let _ = ready_tx.send(true);
                run_webm(&movie, us, &tx, &spare, stride, rows);
            }
        })
        .ok()?;
    ready.recv_timeout(Duration::from_secs(5)).ok()?.then_some(Worker { frames, recycle })
}

fn run_webm(movie: &super::webm::Movie, us: i64, tx: &SyncSender<Vec<u8>>, spare: &Receiver<Vec<u8>>, stride: usize, rows: usize) {
    let size = stride * rows * 3 / 2;
    let start = movie.frames.iter().position(|f| f.us >= us).unwrap_or(movie.frames.len());
    let key = movie.frames[..=start.min(movie.frames.len() - 1)].iter().rposition(|f| f.key).unwrap_or(0);
    let Some(mut dec) = vpx::Vp8::new() else {
        eprintln!("[movie] VP8 decoder failed to start");
        return;
    };
    let mut last = vec![0u8; size];
    last[stride * rows..].fill(128);
    let mut warned = false;
    for (i, f) in movie.frames.iter().enumerate().skip(key) {
        match dec.decode(&movie.data[f.range.clone()]) {
            Ok(Some(img)) => nv12(&img, &mut last, stride, rows),
            Ok(None) => {}
            Err(e) if !warned => {
                warned = true;
                eprintln!("[movie] VP8 frame {i}: decode error {e}");
            }
            Err(_) => {}
        }
        if i < start {
            continue;
        }
        let mut out = spare.try_recv().unwrap_or_default();
        out.clear();
        out.extend_from_slice(&last);
        if tx.send(out).is_err() {
            return;
        }
    }
}

fn nv12(img: &vpx::Image, out: &mut [u8], stride: usize, rows: usize) {
    let (w, h) = (img.width.min(stride), img.height.min(rows));
    let [y, u, v] = img.planes;
    let [ys, us, vs] = img.strides;
    for r in 0..h {
        out[r * stride..r * stride + w].copy_from_slice(&y[r * ys..r * ys + w]);
    }
    let uv = stride * rows;
    let (cw, ch) = (img.width.div_ceil(2).min(stride / 2), img.height.div_ceil(2).min(rows / 2));
    for r in 0..ch {
        let (u, v, o) = (&u[r * us..r * us + cw], &v[r * vs..r * vs + cw], &mut out[uv + r * stride..uv + r * stride + cw * 2]);
        for x in 0..cw {
            o[2 * x] = u[x];
            o[2 * x + 1] = v[x];
        }
    }
}

struct Reader {
    _reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
}

impl Reader {
    fn open(path: &Path, us: i64) -> Option<Self> {
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
            let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
            #[allow(deprecated)]
            let tracks = asset.tracksWithMediaType(AVMediaTypeVideo?);
            let track = tracks.firstObject()?;
            let reader = AVAssetReader::assetReaderWithAsset_error(&asset).ok()?;
            let key: &NSString = &*(kCVPixelBufferPixelFormatTypeKey as *const CFString as *const NSString);
            let value = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[&*value as &AnyObject]);
            let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&track, Some(&settings));
            output.setAlwaysCopiesSampleData(false);
            reader.addOutput(&output);
            if us > 0 {
                let start = CMTime { value: us, timescale: 1_000_000, flags: CMTimeFlags::Valid, epoch: 0 };
                reader.setTimeRange(CMTimeRange { start, duration: kCMTimePositiveInfinity });
            }
            if !reader.startReading() {
                eprintln!("[movie] AVAssetReader failed to start");
                return None;
            }
            Some(Self { _reader: reader, output })
        }
    }

    fn run(&self, tx: &SyncSender<Vec<u8>>, spare: &Receiver<Vec<u8>>, stride: usize, rows: usize) {
        loop {
            let mut out = spare.try_recv().unwrap_or_default();
            out.resize(stride * rows * 3 / 2, 0);
            if !objc2::rc::autoreleasepool(|_| self.decode(&mut out, stride, rows)) || tx.send(out).is_err() {
                return;
            }
        }
    }

    fn decode(&self, out: &mut [u8], stride: usize, rows: usize) -> bool {
        let Some(sample) = (unsafe { self.output.copyNextSampleBuffer() }) else { return false };
        let Some(image) = (unsafe { sample.image_buffer() }) else { return false };
        let pb: &CVPixelBuffer = &image;
        unsafe { CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly) };
        for plane in 0..2 {
            let base = CVPixelBufferGetBaseAddressOfPlane(pb, plane) as *const u8;
            if base.is_null() {
                continue;
            }
            let src_stride = CVPixelBufferGetBytesPerRowOfPlane(pb, plane);
            let height = CVPixelBufferGetHeightOfPlane(pb, plane);
            let width_bytes = CVPixelBufferGetWidthOfPlane(pb, plane) * if plane == 0 { 1 } else { 2 };
            let dst0 = if plane == 0 { 0 } else { stride * rows };
            for y in 0..height.min(if plane == 0 { rows } else { rows / 2 }) {
                let n = width_bytes.min(stride);
                let d = dst0 + y * stride;
                if d + n <= out.len() {
                    unsafe { std::ptr::copy_nonoverlapping(base.add(y * src_stride), out[d..].as_mut_ptr(), n) };
                }
            }
        }
        unsafe { CVPixelBufferUnlockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly) };
        true
    }
}
