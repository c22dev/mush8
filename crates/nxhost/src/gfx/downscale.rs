// Mush8 - downscale.rs
// Constantin Clerc (c22dev) - 0.1
use super::{Shared, Texture, gpu};
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

const MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; };
vertex V vs(uint id [[vertex_id]]) {
    float2 q = float2((id << 1) & 2, id & 2);
    V v; v.pos = float4(q * 2.0 - 1.0, 0.0, 1.0);
    return v;
}
static float3 lin(float3 c) { return select(pow((c + 0.055) / 1.055, 2.4), c / 12.92, c <= 0.04045); }
static float3 enc(float3 c) { return select(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, c <= 0.0031308); }
fragment float4 fs(V v [[stage_in]], texture2d<float> src [[texture(0)]], sampler s [[sampler(0)]], constant float3& p [[buffer(0)]]) {
    float2 f = p.xy;
    bool raw = p.z != 0.0;
    float2 size = float2(src.get_width(), src.get_height());
    float2 c = v.pos.xy * f;
    int2 n = int2(ceil(f));
    float4 sum = 0.0;
    for (int y = 0; y < n.y; y++) {
        for (int x = 0; x < n.x; x++) {
            float2 o = (float2(x, y) + 0.5) / float2(n) * f - f * 0.5;
            float4 t = src.sample(s, (c + o) / size);
            sum += float4(raw ? t.rgb : lin(saturate(t.rgb)), t.a);
        }
    }
    sum /= float(n.x * n.y);
    return float4(raw ? sum.rgb : enc(sum.rgb), sum.a);
}
"#;

struct State {
    pipeline: Shared<ProtocolObject<dyn MTLRenderPipelineState>>,
    format: MTLPixelFormat,
    out: Option<Shared<Texture>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn pipeline(format: MTLPixelFormat) -> Option<Shared<ProtocolObject<dyn MTLRenderPipelineState>>> {
    let device = &gpu().device.0;
    let lib = device.newLibraryWithSource_options_error(&NSString::from_str(MSL), None).ok()?;
    let desc = MTLRenderPipelineDescriptor::new();
    desc.setVertexFunction(lib.newFunctionWithName(&NSString::from_str("vs")).as_deref());
    desc.setFragmentFunction(lib.newFunctionWithName(&NSString::from_str("fs")).as_deref());
    unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) }.setPixelFormat(format);
    Some(Shared(device.newRenderPipelineStateWithDescriptor_error(&desc).ok()?))
}

static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000);

pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(Relaxed))
}

pub fn fit(tex: &Shared<Texture>) -> Shared<Texture> {
    let out = filter(tex);
    SCALE.store((out.0.width() as f32 / tex.0.width() as f32).to_bits(), Relaxed);
    out
}

fn filter(tex: &Shared<Texture>) -> Shared<Texture> {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("NX_SUPERSAMPLE").map_or(true, |v| v != "0"));
    let Some(screen) = super::screen_height().filter(|_| *ON) else { return tex.clone() };
    let (w, h) = (tex.0.width(), tex.0.height());
    if (h as f64) < screen * 1.05 {
        return tex.clone();
    }
    let dh = screen.round() as usize;
    let dw = (w * dh).div_ceil(h);
    let format = tex.0.pixelFormat();
    let mut st = STATE.lock().unwrap();
    if st.as_ref().is_none_or(|s| s.format != format) {
        let Some(pipeline) = pipeline(format) else { return tex.clone() };
        *st = Some(State { pipeline, format, out: None });
        eprintln!("[gfx] supersampling {w}x{h} down to {dw}x{dh}");
    }
    let s = st.as_mut().unwrap();
    if s.out.as_ref().is_none_or(|o| o.0.width() != dw || o.0.height() != dh) {
        let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(format, dw, dh, false) };
        d.setStorageMode(MTLStorageMode::Private);
        d.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
        let Some(t) = gpu().device.0.newTextureWithDescriptor(&d) else { return tex.clone() };
        s.out = Some(Shared(t));
    }
    let out = s.out.clone().unwrap();
    let g = gpu();
    let Some(cb) = g.queue.0.commandBuffer() else { return tex.clone() };
    let desc = MTLRenderPassDescriptor::new();
    let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    att.setTexture(Some(&out.0));
    att.setLoadAction(MTLLoadAction::DontCare);
    att.setStoreAction(MTLStoreAction::Store);
    let Some(enc) = cb.renderCommandEncoderWithDescriptor(&desc) else { return tex.clone() };
    let srgb = matches!(format, MTLPixelFormat::RGBA8Unorm_sRGB | MTLPixelFormat::BGRA8Unorm_sRGB);
    let f = [w as f32 / dw as f32, h as f32 / dh as f32, srgb as u32 as f32, 0.0];
    enc.setRenderPipelineState(&s.pipeline.0);
    unsafe {
        enc.setFragmentTexture_atIndex(Some(&tex.0), 0);
        enc.setFragmentSamplerState_atIndex(Some(&g.sampler.0), 0);
        enc.setFragmentBytes_length_atIndex(std::ptr::NonNull::from(&f).cast(), size_of_val(&f), 0);
        enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
    }
    enc.endEncoding();
    cb.commit();
    out
}
