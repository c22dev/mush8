// Mush8 - mod.rs
// Constantin Clerc (c22dev) - 0.1
#[cfg(target_os = "ios")]
mod ios;
pub mod bc;
pub mod overlay;
#[cfg(target_os = "ios")]
mod touch;

#[cfg(target_os = "ios")]
pub use ios::{run_app, set_touch_controls, wait_foreground};

#[cfg(target_os = "macos")]
pub fn set_touch_controls(_visible: bool) {}

#[cfg(target_os = "macos")]
pub fn wait_foreground() {}

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2::{MainThreadMarker, MainThreadOnly};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_core_foundation::CGSize;
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_foundation::NSString;
use objc2_metal::*;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use std::sync::OnceLock;

pub type Device = ProtocolObject<dyn MTLDevice>;
pub type Texture = ProtocolObject<dyn MTLTexture>;
pub type CommandBuffer = ProtocolObject<dyn MTLCommandBuffer>;

pub struct Shared<T: ?Sized>(pub Retained<T>);
unsafe impl<T: ?Sized> Send for Shared<T> {}
unsafe impl<T: ?Sized> Sync for Shared<T> {}
impl<T: objc2::Message> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared(self.0.clone())
    }
}

pub type Buffer = ProtocolObject<dyn MTLBuffer>;

pub struct HeapBuffer {
    pub buffer: Shared<Buffer>,
    pub base: u64,
    pub size: u64,
    pub gpu: u64,
}

static HEAP: std::sync::RwLock<Option<&'static HeapBuffer>> = std::sync::RwLock::new(None);

pub fn wrap_heap(base: u64, size: u64) -> bool {
    let ptr = std::ptr::NonNull::new(base as *mut std::ffi::c_void).unwrap();
    let b = unsafe { gpu().device.0.newBufferWithBytesNoCopy_length_options_deallocator(ptr, size as usize, MTLResourceOptions::StorageModeShared, None) };
    match b {
        Some(b) => {
            let gpu = b.gpuAddress();
            eprintln!("[gfx] guest heap {base:#x}+{size:#x} mapped for the GPU at {gpu:#x}");
            *HEAP.write().unwrap() = Some(Box::leak(Box::new(HeapBuffer { buffer: Shared(b), base, size, gpu })));
            true
        }
        None => {
            eprintln!("[gfx] could not wrap the guest heap {base:#x}+{size:#x} for the GPU");
            false
        }
    }
}

pub fn heap() -> Option<&'static HeapBuffer> {
    *HEAP.read().unwrap()
}

pub fn max_buffer_len() -> u64 {
    gpu().device.0.maxBufferLength() as u64
}

impl HeapBuffer {
    pub fn gpu_addr(&self, addr: u64) -> Option<u64> {
        (addr >= self.base && addr < self.base + self.size).then(|| self.gpu + (addr - self.base))
    }
}

pub struct Gpu {
    pub device: Shared<Device>,
    pub zeros: Shared<Buffer>,
    pub queue: Shared<ProtocolObject<dyn MTLCommandQueue>>,
    present: Shared<ProtocolObject<dyn MTLRenderPipelineState>>,
    sampler: Shared<ProtocolObject<dyn MTLSamplerState>>,
}

static GPU: OnceLock<Gpu> = OnceLock::new();
static LAYER: OnceLock<Shared<CAMetalLayer>> = OnceLock::new();

pub fn gpu() -> &'static Gpu {
    GPU.get_or_init(|| {
        let device = MTLCreateSystemDefaultDevice().expect("no Metal device");
        eprintln!("[gfx] {} ({})", device.name(), device.architecture().name());
        let queue = device.newCommandQueue().expect("command queue");
        let (present, sampler) = make_present_pipeline(&device);
        let zeros = device.newBufferWithLength_options(0x10000, MTLResourceOptions::StorageModeShared).expect("zero buffer");
        Gpu { device: Shared(device), zeros: Shared(zeros), queue: Shared(queue), present: Shared(present), sampler: Shared(sampler) }
    })
}

