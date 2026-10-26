// Mush8 - fi.rs
// Constantin Clerc (c22dev) - 0.1
use super::textures::TexKey;
use crate::gfx::{self, Shared, Texture};
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use objc2_metal_fx::{MTLFXFrameInterpolator, MTLFXFrameInterpolatorBase, MTLFXFrameInterpolatorDescriptor};
use std::collections::HashMap;
use std::sync::LazyLock;

pub static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("NX_METALFX").is_ok_and(|v| v != "0"));

type Interpolator = ProtocolObject<dyn MTLFXFrameInterpolator>;

struct Setup {
    key: (usize, usize, MTLPixelFormat),
    fx: Shared<Interpolator>,
    color: [Shared<Texture>; 2],
    depth: Shared<Texture>,
    motion: Shared<Texture>,
    output: Shared<Texture>,
    resample: Shared<ProtocolObject<dyn MTLRenderPipelineState>>,
    depth_write: Shared<ProtocolObject<dyn MTLDepthStencilState>>,
}

const RESAMPLE_MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; };
vertex V vs(uint id [[vertex_id]]) {
    float2 q = float2((id << 1) & 2, id & 2);
    V v; v.pos = float4(q * 2.0 - 1.0, 0.0, 1.0);
    return v;
}
struct D { float depth [[depth(any)]]; };
fragment D fs(V v [[stage_in]], depth2d<float> src [[texture(0)]], constant float2& scale [[buffer(0)]]) {
    uint2 p = min(uint2(v.pos.xy * scale), uint2(src.get_width() - 1, src.get_height() - 1));
    D d; d.depth = src.read(p);
    return d;
}
"#;

#[derive(Default)]
pub struct State {
    depths: HashMap<TexKey, (Shared<Texture>, u32)>,
    setup: Option<Setup>,
    prev: Option<usize>,
    fresh: bool,
    failed: bool,
}

impl State {
    pub fn pass_drawn(&mut self, depth: Option<&(TexKey, Shared<Texture>)>, draws: u32) {
        if let Some((k, t)) = depth.filter(|_| draws > 0) {
            self.depths.entry(*k).or_insert_with(|| (t.clone(), 0)).1 += draws;
        }
    }
}

fn resample_pipeline() -> Option<(Shared<ProtocolObject<dyn MTLRenderPipelineState>>, Shared<ProtocolObject<dyn MTLDepthStencilState>>)> {
    use objc2_foundation::NSString;
    let device = &gfx::gpu().device.0;
    let lib = device.newLibraryWithSource_options_error(&NSString::from_str(RESAMPLE_MSL), None).ok()?;
    let desc = MTLRenderPipelineDescriptor::new();
    desc.setVertexFunction(lib.newFunctionWithName(&NSString::from_str("vs")).as_deref());
    desc.setFragmentFunction(lib.newFunctionWithName(&NSString::from_str("fs")).as_deref());
    desc.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float);
    let pipeline = device.newRenderPipelineStateWithDescriptor_error(&desc).ok()?;
    let ds = MTLDepthStencilDescriptor::new();
    ds.setDepthCompareFunction(MTLCompareFunction::Always);
    ds.setDepthWriteEnabled(true);
    let state = device.newDepthStencilStateWithDescriptor(&ds)?;
    Some((Shared(pipeline), Shared(state)))
}

fn texture(w: usize, h: usize, format: MTLPixelFormat, usage: MTLTextureUsage) -> Option<Shared<Texture>> {
    let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(format, w, h, false) };
    d.setStorageMode(MTLStorageMode::Private);
    d.setUsage(usage);
    Some(Shared(gfx::gpu().device.0.newTextureWithDescriptor(&d)?))
}

