// Mush8 - exec.rs
// Constantin Clerc (c22dev) - 0.1
use super::state::Res;
use super::{Replay, UPPER_LEFT, ZERO_TO_ONE};
use crate::gfx;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::Relaxed;

fn rd<T: Copy>(addr: u64) -> T {
    unsafe { (addr as *const T).read_unaligned() }
}

impl Replay<'_> {
    pub(super) fn draw(&mut self, d: &super::Draw) -> Result<(), &'static str> {
        if d.count == 0 || d.instances == 0 || self.e.st.rasterizer_discard || !self.e.st.render_enable {
            return Ok(());
        }
        let st = self.e.st;
        let mut vs = None;
        let mut fs = None;
        for &(code, ctl) in st.stages() {
            let Some(s) = self.e.shaders.stage(code, ctl) else { continue };
            if s.fragment {
                fs = Some(s)
            } else if s.local_size.is_none() {
                vs = Some(s)
            }
        }
        let (Some(vs), Some(fs)) = (vs, fs) else { return Err("program without vertex+fragment stages") };
        self.ensure_pass()?;
        let targets = self.pass.as_ref().unwrap().targets.clone();
        let (mut uints, mut sints) = (0, 0);
        for (i, f) in targets.colors.iter().enumerate() {
            match f.map_or(0, super::shaders::integer_kind) {
                1 => uints |= 1 << i,
                2 => sints |= 1 << i,
                _ => {}
            }
        }
        use super::shaders::Lookup;
        let wait = self.pass.as_ref().unwrap().fresh;
        let fsc = match self.e.shaders.function(&fs, 0, uints, sints, wait) {
            Lookup::Ready(f) => f,
            Lookup::Pending => return Err("shader compiling"),
            Lookup::Failed => return Err("fragment shader failed"),
        };
        let extra = fsc.iface.attributes;
        let vsc = match self.e.shaders.function(&vs, extra, 0, 0, wait) {
            Lookup::Ready(f) => f,
            Lookup::Pending => return Err("shader compiling"),
            Lookup::Failed => return Err("vertex shader failed"),
        };
        let (w, h) = self.pass.as_ref().unwrap().size;
        let pipeline = self.e.states.pipeline(&vsc, (vs.hash, extra), &fsc, fs.hash ^ (uints as u64) << 48 ^ (sints as u64) << 56, &targets, &st, wait)?;
        let ds = self.e.states.depth_stencil(&st, targets.depth.is_some(), targets.stencil);

        let (sx, sy, sw, sh) = match st.scissor {
            Some([x, y, sw, sh]) => {
                let (x0, y0) = (x.max(0) as u32, y.max(0) as u32);
                let x1 = ((x as i64 + sw as i64).clamp(0, w as i64)) as u32;
                let y1 = ((y as i64 + sh as i64).clamp(0, h as i64)) as u32;
                if x1 <= x0 || y1 <= y0 {
                    return Ok(());
                }
                (x0, y0, x1 - x0, y1 - y0)
            }
            None => (0, 0, w, h),
        };
        let upper_left = UPPER_LEFT.load(Relaxed);

        self.heap = gfx::heap();
        let mut bound = Vec::new();
        for (stage, c) in [(0usize, &vsc), (1, &fsc)] {
            for (b, tex, slot, sampler) in self.stage_textures(&st, stage, c) {
                bound.push((stage == 1, b, tex, slot, sampler));
            }
        }

        let flags = (!upper_left) as u32 | ((!ZERO_TO_ONE.load(Relaxed)) as u32) << 1;
        let alpha = st.color_state >> 24 & 0xF;
        let mut res = [self.res(&st, 0, vs.constants, flags), self.res(&st, 1, fs.constants, flags)];
        if alpha != 0 && alpha != 8 {
            res[1].flags |= 4;
            res[1].alpha_func = alpha - 1;
        }
        let r = &mut res[0];
        for i in 0..16 {
            let a = st.attribs[i];
            if a & 0x40 == 0 {
                continue;
            }
            let stream = (a & 0x1F) as usize % 16;
            r.attr_format[i] = (a >> 21) & 0x3FF;
            r.attr_offset[i] = (a >> 7) & 0x3FFF;
            r.attr_stream[i] = stream as u32;
        }
        for s in 0..16 {
            r.vb[s] = if st.vbufs[s].0 != 0 { self.gpu_range(st.vbufs[s].0, st.vbufs[s].1, false) } else { gfx::gpu().zeros.0.gpuAddress() };
            r.vb_stride[s] = st.streams[s].0;
            r.vb_divisor[s] = st.streams[s].1;
        }
        r.base_instance = d.base_instance;

        let heap = self.heap;
        let pass = self.pass.as_mut().unwrap();
        if let Some(hb) = heap {
            if pass.heap != hb.chunks[0].gpu {
                for chunk in &hb.chunks {
                    pass.enc.useResource_usage_stages(ProtocolObject::from_ref(&*chunk.buffer.0), MTLResourceUsage::Read, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
                }
                pass.heap = hb.chunks[0].gpu;
            }
        }

        let prim = self.primitives(d)?;

        if super::TRACE_DRAWS.load(Relaxed) {
            let attrs: Vec<String> = (0..16).filter(|&i| st.attribs[i] & 0x40 != 0).map(|i| format!("{i}:{}", (st.attribs[i] >> 21) & 0x3FF)).collect();
            eprintln!("[frame]   attributes {attrs:?}");
            let rt: Vec<String> = self.e.colors.iter().map(|c| c.as_ref().and_then(|b| super::textures::TexKey::of(b)).map_or("-".into(), |k| format!("{:#x}/{}/{}x{}", k.addr, k.format, k.width, k.height))).collect();
            let tex: Vec<String> = bound.iter().map(|(f, b, t, _, _)| format!("{}{b}:{}x{}/{}", if *f { "f" } else { "v" }, t.0.width(), t.0.height(), t.0.pixelFormat().0)).collect();
            eprintln!(
                "[frame] draw rt {rt:?} vs {:016x} fs {:016x} poly {:#x} mode {} count {} inst {} vp {:?} sc {:?} ds {:#x} cs {:#x} mask {:#x} tex {tex:?}",
                vs.hash, fs.hash, st.polygon, d.mode, d.count, d.instances, st.viewport, st.scissor, st.depth_stencil, st.color_state, st.channel_mask
            );
        }
        if st.polygon & 3 == 3 {
            return Ok(());
        }
        let prev = self.pass.as_ref().unwrap().fixed;
        let next = {
            let enc = &self.pass.as_ref().unwrap().enc;
            bind_fixed(enc, prev, &pipeline, &ds, &st, upper_left, (w, h), [sx, sy, sw, sh])
        };
        self.pass.as_mut().unwrap().fixed = Some(next);
        let enc = &self.pass.as_ref().unwrap().enc;
        unsafe {
            enc.setVertexBytes_length_atIndex(NonNull::from(&res[0]).cast(), size_of::<Res>(), 0);
            enc.setFragmentBytes_length_atIndex(NonNull::from(&res[1]).cast(), size_of::<Res>(), 0);
            for (frag, b, tex, slot, sampler) in &bound {
                if *frag {
                    enc.setFragmentTexture_atIndex(Some(&tex.0), *b as usize);
                    enc.setFragmentSamplerState_atIndex(Some(&sampler.0), *slot as usize);
                } else {
                    enc.setVertexTexture_atIndex(Some(&tex.0), *b as usize);
                    enc.setVertexSamplerState_atIndex(Some(&sampler.0), *slot as usize);
                }
            }
            match prim {
                Prim::Direct(ty) => enc.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(ty, d.first as usize, d.count as usize, d.instances as usize, 0),
                Prim::Indexed { ty, itype, buffer, offset, count, base_vertex } => enc
                    .drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(
                        ty,
                        count,
                        itype,
                        &buffer.0,
                        offset,
                        d.instances as usize,
                        base_vertex as isize,
                        0,
                    ),
            }
        }
        Ok(())
    }

    fn stage_textures(&mut self, st: &super::GState, stage: usize, c: &super::shaders::Compiled) -> Vec<(u32, gfx::Shared<gfx::Texture>, u32, gfx::Shared<super::textures::Sampler>)> {
        let mut bound = Vec::with_capacity(c.iface.textures.len());
        let ids = |b: u32| {
            let handle = st.textures[stage].get(b as usize).copied().unwrap_or(0);
            ((handle & 0xF_FFFF) as u32, ((handle >> 20) & 0xFFF) as u32)
        };
        let descriptors: Vec<_> = {
            let s = super::STATE.lock().unwrap();
            c.iface
                .textures
                .keys()
                .map(|&b| {
                    let (tid, sid) = ids(b);
                    let tb = s.registered_textures.get(&(st.texture_pool, tid)).copied();
                    let key = tb.as_ref().map(|tb| super::textures::TexKey::of_in(tb, &s));
                    (tb, key, s.registered_samplers.get(&(st.sampler_pool, sid)).copied())
                })
                .collect()
        };
        for ((&b, &kind), (tb, key, sb)) in c.iface.textures.iter().zip(descriptors) {
            let (tid, _) = ids(b);
            let got = tb.and_then(|tb| self.e.textures.get_keyed(key.flatten(), &tb, false).map(|(k, t)| (k, t, tb)));
            if super::TRACE_DRAWS.load(Relaxed) {
                if let Some((k, t, _)) = got.as_ref().filter(|(_, t, _)| t.0.textureType() == MTLTextureType::Type2D) {
                    self.e.sampled.entry(*k).or_insert_with(|| t.clone());
                }
                let swz = tb.map(|b| [0x38usize, 0x3c, 0x40, 0x44].map(|o| u32::from_le_bytes(b[o..o + 4].try_into().unwrap())));
                eprintln!("[frame]   tex {}{b}: id {tid} pool {:#x} -> {:?} swizzle {swz:?}", ["v", "f", "c"][stage], st.texture_pool, got.as_ref().map(|(k, _, _)| (k.addr, k.format, k.width, k.height)));
            }
            let tex = got
                .and_then(|(k, t, tb)| self.e.textures.view(k, &tb, &t, kind))
                .filter(|t| !kind.is_depth() || gfx_is_depth(t.0.pixelFormat()))
                .unwrap_or_else(|| self.e.textures.dummy(kind));
            let sampler = self.e.textures.sampler(sb.as_ref());
            let slot = c.iface.samplers.get(&b).copied().unwrap_or(0);
            bound.push((b, tex, slot, sampler));
        }
        bound
    }

    pub(super) fn dispatch(&mut self, groups: [u32; 3]) -> Result<(), &'static str> {
        if groups.contains(&0) {
            return Ok(());
        }
        self.heap = gfx::heap();
        let st = self.e.st;
        let s = st.stages().iter().find_map(|&(code, ctl)| self.e.shaders.stage(code, ctl).filter(|s| s.local_size.is_some())).ok_or("dispatch without a compute stage")?;
        let c = match self.e.shaders.function(&s, 0, 0, 0, false) {
            super::shaders::Lookup::Ready(c) => c,
            super::shaders::Lookup::Pending => return Err("compute shader compiling"),
            super::shaders::Lookup::Failed => return Err("compute shader failed"),
        };
        let pipeline = match self.e.kernels.get(&s.hash) {
            Some(p) => p.clone(),
            None => {
                let p = gfx::gpu().device.0.newComputePipelineStateWithFunction_error(&c.func.0).map_err(|_| "compute pipeline failed")?;
                let p = gfx::Shared(p);
                self.e.kernels.insert(s.hash, p.clone());
                p
            }
        };
        let textures = self.stage_textures(&st, 2, &c);
        let res = self.res(&st, 2, s.constants, 0);
        let Some(enc) = self.cb.computeCommandEncoder() else { return Err("no compute encoder") };
        enc.setComputePipelineState(&pipeline.0);
        if let Some(h) = self.heap {
            for chunk in &h.chunks {
                enc.useResource_usage(ProtocolObject::from_ref(&*chunk.buffer.0), MTLResourceUsage::Read | MTLResourceUsage::Write);
            }
        }
        for b in &self.e.arena.used {
            enc.useResource_usage(ProtocolObject::from_ref(&*b.0), MTLResourceUsage::Read);
        }
        unsafe {
            enc.setBytes_length_atIndex(NonNull::from(&res).cast(), size_of::<Res>(), 0);
            for (b, tex, slot, sampler) in &textures {
                enc.setTexture_atIndex(Some(&tex.0), *b as usize);
                enc.setSamplerState_atIndex(Some(&sampler.0), *slot as usize);
            }
        }
        let [x, y, z] = s.local_size.unwrap().map(|n| n as usize);
        let [gx, gy, gz] = groups.map(|n| n as usize);
        enc.dispatchThreadgroups_threadsPerThreadgroup(MTLSize { width: gx, height: gy, depth: gz }, MTLSize { width: x, height: y, depth: z });
        enc.endEncoding();
        Ok(())
    }

    fn res(&mut self, st: &super::GState, stage: usize, constants: u64, flags: u32) -> Res {
        let zeros = gfx::gpu().zeros.0.gpuAddress();
        let mut r = Res {
            cb: [zeros; 18],
            vb: [zeros; 16],
            vb_stride: [0; 16],
            vb_divisor: [0; 16],
            attr_format: [0; 16],
            attr_stream: [0; 16],
            attr_offset: [0; 16],
            base_vertex: 0,
            base_instance: 0,
            flags,
            alpha_ref: 0.0,
            alpha_func: 7,
            heap_base: 0,
            heap_gpu: [0; gfx::MAX_HEAP_CHUNKS],
            heap_size: 0,
            heap_chunk_size: 0,
            zero_gpu: zeros,
        };
        if let Some(h) = self.heap {
            (r.heap_base, r.heap_gpu, r.heap_size, r.heap_chunk_size) = (h.base, h.gpu_addresses(), h.size, h.chunk_size);
        }
        if st.storage[stage].iter().any(|s| s.0 != 0) {
            let mut bank0 = [0u32; 0x100];
            for (i, &(addr, size)) in st.storage[stage].iter().enumerate() {
                let w = 0x44 + 4 * i;
                bank0[w] = addr as u32;
                bank0[w + 1] = (addr >> 32) as u32;
                bank0[w + 2] = size as u32;
            }
            let a = self.e.arena.alloc_bytes(unsafe { std::slice::from_raw_parts(bank0.as_ptr() as *const u8, 4 * bank0.len()) });
            r.cb[0] = a.gpu;
            if a.new_chunk {
                self.use_chunk(&a.buffer);
            }
        }
        r.cb[1] = self.gpu_range(constants, 0x10000, false);
        let handles = st.textures[stage];
        if self.e.bank_ok[stage] && self.e.bank_tex[stage] == handles {
            r.cb[2] = self.e.bank_gpu[stage];
        } else {
            let mut bank2 = [0u32; 0x48];
            for (i, h) in handles.iter().enumerate() {
                bank2[8 + 2 * i] = *h as u32;
                bank2[9 + 2 * i] = (*h >> 32) as u32;
            }
            let a = self.e.arena.alloc_bytes(unsafe { std::slice::from_raw_parts(bank2.as_ptr() as *const u8, 4 * bank2.len()) });
            r.cb[2] = a.gpu;
            if a.new_chunk {
                self.use_chunk(&a.buffer);
            }
            self.e.bank_tex[stage] = handles;
            self.e.bank_gpu[stage] = a.gpu;
            self.e.bank_ok[stage] = true;
        }
        for (i, &(addr, size)) in st.ubos[stage].iter().enumerate() {
            if addr != 0 {
                r.cb[3 + i] = self.gpu_range(addr, size, true);
            }
        }
        r
    }

    fn primitives(&mut self, d: &super::Draw) -> Result<Prim, &'static str> {
        let (ty, convert): (MTLPrimitiveType, Option<fn(&[u32]) -> Vec<u32>>) = match d.mode {
            0 => (MTLPrimitiveType::Point, None),
            1 => (MTLPrimitiveType::Line, None),
            2 => (MTLPrimitiveType::Line, Some(line_loop)),
            3 => (MTLPrimitiveType::LineStrip, None),
            4 => (MTLPrimitiveType::Triangle, None),
            5 => (MTLPrimitiveType::TriangleStrip, None),
            6 | 9 => (MTLPrimitiveType::Triangle, Some(fan)),
            7 => (MTLPrimitiveType::Triangle, Some(quads)),
            8 => (MTLPrimitiveType::Triangle, Some(quad_strip)),
            _ => return Err("unsupported primitive"),
        };
        let index_size = |t: u32| match t {
            0 => 1u64,
            1 => 2,
            _ => 4,
        };
        let read_indices = |t: u32, addr: u64, n: u32| -> Vec<u32> {
            (0..n as u64)
                .map(|i| match t {
                    0 => rd::<u8>(addr + i) as u32,
                    1 => rd::<u16>(addr + 2 * i) as u32,
                    _ => rd::<u32>(addr + 4 * i),
                })
                .collect()
        };
        let upload32 = |r: &mut Self, v: &[u32]| {
            let a = r.e.arena.alloc_bytes(unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, 4 * v.len()) });
            if a.new_chunk {
                r.use_chunk(&a.buffer);
            }
            (a.buffer, a.offset)
        };
        match (d.index, convert) {
            (None, None) => Ok(Prim::Direct(ty)),
            (None, Some(f)) => {
                let src: Vec<u32> = (d.first..d.first + d.count).collect();
                let v = f(&src);
                let (buffer, offset) = upload32(self, &v);
                Ok(Prim::Indexed { ty, itype: MTLIndexType::UInt32, buffer, offset, count: v.len(), base_vertex: 0 })
            }
            (Some((t, addr)), conv) => {
                let size = index_size(t);
                let heap = self.heap;
                let range = heap.and_then(|h|h.buffer_range(addr,size*d.count as u64));
                if let Some((chunk,off)) = range.filter(|_|conv.is_none() && t!=0 && addr%size==0) {
                    let itype = if t == 1 { MTLIndexType::UInt16 } else { MTLIndexType::UInt32 };
                    return Ok(Prim::Indexed { ty, itype, buffer: chunk.buffer.clone(), offset: off, count: d.count as usize, base_vertex: d.base_vertex });
                }
                let src = read_indices(t, addr, d.count);
                let v = match conv {
                    Some(f) => f(&src),
                    None => src,
                };
                let (buffer, offset) = upload32(self, &v);
                Ok(Prim::Indexed { ty, itype: MTLIndexType::UInt32, buffer, offset, count: v.len(), base_vertex: d.base_vertex })
            }
        }
    }
}