const PRESENT_MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; float2 uv; };
struct P { uint flip; float2 offset; float2 scale; float2 lo; float2 hi; };
vertex V vs(uint id [[vertex_id]], constant P& p [[buffer(0)]]) {
    float2 q = float2((id << 1) & 2, id & 2);
    V v; v.pos = float4(q * 2.0 - 1.0, 0.0, 1.0);
    v.uv = p.offset + p.scale * float2(q.x, p.flip != 0u ? q.y : 1.0 - q.y);
    return v;
}
fragment float4 fs(V v [[stage_in]], constant P& p [[buffer(0)]], texture2d<float> t [[texture(0)]], sampler s [[sampler(0)]]) {
    return float4(t.sample(s, clamp(v.uv, p.lo, p.hi)).rgb, 1.0);
}
"#;

fn make_present_pipeline(
    device: &Device,
) -> (Retained<ProtocolObject<dyn MTLRenderPipelineState>>, Retained<ProtocolObject<dyn MTLSamplerState>>) {
    let lib = device.newLibraryWithSource_options_error(&NSString::from_str(PRESENT_MSL), None).expect("present shader");
    let vs = lib.newFunctionWithName(&NSString::from_str("vs")).unwrap();
    let fs = lib.newFunctionWithName(&NSString::from_str("fs")).unwrap();
    let desc = MTLRenderPipelineDescriptor::new();
    desc.setVertexFunction(Some(&vs));
    desc.setFragmentFunction(Some(&fs));
    unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) }.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    let pipeline = device.newRenderPipelineStateWithDescriptor_error(&desc).expect("present pipeline");
    let sd = MTLSamplerDescriptor::new();
    sd.setMinFilter(MTLSamplerMinMagFilter::Linear);
    sd.setMagFilter(MTLSamplerMinMagFilter::Linear);
    let sampler = device.newSamplerStateWithDescriptor(&sd).unwrap();
    (pipeline, sampler)
}

#[cfg(target_os = "macos")]
pub fn run_app(title: &str, width: f64, height: f64) -> ! {
    let mtm = MainThreadMarker::new().expect("run_app must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let rect = NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(width, height));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::Resizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false)
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(title));
    let view = window.contentView().expect("content view");
    view.setWantsLayer(true);
    let layer = CAMetalLayer::new();
    layer.setDevice(Some(&gpu().device.0));
    layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    let scale = window.backingScaleFactor();
    layer.setContentsScale(scale);
    layer.setDrawableSize(CGSize { width: width * scale, height: height * scale });
    view.setLayer(Some(&layer));
    let _ = LAYER.set(Shared(layer));
    window.setContentAspectRatio(NSSize::new(width, height));
    window.center();
    overlay::set_game_window(&window);
    window.setCollectionBehavior(objc2_app_kit::NSWindowCollectionBehavior::FullScreenPrimary);
    window.makeKeyAndOrderFront(None);
    if std::env::var_os("NX_WINDOWED").is_none() {
        window.toggleFullScreen(None);
    }
    let win = window.clone();
    let swallow = block2::RcBlock::new(move |e: std::ptr::NonNull<objc2_app_kit::NSEvent>| -> *mut objc2_app_kit::NSEvent {
        let ev = unsafe { e.as_ref() };
        if ev.modifierFlags().contains(objc2_app_kit::NSEventModifierFlags::Command) && ev.r#type() == objc2_app_kit::NSEventType::KeyDown {
            match ev.charactersIgnoringModifiers().map(|c| c.to_string().to_lowercase()).as_deref() {
                Some("q" | "w") => std::process::exit(0),
                Some("f") if !ev.isARepeat() => win.toggleFullScreen(None),
                _ => {}
            }
        }
        std::ptr::null_mut()
    });
    let close = block2::RcBlock::new(|_: std::ptr::NonNull<objc2_foundation::NSNotification>| std::process::exit(0));
    let observer = unsafe {
        objc2_foundation::NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSWindowWillCloseNotification),
            Some(&window),
            None,
            &close,
        )
    };
    std::mem::forget(observer);
    let monitor = unsafe {
        objc2_app_kit::NSEvent::addLocalMonitorForEventsMatchingMask_handler(objc2_app_kit::NSEventMask::KeyDown | objc2_app_kit::NSEventMask::KeyUp, &swallow)
    };
    std::mem::forget(monitor);
    let held = std::cell::Cell::new(false);
    let game = window.clone();
    let mouse = block2::RcBlock::new(move |e: std::ptr::NonNull<objc2_app_kit::NSEvent>| -> *mut objc2_app_kit::NSEvent {
        mouse_touch(&game, unsafe { e.as_ref() }, &held);
        e.as_ptr()
    });
    let monitor = unsafe {
        use objc2_app_kit::NSEventMask as M;
        objc2_app_kit::NSEvent::addLocalMonitorForEventsMatchingMask_handler(M::LeftMouseDown | M::LeftMouseDragged | M::LeftMouseUp, &mouse)
    };
    std::mem::forget(monitor);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    app.run();
    std::process::exit(0)
}