fn create(w: usize, h: usize, color: MTLPixelFormat) -> Option<Setup> {
    let depth = MTLPixelFormat::Depth32Float;
    let device = &gfx::gpu().device.0;
    if !unsafe { MTLFXFrameInterpolatorDescriptor::supportsDevice(device) } {
        eprintln!("[metalfx] frame interpolation is not supported on this GPU");
        return None;
    }
    let d = unsafe { MTLFXFrameInterpolatorDescriptor::new() };
    unsafe {
        d.setColorTextureFormat(color);
        d.setOutputTextureFormat(color);
        d.setDepthTextureFormat(depth);
        d.setMotionTextureFormat(MTLPixelFormat::RG16Float);
        d.setInputWidth(w);
        d.setInputHeight(h);
        d.setOutputWidth(w);
        d.setOutputHeight(h);
    }
    let Some(fx) = (unsafe { d.newFrameInterpolatorWithDevice(device) }) else {
        eprintln!("[metalfx] no interpolator for {w}x{h} {color:?}");
        return None;
    };
    let rw = MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite | MTLTextureUsage::RenderTarget;
    let (cu, du, mu, ou) = unsafe { (fx.colorTextureUsage(), fx.depthTextureUsage(), fx.motionTextureUsage(), fx.outputTextureUsage()) };
    let (resample, depth_write) = resample_pipeline()?;
    let setup = Setup {
        key: (w, h, color),
        color: [texture(w, h, color, rw | cu)?, texture(w, h, color, rw | cu)?],
        depth: texture(w, h, depth, MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget | du)?,
        motion: texture(w, h, MTLPixelFormat::RG16Float, rw | mu)?,
        output: texture(w, h, color, rw | ou)?,
        resample,
        depth_write,
        fx: Shared(fx),
    };
    let cb = gfx::gpu().queue.0.commandBuffer()?;
    let desc = MTLRenderPassDescriptor::new();
    let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    att.setTexture(Some(&setup.motion.0));
    att.setLoadAction(MTLLoadAction::Clear);
    att.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 });
    att.setStoreAction(MTLStoreAction::Store);
    cb.renderCommandEncoderWithDescriptor(&desc)?.endEncoding();
    cb.commit();
    eprintln!("[metalfx] interpolating {w}x{h}");
    gfx::request_high_refresh();
    Some(setup)
}

pub fn frame(st: &mut State, window: &Shared<Texture>) -> Option<Shared<Texture>> {
    let depths = std::mem::take(&mut st.depths);
    if gfx::INTERP_OFF.load(std::sync::atomic::Ordering::Relaxed) || !gfx::high_refresh() {
        st.prev = None;
        return None;
    }
    let (w, h, color) = (window.0.width(), window.0.height(), window.0.pixelFormat());
    let Some(scene) = depths.into_values().filter(|(t, _)| t.0.pixelFormat() == MTLPixelFormat::Depth32Float).max_by_key(|(_, n)| *n).map(|(t, _)| t) else {
        st.prev = None;
        return None;
    };
    let key = (w, h, color);
    if st.setup.as_ref().is_none_or(|s| s.key != key) {
        if st.failed {
            return None;
        }
        st.setup = create(w, h, color);
        st.prev = None;
        st.failed = st.setup.is_none();
    }
    let s = st.setup.as_ref()?;
    let cur = st.prev.map_or(0, |p| 1 - p);
    let fresh = std::mem::take(&mut st.fresh);
    let cb = gfx::gpu().queue.0.commandBuffer()?;
    let blit = cb.blitCommandEncoder()?;
    unsafe { blit.copyFromTexture_toTexture(&window.0, &s.color[cur].0) };
    blit.endEncoding();
    let desc = MTLRenderPassDescriptor::new();
    let att = desc.depthAttachment();
    att.setTexture(Some(&s.depth.0));
    att.setLoadAction(MTLLoadAction::DontCare);
    att.setStoreAction(MTLStoreAction::Store);
    let enc = cb.renderCommandEncoderWithDescriptor(&desc)?;
    let scale = [scene.0.width() as f32 / w as f32, scene.0.height() as f32 / h as f32];
    enc.setRenderPipelineState(&s.resample.0);
    enc.setDepthStencilState(Some(&s.depth_write.0));
    unsafe {
        enc.setFragmentTexture_atIndex(Some(&scene.0), 0);
        enc.setFragmentBytes_length_atIndex(std::ptr::NonNull::from(&scale).cast(), size_of_val(&scale), 0);
        enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
    }
    enc.endEncoding();
    let out = st.prev.map(|prev| {
        unsafe {
            let fx = &s.fx.0;
            fx.setColorTexture(Some(&s.color[cur].0));
            fx.setPrevColorTexture(Some(&s.color[prev].0));
            fx.setDepthTexture(Some(&s.depth.0));
            fx.setMotionTexture(Some(&s.motion.0));
            fx.setOutputTexture(Some(&s.output.0));
            fx.setMotionVectorScaleX(1.0);
            fx.setMotionVectorScaleY(1.0);
            fx.setDeltaTime(1.0 / 60.0);
            fx.setNearPlane(0.1);
            fx.setFarPlane(10000.0);
            fx.setFieldOfView(60.0);
            fx.setAspectRatio(w as f32 / h as f32);
            fx.setDepthReversed(false);
            fx.setShouldResetHistory(fresh);
            fx.encodeToCommandBuffer(&cb);
        }
        s.output.clone()
    });
    cb.commit();
    if st.prev.is_none() {
        st.fresh = true;
    }
    st.prev = Some(cur);
    out
}