fn fbits(x: f32) -> u32 {
    x.to_bits()
}

fn bind_fixed(
    enc: &crate::gfx::RenderEncoder,
    prev: Option<super::EncState>,
    pipeline: &gfx::Shared<super::state::Pipeline>,
    ds: &gfx::Shared<super::state::DepthState>,
    st: &super::GState,
    upper_left: bool,
    target: (u32, u32),
    scissor: [u32; 4],
) -> super::EncState {
    let p = st.polygon;
    let cull = p & 3;
    let ccw = (p >> 2 & 1 != 0) == upper_left;
    let lines = p >> 3 & 3 == 1;
    let bias = if p >> 15 & (1 << (p >> 3 & 3)) != 0 { st.polygon_offset } else { [0.0; 3] };
    let (w, h) = target;
    let [vx, vy, vw, vh] = st.viewport.unwrap_or([0.0, 0.0, w as f32, h as f32]);
    let next = super::EncState {
        pipeline: std::ptr::from_ref(&*pipeline.0) as usize,
        depth: std::ptr::from_ref(&*ds.0) as usize,
        cull,
        winding: u32::from(ccw),
        fill: u32::from(lines),
        bias: bias.map(fbits),
        depth_clamp: st.depth_clamp,
        origin: [fbits(vx), fbits(vy), fbits(vw), fbits(vh)],
        zrange: [fbits(st.depth_range.0), fbits(st.depth_range.1)],
        scissor,
        stencil: st.stencil_ref,
        blend: st.blend_color.map(fbits),
    };
    if prev.is_none_or(|e| e.pipeline != next.pipeline) {
        enc.setRenderPipelineState(&pipeline.0);
    }
    if prev.is_none_or(|e| e.depth != next.depth) {
        enc.setDepthStencilState(Some(&ds.0));
    }
    if prev.is_none_or(|e| e.cull != next.cull) {
        enc.setCullMode(match cull {
            1 => MTLCullMode::Front,
            2 => MTLCullMode::Back,
            _ => MTLCullMode::None,
        });
    }
    if prev.is_none_or(|e| e.winding != next.winding) {
        enc.setFrontFacingWinding(if ccw { MTLWinding::CounterClockwise } else { MTLWinding::Clockwise });
    }
    if prev.is_none_or(|e| e.fill != next.fill) {
        enc.setTriangleFillMode(if lines { MTLTriangleFillMode::Lines } else { MTLTriangleFillMode::Fill });
    }
    if prev.is_none_or(|e| e.bias != next.bias) {
        enc.setDepthBias_slopeScale_clamp(bias[1], bias[0], bias[2]);
    }
    if prev.is_none_or(|e| e.depth_clamp != next.depth_clamp) {
        enc.setDepthClipMode(if st.depth_clamp { MTLDepthClipMode::Clamp } else { MTLDepthClipMode::Clip });
    }
    if prev.is_none_or(|e| e.origin != next.origin || e.zrange != next.zrange) {
        enc.setViewport(MTLViewport {
            originX: vx as f64,
            originY: vy as f64,
            width: vw as f64,
            height: vh as f64,
            znear: st.depth_range.0 as f64,
            zfar: st.depth_range.1 as f64,
        });
    }
    if prev.is_none_or(|e| e.scissor != next.scissor) {
        let [x, y, sw, sh] = scissor;
        enc.setScissorRect(MTLScissorRect { x: x as usize, y: y as usize, width: sw as usize, height: sh as usize });
    }
    if prev.is_none_or(|e| e.stencil != next.stencil) {
        enc.setStencilFrontReferenceValue_backReferenceValue(st.stencil_ref[0], st.stencil_ref[1]);
    }
    if prev.is_none_or(|e| e.blend != next.blend) {
        let c = st.blend_color;
        enc.setBlendColorRed_green_blue_alpha(c[0], c[1], c[2], c[3]);
    }
    next
}