pub fn pixel_format(nvn: u32) -> Option<MTLPixelFormat> {
    if let Some(f) = bc::format(nvn) {
        return Some(f);
    }
    use MTLPixelFormat as P;
    Some(match nvn {
        1 => P::R8Unorm,
        2 => P::R8Snorm,
        3 => P::R8Uint,
        4 => P::R8Sint,
        5 => P::R16Float,
        6 => P::R16Unorm,
        7 => P::R16Snorm,
        8 => P::R16Uint,
        9 => P::R16Sint,
        10 => P::R32Float,
        11 => P::R32Uint,
        12 => P::R32Sint,
        13 => P::RG8Unorm,
        14 => P::RG8Snorm,
        15 => P::RG8Uint,
        16 => P::RG8Sint,
        17 => P::RG16Float,
        18 => P::RG16Unorm,
        19 => P::RG16Snorm,
        20 => P::RG16Uint,
        21 => P::RG16Sint,
        22 => P::RG32Float,
        23 => P::RG32Uint,
        24 => P::RG32Sint,
        37 => P::RGBA8Unorm,
        38 => P::RGBA8Snorm,
        39 => P::RGBA8Uint,
        40 => P::RGBA8Sint,
        41 => P::RGBA16Float,
        42 => P::RGBA16Unorm,
        43 => P::RGBA16Snorm,
        44 => P::RGBA16Uint,
        45 => P::RGBA16Sint,
        46 => P::RGBA32Float,
        47 => P::RGBA32Uint,
        48 => P::RGBA32Sint,
        49 => P::Stencil8,
        50 => P::Depth16Unorm,
        51 | 52 => P::Depth32Float,
        53 | 54 => P::Depth32Float_Stencil8,
        55 | 56 => P::RGBA8Unorm_sRGB,
        57 => P::ABGR4Unorm,
        59 => P::A1BGR5Unorm,
        60 => P::B5G6R5Unorm,
        61 => P::RGB10A2Unorm,
        62 => P::RGB10A2Uint,
        63 => P::RG11B10Float,
        64 => P::RGB9E5Float,
        65 | 66 => P::BC1_RGBA,
        67 => P::BC2_RGBA,
        68 => P::BC3_RGBA,
        69 | 70 => P::BC1_RGBA_sRGB,
        71 => P::BC2_RGBA_sRGB,
        72 => P::BC3_RGBA_sRGB,
        73 => P::BC4_RUnorm,
        74 => P::BC4_RSnorm,
        75 => P::BC5_RGUnorm,
        76 => P::BC5_RGSnorm,
        77 => P::BC7_RGBAUnorm,
        78 => P::BC7_RGBAUnorm_sRGB,
        79 => P::BC6H_RGBFloat,
        80 => P::BC6H_RGBUfloat,
        121..=134 => ASTC_LDR[(nvn - 121) as usize],
        135..=148 => ASTC_SRGB[(nvn - 135) as usize],
        _ => return None,
    })
}

