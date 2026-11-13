// Mush8 - bntx.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Context, Result, bail, ensure};

const RGBA8_UNORM: u32 = 0x0B01;
const RGBA8_SRGB: u32 = 0x0B06;

pub struct Texture {
    pub name: String,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub block_height_log2: u32,
    pub tile_mode: u16,
    pub depth: u32,
    pub layers: u32,
    pub mip_offsets: usize,
    pub info: usize,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(b.get(o..o + 2).context("BNTX cut short")?.try_into()?))
}
fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(b.get(o..o + 4).context("BNTX cut short")?.try_into()?))
}
fn u64_at(b: &[u8], o: usize) -> Result<usize> {
    Ok(u64::from_le_bytes(b.get(o..o + 8).context("BNTX cut short")?.try_into()?) as usize)
}

pub fn textures(data: &[u8]) -> Result<Vec<Texture>> {
    ensure!(data.len() >= 0x40 && &data[..4] == b"BNTX" && &data[0x20..0x24] == b"NX  ", "not a BNTX");
    let count = u32_at(data, 0x24)? as usize;
    let infos = u64_at(data, 0x28)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = u64_at(data, infos + i * 8)?;
        ensure!(data.get(t..t + 4) == Some(b"BRTI"), "BNTX texture {i} is not a BRTI");
        let name_at = u64_at(data, t + 0x60)?;
        let len = u16_at(data, name_at)? as usize;
        out.push(Texture {
            name: String::from_utf8_lossy(data.get(name_at + 2..name_at + 2 + len).context("BNTX name cut short")?).into_owned(),
            tile_mode: u16_at(data, t + 0x12)?,
            mips: u16_at(data, t + 0x16)? as u32,
            format: u32_at(data, t + 0x1C)?,
            width: u32_at(data, t + 0x24)?,
            height: u32_at(data, t + 0x28)?,
            depth: u32_at(data, t + 0x2C)?,
            layers: u32_at(data, t + 0x30)?,
            block_height_log2: u32_at(data, t + 0x34)? & 7,
            mip_offsets: u64_at(data, t + 0x70)?,
            info: t,
        });
    }
    Ok(out)
}

fn gob_offset(x: u32, y: u32) -> u32 {
    (x % 64 / 32) * 256 + (y % 8 / 2) * 64 + (x % 32 / 16) * 32 + (y % 2) * 16 + x % 16
}

fn tiled_offset(x: u32, y: u32, row_bytes: u32, gob_rows: u32) -> usize {
    let block = gob_rows * 512;
    let block_row = row_bytes.div_ceil(64) * block;
    (y / (8 * gob_rows) * block_row + x / 64 * block + y / 8 % gob_rows * 512 + gob_offset(x, y)) as usize
}

fn gob_rows(height: u32, log2: u32) -> u32 {
    let mut g = 1 << log2;
    while g > 1 && height.div_ceil(8) <= g / 2 {
        g /= 2;
    }
    g
}

fn tiled_size(width: u32, height: u32, log2: u32) -> usize {
    let g = gob_rows(height, log2);
    ((width * 4).div_ceil(64) * height.div_ceil(8).div_ceil(g) * g * 512) as usize
}

pub fn tile(rgba: &[u8], width: u32, height: u32, log2: u32, out: &mut [u8]) {
    let g = gob_rows(height, log2);
    for y in 0..height {
        for x in 0..width * 4 {
            out[tiled_offset(x, y, width * 4, g)] = rgba[(y * width * 4 + x) as usize];
        }
    }
}

pub fn untile(tiled: &[u8], width: u32, height: u32, log2: u32) -> Vec<u8> {
    let g = gob_rows(height, log2);
    let mut out = vec![0; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width * 4 {
            out[(y * width * 4 + x) as usize] = tiled[tiled_offset(x, y, width * 4, g)];
        }
    }
    out
}

fn half(rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let px = |dx: u32, dy: u32| rgba[(((y * 2 + dy).min(h - 1) * w + (x * 2 + dx).min(w - 1)) * 4 + c) as usize] as u32;
                out[((y * nw + x) * 4 + c) as usize] = ((px(0, 0) + px(1, 0) + px(0, 1) + px(1, 1) + 2) / 4) as u8;
            }
        }
    }
    (out, nw, nh)
}

impl Texture {
    pub fn read(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.check()?;
        let at = u64_at(data, self.mip_offsets)?;
        let tiled = data.get(at..at + tiled_size(self.width, self.height, self.block_height_log2)).context("BNTX texture data cut short")?;
        Ok(untile(tiled, self.width, self.height, self.block_height_log2))
    }

    pub fn data_offset(&self, data: &[u8]) -> Result<usize> {
        u64_at(data, self.mip_offsets)
    }

    pub fn write(&self, data: &mut [u8], rgba: &[u8], width: u32, height: u32) -> Result<()> {
        self.check()?;
        ensure!((width, height) == (self.width, self.height), "{} is {}x{}, the image is {width}x{height}", self.name, self.width, self.height);
        let (mut img, mut w, mut h) = (rgba.to_vec(), width, height);
        for level in 0..self.mips.max(1) {
            let at = u64_at(data, self.mip_offsets + level as usize * 8)?;
            let size = tiled_size(w, h, self.block_height_log2);
            let dst = data.get_mut(at..at + size).context("BNTX texture data cut short")?;
            tile(&img, w, h, self.block_height_log2, dst);
            (img, w, h) = half(&img, w, h);
        }
        Ok(())
    }

    fn check(&self) -> Result<()> {
        if ![RGBA8_UNORM, RGBA8_SRGB].contains(&self.format) {
            bail!("{} uses texture format {:#x}, only RGBA8 textures can be replaced", self.name, self.format);
        }
        ensure!(self.tile_mode == 0 && self.depth <= 1 && self.layers <= 1, "{} is not a plain 2D texture", self.name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_round_trips_odd_sizes() {
        for (w, h, l) in [(64, 64, 3), (52, 52, 3), (128, 128, 4), (7, 33, 2)] {
            let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
            let mut tiled = vec![0; tiled_size(w, h, l)];
            tile(&rgba, w, h, l, &mut tiled);
            assert_eq!(untile(&tiled, w, h, l), rgba, "{w}x{h}");
        }
    }

    #[test]
    fn first_gob_is_swizzled() {
        let rgba: Vec<u8> = (0..16 * 8 * 4).map(|i| i as u8).collect();
        let mut tiled = vec![0; tiled_size(16, 8, 0)];
        tile(&rgba, 16, 8, 0, &mut tiled);
        assert_eq!(tiled[16], rgba[64]);
        assert_eq!(tiled[32], rgba[16]);
        assert_eq!(tiled[256], rgba[32]);
    }
}