fn gfx_is_depth(f: MTLPixelFormat) -> bool {
    matches!(f, MTLPixelFormat::Depth16Unorm | MTLPixelFormat::Depth32Float | MTLPixelFormat::Depth32Float_Stencil8)
}

pub(super) enum Prim {
    Direct(MTLPrimitiveType),
    Indexed { ty: MTLPrimitiveType, itype: MTLIndexType, buffer: gfx::Shared<gfx::Buffer>, offset: usize, count: usize, base_vertex: i32 },
}

fn line_loop(v: &[u32]) -> Vec<u32> {
    let mut o = Vec::with_capacity(v.len() * 2);
    for i in 0..v.len() {
        o.extend([v[i], v[(i + 1) % v.len()]]);
    }
    o
}

fn fan(v: &[u32]) -> Vec<u32> {
    (1..v.len().saturating_sub(1)).flat_map(|i| [v[0], v[i], v[i + 1]]).collect()
}

fn quads(v: &[u32]) -> Vec<u32> {
    v.chunks_exact(4).flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect()
}

fn quad_strip(v: &[u32]) -> Vec<u32> {
    (0..v.len().saturating_sub(2) / 2).flat_map(|i| {
        let q = &v[2 * i..2 * i + 4];
        [q[0], q[1], q[3], q[0], q[3], q[2]]
    }).collect()
}