const ASTC_LDR: [MTLPixelFormat; 14] = {
    use MTLPixelFormat as P;
    [
        P::ASTC_4x4_LDR, P::ASTC_5x4_LDR, P::ASTC_5x5_LDR, P::ASTC_6x5_LDR, P::ASTC_6x6_LDR, P::ASTC_8x5_LDR, P::ASTC_8x6_LDR,
        P::ASTC_8x8_LDR, P::ASTC_10x5_LDR, P::ASTC_10x6_LDR, P::ASTC_10x8_LDR, P::ASTC_10x10_LDR, P::ASTC_12x10_LDR, P::ASTC_12x12_LDR,
    ]
};
const ASTC_SRGB: [MTLPixelFormat; 14] = {
    use MTLPixelFormat as P;
    [
        P::ASTC_4x4_sRGB, P::ASTC_5x4_sRGB, P::ASTC_5x5_sRGB, P::ASTC_6x5_sRGB, P::ASTC_6x6_sRGB, P::ASTC_8x5_sRGB, P::ASTC_8x6_sRGB,
        P::ASTC_8x8_sRGB, P::ASTC_10x5_sRGB, P::ASTC_10x6_sRGB, P::ASTC_10x8_sRGB, P::ASTC_10x10_sRGB, P::ASTC_12x10_sRGB, P::ASTC_12x12_sRGB,
    ]
};

pub fn has_stencil(nvn: u32) -> bool {
    matches!(nvn, 49 | 53 | 54)
}

pub fn is_depth_format(nvn: u32) -> bool {
    (49..=54).contains(&nvn)
}

#[derive(Clone, Copy, Debug)]
pub enum Load {
    Keep,
    ClearColor([f64; 4]),
    ClearDepthStencil(f64, u32),
}

pub struct Attachments {
    pub colors: Vec<Option<(Shared<Texture>, Load)>>,
    pub depth: Option<(Shared<Texture>, Load, bool)>,
    pub subs: Vec<(u32, u32)>,
    pub depth_sub: (u32, u32),
}

fn set_subresource(att: &MTLRenderPassAttachmentDescriptor, tex: &Texture, (layer, level): (u32, u32)) {
    att.setLevel(level as usize);
    if tex.textureType() == MTLTextureType::Type3D {
        att.setDepthPlane(layer as usize);
    } else {
        att.setSlice(layer as usize);
    }
}

pub type RenderEncoder = ProtocolObject<dyn MTLRenderCommandEncoder>;

pub fn run_pass(cb: &CommandBuffer, a: &Attachments) {
    if let Some(enc) = begin_pass(cb, a) {
        enc.endEncoding();
    }
}

pub fn begin_pass(cb: &CommandBuffer, a: &Attachments) -> Option<Retained<RenderEncoder>> {
    let desc = MTLRenderPassDescriptor::new();
    for (i, c) in a.colors.iter().enumerate() {
        let Some((tex, load)) = c else { continue };
        let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(i) };
        att.setTexture(Some(&tex.0));
        set_subresource(&att, &tex.0, a.subs.get(i).copied().unwrap_or((0, 0)));
        att.setStoreAction(MTLStoreAction::Store);
        match load {
            Load::ClearColor(c) => {
                att.setLoadAction(MTLLoadAction::Clear);
                att.setClearColor(MTLClearColor { red: c[0], green: c[1], blue: c[2], alpha: c[3] });
            }
            _ => att.setLoadAction(MTLLoadAction::Load),
        }
    }
    if let Some((tex, load, has_stencil)) = &a.depth {
        let d = desc.depthAttachment();
        d.setTexture(Some(&tex.0));
        set_subresource(&d, &tex.0, a.depth_sub);
        d.setStoreAction(MTLStoreAction::Store);
        if let Load::ClearDepthStencil(z, _) = load {
            d.setLoadAction(MTLLoadAction::Clear);
            d.setClearDepth(*z);
        } else {
            d.setLoadAction(MTLLoadAction::Load);
        }
        if *has_stencil {
            let s = desc.stencilAttachment();
            s.setTexture(Some(&tex.0));
            set_subresource(&s, &tex.0, a.depth_sub);
            s.setStoreAction(MTLStoreAction::Store);
            if let Load::ClearDepthStencil(_, v) = load {
                s.setLoadAction(MTLLoadAction::Clear);
                s.setClearStencil(*v);
            } else {
                s.setLoadAction(MTLLoadAction::Load);
            }
        }
    }
    cb.renderCommandEncoderWithDescriptor(&desc)
}

