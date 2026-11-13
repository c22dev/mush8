// Mush8 - bc.rs
// Constantin Clerc (c22dev) - 0.1
use objc2_metal::{MTLDevice, MTLPixelFormat};
use std::sync::LazyLock;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Bc1,
    Bc2,
    Bc3,
    Bc4(bool),
    Bc5(bool),
    Bc6(bool),
    Bc7,
}

pub fn enabled() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| {
        let soft = !super::gpu().device.0.supportsBCTextureCompression() || std::env::var_os("NX_SOFT_BC").is_some();
        if soft {
            eprintln!("[gfx] no BC texture support: decoding BC textures on the CPU");
        }
        soft
    });
    *S
}

pub fn soft(nvn: u32) -> Option<Kind> {
    if !(65..=80).contains(&nvn) || !enabled() {
        return None;
    }
    Some(match nvn {
        65 | 66 | 69 | 70 => Kind::Bc1,
        67 | 71 => Kind::Bc2,
        68 | 72 => Kind::Bc3,
        73 | 74 => Kind::Bc4(nvn == 74),
        75 | 76 => Kind::Bc5(nvn == 76),
        77 | 78 => Kind::Bc7,
        _ => Kind::Bc6(nvn == 79),
    })
}

pub fn format(nvn: u32) -> Option<MTLPixelFormat> {
    use MTLPixelFormat as P;
    let srgb = matches!(nvn, 69..=72 | 78);
    Some(match soft(nvn)? {
        Kind::Bc4(signed) => if signed { P::R8Snorm } else { P::R8Unorm },
        Kind::Bc5(signed) => if signed { P::RG8Snorm } else { P::RG8Unorm },
        Kind::Bc6(_) => P::RGBA16Float,
        _ if srgb => P::RGBA8Unorm_sRGB,
        _ => P::RGBA8Unorm,
    })
}

fn pixel_bytes(k: Kind) -> usize {
    match k {
        Kind::Bc4(_) => 1,
        Kind::Bc5(_) => 2,
        Kind::Bc6(_) => 8,
        _ => 4,
    }
}

pub fn block_bytes(k: Kind) -> usize {
    if matches!(k, Kind::Bc1 | Kind::Bc4(_)) { 8 } else { 16 }
}

pub fn decode(k: Kind, src: &[u8], row_bytes: usize, w: usize, h: usize, images: usize) -> (Vec<u8>, usize) {
    static CORES: LazyLock<usize> = LazyLock::new(|| std::thread::available_parallelism().map_or(1, |n| n.get()).min(8));
    let px = pixel_bytes(k);
    let mut out = vec![0u8; w * h * px * images];
    if out.is_empty() {
        return (out, w * px);
    }
    let (rows, total) = (h.div_ceil(4), out.len());
    let jobs: Vec<(usize, &mut [u8])> = out.chunks_mut(w * h * px).flat_map(|img| img.chunks_mut(4 * w * px)).enumerate().collect();
    let threads = if total >= 1 << 20 { (*CORES).min(jobs.len()) } else { 1 };
    let per = jobs.len().div_ceil(threads);
    let decode_rows = |group: Vec<(usize, &mut [u8])>| {
        for (i, dst) in group {
            let (img, by) = (i / rows, i % rows);
            decode_row(k, src, (img * rows + by) * row_bytes, w, px, dst);
        }
    };
    let mut jobs = jobs.into_iter();
    std::thread::scope(|sc| {
        for _ in 1..threads {
            let group: Vec<_> = jobs.by_ref().take(per).collect();
            sc.spawn(|| decode_rows(group));
        }
        decode_rows(jobs.collect());
    });
    (out, w * px)
}

fn decode_row(k: Kind, src: &[u8], start: usize, w: usize, px: usize, dst: &mut [u8]) {
    let block = block_bytes(k);
    let lines = dst.len() / (w * px);
    let mut tmp = [0u8; 4 * 4 * 8];
    let mut half = [0u16; 4 * 4 * 3];
    for bx in 0..w.div_ceil(4) {
        let o = start + bx * block;
        let Some(b) = src.get(o..o + block) else { return };
        match k {
            Kind::Bc1 => bcdec_rs::bc1(b, &mut tmp, 16),
            Kind::Bc2 => bcdec_rs::bc2(b, &mut tmp, 16),
            Kind::Bc3 => bcdec_rs::bc3(b, &mut tmp, 16),
            Kind::Bc4(signed) => bcdec_rs::bc4(b, &mut tmp, 4, signed),
            Kind::Bc5(signed) => bcdec_rs::bc5(b, &mut tmp, 8, signed),
            Kind::Bc6(signed) => {
                bcdec_rs::bc6h_half(b, &mut half, 12, signed);
                for i in 0..16 {
                    for c in 0..4 {
                        let v = if c < 3 { half[i * 3 + c] } else { 0x3c00 };
                        tmp[i * 8 + c * 2..i * 8 + c * 2 + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
            Kind::Bc7 => bcdec_rs::bc7(b, &mut tmp, 16),
        }
        let n = (w - bx * 4).min(4) * px;
        for r in 0..lines.min(4) {
            let d = (r * w + bx * 4) * px;
            dst[d..d + n].copy_from_slice(&tmp[r * 4 * px..r * 4 * px + n]);
        }
    }
}