#[repr(C)]
struct PresentParams {
    flip: u32,
    _pad: u32,
    offset: [f32; 2],
    scale: [f32; 2],
    lo: [f32; 2],
    hi: [f32; 2],
}

static CROP: std::sync::Mutex<Option<[u32; 4]>> = std::sync::Mutex::new(None);

pub fn set_crop(x: i32, y: i32, w: i32, h: i32) {
    let c = (w > 0 && h > 0).then(|| [x.max(0) as u32, y.max(0) as u32, w as u32, h as u32]);
    let mut cur = CROP.lock().unwrap();
    if *cur != c {
        eprintln!("[gfx] window crop {c:?}");
        *cur = c;
    }
}

fn present_params(tex: &Texture, flip: bool) -> (PresentParams, f64) {
    let (tw, th) = (tex.width().max(1) as f32, tex.height().max(1) as f32);
    let [x, y, w, h] = CROP.lock().unwrap().map_or([0.0, 0.0, tw, th], |c| {
        let x = (c[0] as f32).min(tw - 1.0);
        let y = (c[1] as f32).min(th - 1.0);
        [x, y, (c[2] as f32).min(tw - x), (c[3] as f32).min(th - y)]
    });
    let p = PresentParams {
        flip: flip as u32,
        _pad: 0,
        offset: [x / tw, y / th],
        scale: [w / tw, h / th],
        lo: [(x + 0.5) / tw, (y + 0.5) / th],
        hi: [(x + w - 0.5) / tw, (y + h - 0.5) / th],
    };
    (p, (w / h) as f64)
}

// None keeps the image on screen as it is, while still counting and pacing the frame
pub fn present(tex: Option<&Texture>, flip: bool) {
    #[cfg(target_os = "ios")]
    if !ios::ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let turbo = next_frame();
    let interval = PRESENT_INTERVAL.load(std::sync::atomic::Ordering::Relaxed);
    if turbo {
        pace(FRAME / 4);
        static SHOWN: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        let mut shown = SHOWN.lock().unwrap();
        if shown.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(50)) {
            return;
        }
        *shown = Some(std::time::Instant::now());
    } else if interval > 0 {
        pace(FRAME * interval);
    }
    let Some(tex) = tex else { return };
    let Some(layer) = LAYER.get() else { return };
    let bounds = layer.0.bounds().size;
    let scale = layer.0.contentsScale();
    let size = CGSize { width: (bounds.width * scale).round().max(1.0), height: (bounds.height * scale).round().max(1.0) };
    if layer.0.drawableSize() != size {
        layer.0.setDrawableSize(size);
    }
    let Some(drawable) = layer.0.nextDrawable() else { return };
    let g = gpu();
    let Some(cb) = g.queue.0.commandBuffer() else { return };
    let desc = MTLRenderPassDescriptor::new();
    let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    att.setTexture(Some(&drawable.texture()));
    att.setLoadAction(MTLLoadAction::Clear);
    att.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 1.0 });
    att.setStoreAction(MTLStoreAction::Store);
    if let Some(enc) = cb.renderCommandEncoderWithDescriptor(&desc) {
        let (params, aspect) = present_params(tex, flip);
        let aspect = aspect * crate::aspect::stretch();
        ASPECT.store(aspect.to_bits(), std::sync::atomic::Ordering::Relaxed);
        enc.setViewport(letterbox(aspect, size.width, size.height));
        enc.setRenderPipelineState(&g.present.0);
        unsafe {
            let p = std::ptr::NonNull::from(&params).cast();
            enc.setVertexBytes_length_atIndex(p, size_of::<PresentParams>(), 0);
            enc.setFragmentBytes_length_atIndex(p, size_of::<PresentParams>(), 0);
            enc.setFragmentTexture_atIndex(Some(tex), 0);
            enc.setFragmentSamplerState_atIndex(Some(&g.sampler.0), 0);
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        enc.endEncoding();
    }
    let d: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
    if interval == 0 || turbo {
        cb.presentDrawable(d);
    } else {
        cb.presentDrawable_afterMinimumDuration(d, interval as f64 / 60.0);
    }
    cb.commit();
}

static ASPECT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn screen_point(x: f64, y: f64, w: f64, h: f64) -> (f32, f32) {
    let aspect = Some(f64::from_bits(ASPECT.load(std::sync::atomic::Ordering::Relaxed))).filter(|a| *a > 0.0).unwrap_or(16.0 / 9.0);
    let (vw, vh) = if w / h > aspect { (h * aspect, h) } else { (w, w / aspect) };
    (((x - (w - vw) / 2.0) / vw) as f32, ((y - (h - vh) / 2.0) / vh) as f32)
}

pub fn on_screen((x, y): (f32, f32)) -> bool {
    (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)
}

#[cfg(target_os = "macos")]
fn mouse_touch(window: &NSWindow, ev: &objc2_app_kit::NSEvent, held: &std::cell::Cell<bool>) {
    use objc2_app_kit::NSEventType as T;
    let Some(view) = window.contentView() else { return };
    let ours = ev.window(view.mtm()).is_some_and(|w| std::ptr::eq(&*w, window));
    let b = view.bounds().size;
    let p = view.convertPoint_fromView(ev.locationInWindow(), None);
    let y = if view.isFlipped() { p.y } else { b.height - p.y };
    let pt = screen_point(p.x, y, b.width, b.height);
    match ev.r#type() {
        T::LeftMouseDown if ours && window.isKeyWindow() && on_screen(pt) => {
            held.set(true);
            crate::input::screen_touch(0, Some(pt));
        }
        T::LeftMouseDragged if held.get() => crate::input::screen_touch(0, Some(pt)),
        T::LeftMouseUp if held.replace(false) => crate::input::screen_touch(0, None),
        _ => {}
    }
}

fn letterbox(aspect: f64, w: f64, h: f64) -> MTLViewport {
    let (vw, vh) = if w / h > aspect { (h * aspect, h) } else { (w, w / aspect) };
    MTLViewport { originX: ((w - vw) / 2.0).floor(), originY: ((h - vh) / 2.0).floor(), width: vw.round(), height: vh.round(), znear: 0.0, zfar: 1.0 }
}

static PRESENT_INTERVAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

const FRAME: std::time::Duration = std::time::Duration::from_nanos(16_666_667);
static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
static TURBO_UNTIL: std::sync::LazyLock<Option<f32>> = std::sync::LazyLock::new(|| env_secs("NX_TURBO"));

fn env_secs(k: &str) -> Option<f32> {
    std::env::var(k).ok().and_then(|v| v.parse().ok())
}

pub fn clock() -> f32 {
    if TURBO_UNTIL.is_some() { FRAMES.load(std::sync::atomic::Ordering::Relaxed) as f32 / 60.0 } else { START.elapsed().as_secs_f32() }
}

pub fn turbo() -> bool {
    TURBO_UNTIL.is_some_and(|u| clock() < u)
}

fn next_frame() -> bool {
    static EXIT: std::sync::LazyLock<Option<f32>> = std::sync::LazyLock::new(|| env_secs("NX_EXIT_AT"));
    let n = FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let t = n as f32 / 60.0;
    if n % 600 == 0 && std::env::var_os("NX_FPS").is_some() {
        eprintln!("[clock] game {t:.0} s at {:.1} s", START.elapsed().as_secs_f32());
    }
    if EXIT.is_some_and(|e| t >= e) {
        eprintln!("[host] NX_EXIT_AT reached");
        std::process::exit(0);
    }
    TURBO_UNTIL.is_some_and(|u| t < u)
}

fn pace(period: std::time::Duration) {
    static DUE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut due = DUE.lock().unwrap();
    let now = std::time::Instant::now();
    let mut next = due.unwrap_or(now);
    if now < next {
        std::thread::sleep(next - now);
    } else if now - next > period * 2 {
        next = now;
    }
    *due = Some(next + period);
}

pub fn set_present_interval(n: u32) {
    PRESENT_INTERVAL.store(n, std::sync::atomic::Ordering::Relaxed);
}

pub fn dump_png(tex: &Texture, path: &str) {
    let (w, h) = (tex.width(), tex.height());
    let g = gpu();
    let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(MTLPixelFormat::BGRA8Unorm, w, h, false) };
    d.setStorageMode(MTLStorageMode::Shared);
    d.setUsage(MTLTextureUsage::RenderTarget);
    let Some(out) = g.device.0.newTextureWithDescriptor(&d) else { return };
    let Some(cb) = g.queue.0.commandBuffer() else { return };
    let desc = MTLRenderPassDescriptor::new();
    let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    att.setTexture(Some(&out));
    att.setLoadAction(MTLLoadAction::Clear);
    att.setStoreAction(MTLStoreAction::Store);
    let Some(enc) = cb.renderCommandEncoderWithDescriptor(&desc) else { return };
    enc.setRenderPipelineState(&g.present.0);
    let params = PresentParams { flip: 0, _pad: 0, offset: [0.0; 2], scale: [1.0; 2], lo: [0.0; 2], hi: [1.0; 2] };
    unsafe {
        let p = std::ptr::NonNull::from(&params).cast();
        enc.setVertexBytes_length_atIndex(p, size_of::<PresentParams>(), 0);
        enc.setFragmentBytes_length_atIndex(p, size_of::<PresentParams>(), 0);
        enc.setFragmentTexture_atIndex(Some(tex), 0);
        enc.setFragmentSamplerState_atIndex(Some(&g.sampler.0), 0);
        enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
    }
    enc.endEncoding();
    cb.commit();
    cb.waitUntilCompleted();
    let mut px = vec![0u8; w * h * 4];
    unsafe {
        out.getBytes_bytesPerRow_fromRegion_mipmapLevel(
            std::ptr::NonNull::new(px.as_mut_ptr()).unwrap().cast(),
            w * 4,
            MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: w, height: h, depth: 1 } },
            0,
        )
    };
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let Ok(file) = std::fs::File::create(path) else { return };
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    if let Ok(mut wr) = e.write_header() {
        let _ = wr.write_image_data(&px);
    }
}

pub fn open_url(url: &'static str) {
    dispatch2::DispatchQueue::main().exec_async(move || unsafe {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        let Some(u) = objc2_foundation::NSURL::URLWithString(&objc2_foundation::NSString::from_str(url)) else { return };
        #[cfg(target_os = "macos")]
        {
            let ws: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            let _: bool = msg_send![ws, openURL: &*u];
        }
        #[cfg(target_os = "ios")]
        {
            let app: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
            let options = objc2_foundation::NSDictionary::<AnyObject, AnyObject>::new();
            let _: () = msg_send![app, openURL: &*u, options: &*options, completionHandler: std::ptr::null::<AnyObject>()];
        }
    });
}
