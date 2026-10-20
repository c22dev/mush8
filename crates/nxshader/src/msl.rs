// Mush8 - msl.rs
// Constantin Clerc (c22dev) - 0.1
use crate::inst::*;
use crate::program::{Inst, Program, branch_target};
use crate::sph::{Imap, Sph, Stage};
use crate::table::{InstName, Op};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TexKind {
    T2D,
    T2DArray,
    T3D,
    Cube,
    CubeArray,
    Depth2D,
    Depth2DArray,
    DepthCube,
}

impl TexKind {
    pub fn decl(self) -> &'static str {
        match self {
            TexKind::T2D => "texture2d<float>",
            TexKind::T2DArray => "texture2d_array<float>",
            TexKind::T3D => "texture3d<float>",
            TexKind::Cube => "texturecube<float>",
            TexKind::CubeArray => "texturecube_array<float>",
            TexKind::Depth2D => "depth2d<float>",
            TexKind::Depth2DArray => "depth2d_array<float>",
            TexKind::DepthCube => "depthcube<float>",
        }
    }
    pub fn is_depth(self) -> bool {
        matches!(self, TexKind::Depth2D | TexKind::Depth2DArray | TexKind::DepthCube)
    }
    fn coords(self) -> usize {
        match self {
            TexKind::T3D | TexKind::Cube | TexKind::CubeArray | TexKind::DepthCube => 3,
            _ => 2,
        }
    }
    pub fn is_array(self) -> bool {
        matches!(self, TexKind::T2DArray | TexKind::CubeArray | TexKind::Depth2DArray)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Interface {

    pub textures: BTreeMap<u32, TexKind>,

    pub samplers: BTreeMap<u32, u32>,

    pub attributes: u32,

    pub outputs: u32,

    pub color_targets: u32,
    pub writes_depth: bool,
}

pub struct Translated {
    pub stage: Stage,
    pub msl: String,
    pub entry: String,
    pub interface: Interface,
}

impl Translated {

    pub fn named(&self, suffix: &str) -> (String, String) {
        let mut src = self.msl[PRELUDE.len()..].to_string();
        for name in ["VOut", "FIn", "FOut", self.entry.as_str()] {
            src = rename(&src, name, &format!("{name}_{suffix}"));
        }
        (src, format!("{}_{suffix}", self.entry))
    }
}

pub fn prelude() -> &'static str {
    PRELUDE
}

fn rename(src: &str, from: &str, to: &str) -> String {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find(from) {
        let (before, after) = (rest[..i].chars().next_back(), rest[i + from.len()..].chars().next());
        out.push_str(&rest[..i]);
        out.push_str(if word(before) || word(after) { from } else { to });
        rest = &rest[i + from.len()..];
    }
    out.push_str(rest);
    out
}

const ATTR_POSITION: i32 = 0x70;
const ATTR_GENERIC: i32 = 0x80;
const ATTR_GENERIC_END: i32 = 0x280;
const ATTR_POINT_SIZE: i32 = 0x6c;
const ATTR_POINT_COORD: i32 = 0x2e0;
const ATTR_INSTANCE_ID: i32 = 0x2f8;
const ATTR_VERTEX_ID: i32 = 0x2fc;
const ATTR_FRONT_FACING: i32 = 0x3fc;

struct Gen<'a> {
    sph: &'a Sph,
    prog: &'a Program,
    body: String,
    regs: BTreeSet<i32>,
    preds: BTreeSet<i32>,
    uses_cc: bool,
    uses_lmem: bool,
    uses_point_coord: bool,
    iface: Interface,
    warnings: Vec<String>,

    handle_regs: BTreeMap<i32, u32>,
}

const RZ_: i32 = 255;

const TEXTURE_HANDLE_BANK: i32 = 2;

fn fimm(bits: u32) -> String {
    format!("as_type<float>({bits:#x}u)")
}

impl<'a> Gen<'a> {
    fn line(&mut self, s: impl AsRef<str>) {
        self.body.push_str("        ");
        self.body.push_str(s.as_ref());
        self.body.push('\n');
    }

    fn r(&mut self, n: i32) -> String {
        if n == RZ_ {
            "0u".into()
        } else {
            self.regs.insert(n);
            format!("r{n}")
        }
    }
    fn fr(&mut self, n: i32) -> String {
        if n == RZ_ { "0.0f".into() } else { format!("as_type<float>({})", self.r(n)) }
    }
    fn cb(&mut self, slot: i32, word: i32) -> String {
        format!("res.cb[{slot}][{word}]")
    }
    fn fcb(&mut self, slot: i32, word: i32) -> String {
        format!("as_type<float>({})", self.cb(slot, word))
    }
    fn set(&mut self, d: i32, expr: String) {
        if d != RZ_ {
            self.regs.insert(d);
            self.line(format!("r{d} = {expr};"));
        }
    }
    fn setf(&mut self, d: i32, fexpr: String) {
        self.set(d, format!("as_type<uint>({fexpr})"));
    }
    fn p(&mut self, n: i32, inv: bool) -> String {
        let base = if n == 7 {
            "true".to_string()
        } else {
            self.preds.insert(n);
            format!("p{n}")
        };
        if inv { format!("(!{base})") } else { base }
    }
    fn setp(&mut self, n: i32, expr: String) {
        if n != 7 {
            self.preds.insert(n);
            self.line(format!("p{n} = {expr};"));
        }
    }
    fn fmod(x: String, abs: bool, neg: bool) -> String {
        let x = if abs { format!("fabs({x})") } else { x };
        if neg { format!("(-{x})") } else { x }
    }
    fn sat(x: String, sat: bool) -> String {
        if sat { format!("saturate({x})") } else { x }
    }
    fn set_zn(&mut self, d: String) {
        self.uses_cc = true;
        self.line(format!("zf = ({d}) == 0u; nf = int({d}) < 0;"));
    }

    fn label_target(&mut self, addr: u32) {
        self.line(format!("pc = {addr}u; continue;"));
    }

    fn attr_load(&mut self, addr: i32, output: bool) -> String {
        match (self.sph.stage, addr) {
            (Stage::Vertex, a) if output => self.vs_output_ref(a).map_or("0u".into(), |f| format!("as_type<uint>({f})")),
            (Stage::Vertex, ATTR_GENERIC..ATTR_GENERIC_END) => {
                let (i, c) = ((addr - ATTR_GENERIC) / 16, (addr - ATTR_GENERIC) / 4 % 4);
                self.iface.attributes |= 1 << i;
                format!("a{i}[{c}]")
            }
            (Stage::Vertex, ATTR_INSTANCE_ID) => "iid".into(),
            (Stage::Vertex, ATTR_VERTEX_ID) => "vid".into(),
            (Stage::Fragment, a) => self.fs_input(a, false),
            _ => {
                self.warnings.push(format!("attribute load {addr:#x}"));
                "0u".into()
            }
        }
    }

    fn vs_output_ref(&mut self, addr: i32) -> Option<String> {
        let c = ["x", "y", "z", "w"];
        Some(match addr {
            ATTR_POSITION..=0x7c => format!("o.pos.{}", c[((addr - ATTR_POSITION) / 4) as usize]),
            ATTR_GENERIC..ATTR_GENERIC_END => {
                let (i, k) = ((addr - ATTR_GENERIC) / 16, (addr - ATTR_GENERIC) / 4 % 4);
                self.iface.outputs |= 1 << i;
                format!("o.v{i}.{}", c[k as usize])
            }
            ATTR_POINT_SIZE => "o.psize".into(),
            _ => return None,
        })
    }

    fn fs_input(&mut self, addr: i32, perspective_scale: bool) -> String {
        let c = ["x", "y", "z", "w"];
        match addr {
            ATTR_POSITION..=0x7c => format!("as_type<uint>(in.pos.{})", c[((addr - ATTR_POSITION) / 4) as usize]),
            ATTR_GENERIC..ATTR_GENERIC_END => {
                let (i, k) = ((addr - ATTR_GENERIC) / 16, (addr - ATTR_GENERIC) / 4 % 4);
                self.iface.attributes |= 1 << i;
                let v = format!("in.v{i}.{}", c[k as usize]);

                if perspective_scale && self.sph.imap[i as usize] == Imap::Perspective {
                    format!("as_type<uint>({v} * in.pos.w)")
                } else {
                    format!("as_type<uint>({v})")
                }
            }
            ATTR_FRONT_FACING => "(front ? 0xffffffffu : 0u)".into(),
            ATTR_POINT_COORD | 0x2e4 => {
                self.uses_point_coord = true;
                format!("as_type<uint>(pcoord.{})", if addr == ATTR_POINT_COORD { "x" } else { "y" })
            }
            _ => {
                self.warnings.push(format!("fragment input {addr:#x}"));
                "0u".into()
            }
        }
    }

    fn tex(&mut self, binding: i32, kind: TexKind) -> u32 {
        let b = ((binding - 8) / 2).max(0) as u32;
        let e = self.iface.textures.entry(b).or_insert(kind);
        if *e != kind {
            self.warnings.push(format!("texture binding {b} used as {:?} and {kind:?}", *e));
        }
        b
    }

    fn write_components(&mut self, value: &str, mask: u32, dests: &[i32]) {
        let mut k = 0;
        for c in 0..4 {
            if mask >> c & 1 == 0 {
                continue;
            }
            if let Some(&d) = dests.get(k) {
                self.set(d, format!("as_type<uint>({value}[{c}])"));
            }
            k += 1;
        }
    }

    fn coord(&mut self, regs: &[i32], n: usize) -> String {
        let v: Vec<String> = regs.iter().take(n).map(|&r| self.fr(r)).collect();
        match n {
            1 => v[0].clone(),
            2 => format!("float2({}, {})", v[0], v[1]),
            _ => format!("float3({}, {}, {})", v[0], v[1], v[2]),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sample(&mut self, b: u32, kind: TexKind, coords: &[i32], array: Option<i32>, lod: Option<String>, bias: Option<String>, depth_ref: Option<i32>) -> String {
        let n = kind.coords();
        let c = self.coord(coords, n);

        let arr = array.map(|a| format!(", ({} & 0xffffu)", self.r(a)));
        let level = match (&lod, &bias) {
            (Some(l), _) => format!(", level({l})"),
            (None, Some(bb)) => format!(", bias({bb})"),
            _ => String::new(),
        };
        let arr = arr.unwrap_or_default();
        if let Some(rf) = depth_ref {
            let rv = self.fr(rf);
            format!("float4(t{b}.sample_compare(s{b}, {c}{arr}, {rv}{level}))")
        } else if kind.is_depth() {
            format!("float4(t{b}.sample(s{b}, {c}{arr}{level}))")
        } else {
            format!("t{b}.sample(s{b}, {c}{arr}{level})")
        }
    }

    fn inst(&mut self, i: &Inst) -> Result<()> {
        let w = i.raw;
        macro_rules! op {
            ($t:ident) => {
                $t(w)
            };
        }

        let fb_r = |g: &mut Self, r: i32| g.fr(r);
        match i.op {
            Op::Invalid => bail!("invalid instruction {w:016x} at {:#x}", i.addr),
            Op::Nop | Op::Depbar | Op::Ssy | Op::Pbk | Op::Membar | Op::Bar => {}

            Op::FaddR | Op::FaddI | Op::FaddC | Op::Fadd32i => {
                let (d, a, b, abs_a, abs_b, neg_a, neg_b, s) = match i.op {
                    Op::FaddR => {
                        let o = op!(InstFaddR);
                        (o.dest(), self.fr(o.src_a()), fb_r(self, o.src_b()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.sat())
                    }
                    Op::FaddI => {
                        let o = op!(InstFaddI);
                        (o.dest(), self.fr(o.src_a()), fimm((o.imm20() as u32) << 12), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.sat())
                    }
                    Op::FaddC => {
                        let o = op!(InstFaddC);
                        (o.dest(), self.fr(o.src_a()), self.fcb(o.cbuf_slot(), o.cbuf_offset()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.sat())
                    }
                    _ => {
                        let o = op!(InstFadd32i);
                        (o.dest(), self.fr(o.src_a()), fimm(o.imm32() as u32), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), false)
                    }
                };
                let e = Self::sat(format!("({} + {})", Self::fmod(a, abs_a, neg_a), Self::fmod(b, abs_b, neg_b)), s);
                self.setf(d, e);
            }
            Op::FmulR | Op::FmulI | Op::FmulC | Op::Fmul32i => {
                let (d, a, b, neg_a, scale, s) = match i.op {
                    Op::FmulR => {
                        let o = op!(InstFmulR);
                        (o.dest(), self.fr(o.src_a()), fb_r(self, o.src_b()), o.neg_a(), o.scale().0, o.sat())
                    }
                    Op::FmulI => {
                        let o = op!(InstFmulI);
                        (o.dest(), self.fr(o.src_a()), fimm((o.imm20() as u32) << 12), o.neg_a(), o.scale().0, o.sat())
                    }
                    Op::FmulC => {
                        let o = op!(InstFmulC);
                        (o.dest(), self.fr(o.src_a()), self.fcb(o.cbuf_slot(), o.cbuf_offset()), o.neg_a(), o.scale().0, o.sat())
                    }
                    _ => {
                        let o = op!(InstFmul32i);
                        (o.dest(), self.fr(o.src_a()), fimm(o.imm32() as u32), false, 0, o.sat())
                    }
                };
                let k = ["", " * 0.5f", " * 0.25f", " * 0.125f", " * 8.0f", " * 4.0f", " * 2.0f"].get(scale as usize).copied().unwrap_or("");
                let e = Self::sat(format!("({} * {}{k})", Self::fmod(a, false, neg_a), b), s);
                self.setf(d, e);
            }
            Op::FfmaR | Op::FfmaI | Op::FfmaC | Op::FfmaRc | Op::Ffma32i => {
                let (d, a, b, c, neg_a, neg_c, s) = match i.op {
                    Op::FfmaR => {
                        let o = op!(InstFfmaR);
                        (o.dest(), self.fr(o.src_a()), self.fr(o.src_b()), self.fr(o.src_c()), o.neg_a(), o.neg_c(), o.sat())
                    }
                    Op::FfmaI => {
                        let o = op!(InstFfmaI);
                        (o.dest(), self.fr(o.src_a()), fimm((o.imm20() as u32) << 12), self.fr(o.src_c()), o.neg_a(), o.neg_c(), o.sat())
                    }
                    Op::FfmaC => {
                        let o = op!(InstFfmaC);
                        (o.dest(), self.fr(o.src_a()), self.fcb(o.cbuf_slot(), o.cbuf_offset()), self.fr(o.src_c()), o.neg_a(), o.neg_c(), o.sat())
                    }
                    Op::FfmaRc => {
                        let o = op!(InstFfmaRc);
                        (o.dest(), self.fr(o.src_a()), self.fr(o.src_c()), self.fcb(o.cbuf_slot(), o.cbuf_offset()), o.neg_a(), o.neg_c(), o.sat())
                    }
                    _ => {
                        let o = op!(InstFfma32i);
                        (o.dest(), self.fr(o.src_a()), fimm(o.imm32() as u32), self.fr(o.dest()), o.neg_a(), o.neg_c(), o.sat())
                    }
                };
                let e = Self::sat(format!("fma({}, {b}, {})", Self::fmod(a, false, neg_a), Self::fmod(c, false, neg_c)), s);
                self.setf(d, e);
            }
            Op::FmnmxR | Op::FmnmxI | Op::FmnmxC => {
                let (d, a, b, abs_a, abs_b, neg_a, neg_b, sp, spi) = match i.op {
                    Op::FmnmxR => {
                        let o = op!(InstFmnmxR);
                        (o.dest(), self.fr(o.src_a()), self.fr(o.src_b()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.src_pred(), o.src_pred_inv())
                    }
                    Op::FmnmxI => {
                        let o = op!(InstFmnmxI);
                        (o.dest(), self.fr(o.src_a()), fimm((o.imm20() as u32) << 12), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.src_pred(), o.src_pred_inv())
                    }
                    _ => {
                        let o = op!(InstFmnmxC);
                        (o.dest(), self.fr(o.src_a()), self.fcb(o.cbuf_slot(), o.cbuf_offset()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.src_pred(), o.src_pred_inv())
                    }
                };
                let (a, b) = (Self::fmod(a, abs_a, neg_a), Self::fmod(b, abs_b, neg_b));
                let p = self.p(sp, spi);
                self.setf(d, format!("({p} ? fmin({a}, {b}) : fmax({a}, {b}))"));
            }
            Op::Mufu => {
                let o = op!(InstMufu);
                let a = Self::fmod(self.fr(o.src_a()), o.abs_a(), o.neg_a());
                let e = match o.mufu_op() {
                    MufuOp::Cos => format!("cos({a})"),
                    MufuOp::Sin => format!("sin({a})"),
                    MufuOp::Ex2 => format!("exp2({a})"),
                    MufuOp::Lg2 => format!("log2({a})"),
                    MufuOp::Rcp | MufuOp::Rcp64h => format!("(1.0f / {a})"),
                    MufuOp::Rsq | MufuOp::Rsq64h => format!("rsqrt({a})"),
                    MufuOp::Sqrt => format!("sqrt({a})"),
                    m => bail!("mufu op {m:?}"),
                };
                self.setf(o.dest(), Self::sat(e, o.sat()));
            }
            Op::RroR | Op::RroC => {
                let (d, b, abs_b, neg_b) = if i.op == Op::RroR {
                    let o = op!(InstRroR);
                    (o.dest(), self.fr(o.src_b()), o.abs_b(), o.neg_b())
                } else {
                    let o = op!(InstRroC);
                    (o.dest(), self.fcb(o.cbuf_slot(), o.cbuf_offset()), o.abs_b(), o.neg_b())
                };
                self.setf(d, Self::fmod(b, abs_b, neg_b));
            }
            Op::FsetpR | Op::FsetpI | Op::FsetpC => {
                let (dp, dpi, a, b, aa, ab, na, nb, cmp, sp, spi, bop) = match i.op {
                    Op::FsetpR => {
                        let o = op!(InstFsetpR);
                        (o.dest_pred(), o.dest_pred_inv(), self.fr(o.src_a()), self.fr(o.src_b()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.f_comp().0, o.src_pred(), o.src_pred_inv(), o.bop().0)
                    }
                    Op::FsetpI => {
                        let o = op!(InstFsetpI);
                        (o.dest_pred(), o.dest_pred_inv(), self.fr(o.src_a()), fimm((o.imm20() as u32) << 12), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.f_comp().0, o.src_pred(), o.src_pred_inv(), o.bop().0)
                    }
                    _ => {
                        let o = op!(InstFsetpC);
                        (o.dest_pred(), o.dest_pred_inv(), self.fr(o.src_a()), self.fcb(o.cbuf_slot(), o.cbuf_offset()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.f_comp().0, o.src_pred(), o.src_pred_inv(), o.bop().0)
                    }
                };
                let c = format!("fcmp({cmp}u, {}, {})", Self::fmod(a, aa, na), Self::fmod(b, ab, nb));
                let p = self.p(sp, spi);
                self.line(format!("{{ bool c = {c};"));
                self.setp(dp, format!("bop({bop}u, c, {p})"));
                self.setp(dpi, format!("bop({bop}u, !c, {p})"));
                self.line("}");
            }
            Op::FsetR | Op::FsetI | Op::FsetC => {
                let (d, a, b, aa, ab, na, nb, cmp, sp, spi, bop) = match i.op {
                    Op::FsetR => {
                        let o = op!(InstFsetR);
                        (o.dest(), self.fr(o.src_a()), self.fr(o.src_b()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.f_comp().0, o.src_pred(), o.src_pred_inv(), o.bop().0)
                    }
                    Op::FsetI => {
                        let o = op!(InstFsetI);
                        (o.dest(), self.fr(o.src_a()), fimm((o.imm20() as u32) << 12), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.f_comp().0, o.src_pred(), o.src_pred_inv(), o.bop().0)
                    }
                    _ => {
                        let o = op!(InstFsetC);
                        (o.dest(), self.fr(o.src_a()), self.fcb(o.cbuf_slot(), o.cbuf_offset()), o.abs_a(), o.abs_b(), o.neg_a(), o.neg_b(), o.f_comp().0, o.src_pred(), o.src_pred_inv(), o.bop().0)
                    }
                };
                let bf = (w >> 52) & 1 != 0;
                let p = self.p(sp, spi);
                let c = format!("bop({bop}u, fcmp({cmp}u, {}, {}), {p})", Self::fmod(a, aa, na), Self::fmod(b, ab, nb));
                let v = if bf { format!("({c} ? 0x3f800000u : 0u)") } else { format!("({c} ? 0xffffffffu : 0u)") };
                self.set(d, v);
            }
            Op::Fswzadd => {
                let o = op!(InstFswzadd);
                let (a, b) = (self.fr(o.src_a()), self.fr(o.src_b()));
                self.setf(o.dest(), format!("({a} + {b})"));
            }

            Op::Hadd2I => {
                let o = op!(InstHadd2I);
                let a = self.r(o.src_a());
                let imm = (((o.bimm_h1() as u32) << 6) << 16) | ((o.bimm_h0() as u32) << 6);
                self.set(o.dest(), format!("as_type<uint>(as_type<half2>({a}) + as_type<half2>({imm:#x}u))"));
            }
            Op::Hsetp2R => {
                let o = op!(InstHsetp2R);
                let (a, b) = (self.r(o.src_a()), self.r(o.src_b()));
                let p = self.p(o.src_pred(), o.src_pred_inv());
                let cmp = o.f_comp2().0;
                self.line(format!("{{ half2 ha = as_type<half2>({a}), hb = as_type<half2>({b}); bool c0 = fcmp({cmp}u, float(ha.x), float(hb.x)), c1 = fcmp({cmp}u, float(ha.y), float(hb.y));"));
                let bop = o.bop().0;
                if o.h_and() {
                    self.setp(o.dest_pred(), format!("bop({bop}u, c0 && c1, {p})"));
                    self.setp(o.dest_pred_inv(), format!("bop({bop}u, !(c0 && c1), {p})"));
                } else {
                    self.setp(o.dest_pred(), format!("bop({bop}u, c0, {p})"));
                    self.setp(o.dest_pred_inv(), format!("bop({bop}u, c1, {p})"));
                }
                self.line("}");
            }

            Op::IaddR | Op::IaddI | Op::IaddC | Op::Iadd32i => {
                let (d, a, b, avg, x, cc) = match i.op {
                    Op::IaddR => {
                        let o = op!(InstIaddR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.avg_mode().0, o.x(), o.write_cc())
                    }
                    Op::IaddI => {
                        let o = op!(InstIaddI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.avg_mode().0, o.x(), o.write_cc())
                    }
                    Op::IaddC => {
                        let o = op!(InstIaddC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.avg_mode().0, o.x(), o.write_cc())
                    }
                    _ => {
                        let o = op!(InstIadd32i);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", o.imm32() as u32), o.avg_mode().0, o.x(), o.write_cc())
                    }
                };
                let a = if avg == 2 { format!("(0u - {a})") } else { a };
                let b = if avg == 1 { format!("(0u - {b})") } else { b };
                let one = if avg == 3 { " + 1u" } else { "" };
                let carry = if x {
                    self.uses_cc = true;
                    " + uint(cf)"
                } else {
                    ""
                };
                if cc {
                    self.uses_cc = true;
                    self.line(format!("{{ ulong t = ulong({a}) + ulong({b}){one}{carry}; cf = (t >> 32) != 0ul; uint rr = uint(t); vf = (int((rr ^ {a}) & ~({a} ^ {b}))) < 0; zf = rr == 0u; nf = int(rr) < 0;"));
                    self.set(d, "rr".into());
                    self.line("}");
                } else {
                    self.set(d, format!("({a} + {b}{one}{carry})"));
                }
            }
            Op::Iadd3R | Op::Iadd3I | Op::Iadd3C => {
                let (d, a, b, c, na, nb, nc, lrs, ap, bp, cp) = match i.op {
                    Op::Iadd3R => {
                        let o = op!(InstIadd3R);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), self.r(o.src_c()), o.neg_a(), o.neg_b(), o.neg_c(), o.lrs().0, o.apart().0, o.bpart().0, o.cpart().0)
                    }
                    Op::Iadd3I => {
                        let o = op!(InstIadd3I);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), self.r(o.src_c()), o.neg_a(), o.neg_b(), o.neg_c(), 0, 0, 0, 0)
                    }
                    _ => {
                        let o = op!(InstIadd3C);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), self.r(o.src_c()), o.neg_a(), o.neg_b(), o.neg_c(), 0, 0, 0, 0)
                    }
                };
                let part = |x: String, p: u32| match p {
                    1 => format!("({x} & 0xffffu)"),
                    2 => format!("({x} >> 16)"),
                    _ => x,
                };
                let ng = |x: String, n: bool| if n { format!("(0u - {x})") } else { x };
                let (a, b, c) = (ng(part(a, ap), na), ng(part(b, bp), nb), ng(part(c, cp), nc));
                let ab = match lrs {
                    1 => format!("(({a} + {b}) >> 16)"),
                    2 => format!("(({a} + {b}) << 16)"),
                    _ => format!("({a} + {b})"),
                };
                self.set(d, format!("({ab} + {c})"));
            }
            Op::IscaddR | Op::IscaddI | Op::IscaddC => {
                let (d, a, b, sh, avg) = match i.op {
                    Op::IscaddR => {
                        let o = op!(InstIscaddR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.imm5(), o.avg_mode().0)
                    }
                    Op::IscaddI => {
                        let o = op!(InstIscaddI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.imm5(), o.avg_mode().0)
                    }
                    _ => {
                        let o = op!(InstIscaddC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.imm5(), o.avg_mode().0)
                    }
                };
                let a = if avg == 2 { format!("(0u - {a})") } else { a };
                let b = if avg == 1 { format!("(0u - {b})") } else { b };
                self.set(d, format!("(({a} << {sh}u) + {b})"));
            }
            Op::XmadR | Op::XmadI | Op::XmadC | Op::XmadRc => {
                let (d, a, b, c, cop, sa, sb, ha, hb, psl, mrg, x, write_cc) = match i.op {
                    Op::XmadR => {
                        let o = op!(InstXmadR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), self.r(o.src_c()), o.xmad_cop().0, o.a_signed(), o.b_signed(), o.hilo_a(), o.hilo_b(), o.psl(), o.mrg(), o.x(), o.write_cc())
                    }
                    Op::XmadI => {
                        let o = op!(InstXmadI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", o.imm16() as u32 & 0xffff), self.r(o.src_c()), o.xmad_cop().0, o.a_signed(), o.b_signed(), o.hilo_a(), false, o.psl(), o.mrg(), o.x(), o.write_cc())
                    }
                    Op::XmadC => {
                        let o = op!(InstXmadC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), self.r(o.src_c()), o.xmad_cop().0, o.a_signed(), o.b_signed(), o.hilo_a(), o.hilo_b(), o.psl(), o.mrg(), o.x(), o.write_cc())
                    }
                    _ => {
                        let o = op!(InstXmadRc);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_c()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.xmad_cop().0, o.a_signed(), o.b_signed(), o.hilo_a(), o.hilo_b(), false, false, o.x(), o.write_cc())
                    }
                };
                let carry = if x {
                    self.uses_cc = true;
                    "uint(cf)"
                } else {
                    "0xffffffffu"
                };
                let v = format!("xmad({a}, {b}, {c}, {cop}u, {sa}, {sb}, {ha}, {hb}, {psl}, {mrg}, {carry})");
                if write_cc {

                    self.uses_cc = true;
                    let p = format!("xmad({a}, {b}, 0u, 0u, {sa}, {sb}, {ha}, {hb}, {psl}, false, 0xffffffffu)");
                    self.line(format!("{{ uint xr = {v}; uint xp = {p}; uint xc = {c}; cf = xr < xp; vf = int((xr ^ xp) & ~(xp ^ xc)) < 0; zf = xr == 0u; nf = int(xr) < 0;"));
                    self.set(d, "xr".into());
                    self.line("}");
                } else {
                    self.set(d, v);
                }
            }
            Op::ImnmxR | Op::ImnmxI | Op::ImnmxC => {
                let (d, a, b, s, sp, spi) = match i.op {
                    Op::ImnmxR => {
                        let o = op!(InstImnmxR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.signed(), o.src_pred(), o.src_pred_inv())
                    }
                    Op::ImnmxI => {
                        let o = op!(InstImnmxI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.signed(), o.src_pred(), o.src_pred_inv())
                    }
                    _ => {
                        let o = op!(InstImnmxC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.signed(), o.src_pred(), o.src_pred_inv())
                    }
                };
                let p = self.p(sp, spi);
                let e = if s {
                    format!("({p} ? uint(min(int({a}), int({b}))) : uint(max(int({a}), int({b}))))")
                } else {
                    format!("({p} ? min({a}, {b}) : max({a}, {b}))")
                };
                self.set(d, e);
            }
            Op::IsetpR | Op::IsetpI | Op::IsetpC => {
                let (a, b, cmp, s, bop, sp, spi, dp, dpi) = match i.op {
                    Op::IsetpR => {
                        let o = op!(InstIsetpR);
                        (self.r(o.src_a()), self.r(o.src_b()), o.i_comp().0, o.signed(), o.bop().0, o.src_pred(), o.src_pred_inv(), o.dest_pred(), o.dest_pred_inv())
                    }
                    Op::IsetpI => {
                        let o = op!(InstIsetpI);
                        (self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.i_comp().0, o.signed(), o.bop().0, o.src_pred(), o.src_pred_inv(), o.dest_pred(), o.dest_pred_inv())
                    }
                    _ => {
                        let o = op!(InstIsetpC);
                        (self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.i_comp().0, o.signed(), o.bop().0, o.src_pred(), o.src_pred_inv(), o.dest_pred(), o.dest_pred_inv())
                    }
                };
                let p = self.p(sp, spi);
                self.line(format!("{{ bool c = icmp({cmp}u, {a}, {b}, {s});"));
                self.setp(dp, format!("bop({bop}u, c, {p})"));
                self.setp(dpi, format!("bop({bop}u, !c, {p})"));
                self.line("}");
            }
            Op::IsetR | Op::IsetI | Op::IsetC => {
                let (d, a, b, cmp, s, bop, sp, spi, bv) = match i.op {
                    Op::IsetR => {
                        let o = op!(InstIsetR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.i_comp().0, o.signed(), o.bop().0, o.src_pred(), o.src_pred_inv(), o.b_val())
                    }
                    Op::IsetI => {
                        let o = op!(InstIsetI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.i_comp().0, o.signed(), o.bop().0, o.src_pred(), o.src_pred_inv(), o.b_val())
                    }
                    _ => {
                        let o = op!(InstIsetC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.i_comp().0, o.signed(), o.bop().0, o.src_pred(), o.src_pred_inv(), o.b_val())
                    }
                };
                let p = self.p(sp, spi);
                let c = format!("bop({bop}u, icmp({cmp}u, {a}, {b}, {s}), {p})");
                self.set(d, if bv { format!("({c} ? 0x3f800000u : 0u)") } else { format!("({c} ? 0xffffffffu : 0u)") });
            }
            Op::SelR | Op::SelI | Op::SelC => {
                let (d, a, b, sp, spi) = match i.op {
                    Op::SelR => {
                        let o = op!(InstSelR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.src_pred(), o.src_pred_inv())
                    }
                    Op::SelI => {
                        let o = op!(InstSelI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.src_pred(), o.src_pred_inv())
                    }
                    _ => {
                        let o = op!(InstSelC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.src_pred(), o.src_pred_inv())
                    }
                };
                let p = self.p(sp, spi);
                self.set(d, format!("({p} ? {a} : {b})"));
            }

            Op::LopR | Op::LopI | Op::LopC | Op::Lop32i => {
                let (d, a, b, lop, na, nb, dp, pop, cc) = match i.op {
                    Op::LopR => {
                        let o = op!(InstLopR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.lop().0, o.neg_a(), o.neg_b(), o.dest_pred(), o.predicate_op().0, o.write_cc())
                    }
                    Op::LopI => {
                        let o = op!(InstLopI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.logic_op().0, o.neg_a(), o.neg_b(), o.dest_pred(), o.predicate_op().0, o.write_cc())
                    }
                    Op::LopC => {
                        let o = op!(InstLopC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.logic_op().0, o.neg_a(), o.neg_b(), o.dest_pred(), o.predicate_op().0, o.write_cc())
                    }
                    _ => {
                        let o = op!(InstLop32i);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", o.imm32() as u32), o.logic_op().0, o.neg_a(), o.neg_b(), 7, 0, o.write_cc())
                    }
                };
                let a = if na { format!("(~{a})") } else { a };
                let b = if nb { format!("(~{b})") } else { b };
                let e = match lop {
                    0 => format!("({a} & {b})"),
                    1 => format!("({a} | {b})"),
                    2 => format!("({a} ^ {b})"),
                    _ => b,
                };
                self.line(format!("{{ uint v = {e};"));
                self.setp(dp, match pop {
                    0 => "false".into(),
                    1 => "true".into(),
                    2 => "v == 0u".into(),
                    _ => "v != 0u".into(),
                });
                if cc {
                    self.set_zn("v".into());
                }
                self.set(d, "v".into());
                self.line("}");
            }
            Op::Lop3R | Op::Lop3I | Op::Lop3C => {
                let (d, a, b, c, lut, dp, pop) = match i.op {
                    Op::Lop3R => {
                        let o = op!(InstLop3R);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), self.r(o.src_c()), o.imm(), o.dest_pred(), o.predicate_op().0)
                    }
                    Op::Lop3I => {
                        let o = op!(InstLop3I);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), self.r(o.src_c()), o.imm(), 7, 0)
                    }
                    _ => {
                        let o = op!(InstLop3C);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), self.r(o.src_c()), o.imm(), 7, 0)
                    }
                };
                self.line(format!("{{ uint v = lop3({a}, {b}, {c}, {lut}u);"));
                self.setp(dp, match pop {
                    0 => "false".into(),
                    1 => "true".into(),
                    2 => "v == 0u".into(),
                    _ => "v != 0u".into(),
                });
                self.set(d, "v".into());
                self.line("}");
            }
            Op::ShlR | Op::ShlI | Op::ShlC => {
                let (d, a, b, m) = match i.op {
                    Op::ShlR => {
                        let o = op!(InstShlR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.m())
                    }
                    Op::ShlI => {
                        let o = op!(InstShlI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.m())
                    }
                    _ => {
                        let o = op!(InstShlC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.m())
                    }
                };
                let e = if m { format!("({a} << ({b} & 31u))") } else { format!("({b} >= 32u ? 0u : {a} << {b})") };
                self.set(d, e);
            }
            Op::ShrR | Op::ShrI | Op::ShrC => {
                let (d, a, b, s, m, brev) = match i.op {
                    Op::ShrR => {
                        let o = op!(InstShrR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.signed(), o.m(), o.brev())
                    }
                    Op::ShrI => {
                        let o = op!(InstShrI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.signed(), o.m(), o.brev())
                    }
                    _ => {
                        let o = op!(InstShrC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.signed(), o.m(), o.brev())
                    }
                };
                let a = if brev { format!("reverse_bits({a})") } else { a };
                let sh = if m { format!("({b} & 31u)") } else { format!("min({b}, 31u)") };
                let e = if s {
                    format!("uint(int({a}) >> {sh})")
                } else if m {
                    format!("({a} >> {sh})")
                } else {
                    format!("({b} >= 32u ? 0u : {a} >> {b})")
                };
                self.set(d, e);
            }
            Op::BfeR | Op::BfeI | Op::BfeC => {
                let (d, a, b, s) = match i.op {
                    Op::BfeR => {
                        let o = op!(InstBfeR);
                        (o.dest(), self.r(o.src_a()), self.r(o.src_b()), o.signed())
                    }
                    Op::BfeI => {
                        let o = op!(InstBfeI);
                        (o.dest(), self.r(o.src_a()), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.signed())
                    }
                    _ => {
                        let o = op!(InstBfeC);
                        (o.dest(), self.r(o.src_a()), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.signed())
                    }
                };
                self.set(d, format!("bfe({a}, {b}, {s})"));
            }

            Op::I2fR | Op::I2fI | Op::I2fC => {
                let (d, b, fmt, sel, abs, neg) = match i.op {
                    Op::I2fR => {
                        let o = op!(InstI2fR);
                        (o.dest(), self.r(o.src_b()), o.i_src_fmt().0, o.byte_sel().0, o.abs_b(), o.neg_b())
                    }
                    Op::I2fI => {
                        let o = op!(InstI2fI);
                        (o.dest(), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.i_src_fmt().0, o.byte_sel().0, o.abs_b(), o.neg_b())
                    }
                    _ => {
                        let o = op!(InstI2fC);
                        (o.dest(), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.i_src_fmt().0, o.byte_sel().0, o.abs_b(), o.neg_b())
                    }
                };
                self.setf(d, Self::fmod(format!("i2f({b}, {fmt}u, {sel}u)"), abs, neg));
            }
            Op::F2iR | Op::F2iI | Op::F2iC => {
                let (d, b, abs, neg, sh, dst, src, rnd) = match i.op {
                    Op::F2iR => {
                        let o = op!(InstF2iR);
                        (o.dest(), self.r(o.src_b()), o.abs_b(), o.neg_b(), o.sh(), o.i_dst_fmt().0, o.src_fmt().0, o.round_mode().0)
                    }
                    Op::F2iI => {
                        let o = op!(InstF2iI);
                        (o.dest(), format!("{:#x}u", (o.imm20() as u32) << 12), o.abs_b(), o.neg_b(), o.sh(), o.i_dst_fmt().0, o.src_fmt().0, o.round_mode().0)
                    }
                    _ => {
                        let o = op!(InstF2iC);
                        (o.dest(), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.abs_b(), o.neg_b(), o.sh(), o.i_dst_fmt().0, o.src_fmt().0, o.round_mode().0)
                    }
                };
                let x = Self::fmod(format!("tof32({b}, {src}u, {sh})"), abs, neg);
                self.set(d, format!("f2i({x}, {rnd}u, {dst}u)"));
            }
            Op::F2fR | Op::F2fI | Op::F2fC => {
                let (d, b, abs, neg, sh, dst, src, rnd, s) = match i.op {
                    Op::F2fR => {
                        let o = op!(InstF2fR);
                        (o.dest(), self.r(o.src_b()), o.abs_b(), o.neg_b(), o.sh(), o.dst_fmt().0, o.src_fmt().0, o.round_mode().0, o.sat())
                    }
                    Op::F2fI => {
                        let o = op!(InstF2fI);
                        (o.dest(), format!("{:#x}u", (o.imm20() as u32) << 12), o.abs_b(), o.neg_b(), o.sh(), o.dst_fmt().0, o.src_fmt().0, o.round_mode().0, o.sat())
                    }
                    _ => {
                        let o = op!(InstF2fC);
                        (o.dest(), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.abs_b(), o.neg_b(), o.sh(), o.dst_fmt().0, o.src_fmt().0, o.round_mode().0, o.sat())
                    }
                };
                let x = Self::sat(Self::fmod(format!("fround(tof32({b}, {src}u, {sh}), {rnd}u)"), abs, neg), s);
                if dst == 1 {
                    self.set(d, format!("as_type<uint>(half2(half({x}), 0.0h))"));
                } else {
                    self.setf(d, x);
                }
            }
            Op::I2iR | Op::I2iI | Op::I2iC => {
                let (d, b, sel, src, dst, s, abs, neg) = match i.op {
                    Op::I2iR => {
                        let o = op!(InstI2iR);
                        (o.dest(), self.r(o.src_b()), o.byte_sel().0, o.i_src_fmt().0, o.i_dst_fmt().0, o.sat(), o.abs_b(), o.neg_b())
                    }
                    Op::I2iI => {
                        let o = op!(InstI2iI);
                        (o.dest(), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32), o.byte_sel().0, o.i_src_fmt().0, o.i_dst_fmt().0, o.sat(), o.abs_b(), o.neg_b())
                    }
                    _ => {
                        let o = op!(InstI2iC);
                        (o.dest(), self.cb(o.cbuf_slot(), o.cbuf_offset()), o.byte_sel().0, o.i_src_fmt().0, o.i_dst_fmt().0, o.sat(), o.abs_b(), o.neg_b())
                    }
                };
                self.set(d, format!("i2i({b}, {sel}u, {src}u, {dst}u, {s}, {abs}, {neg})"));
            }

            Op::MovR | Op::MovI | Op::MovC | Op::Mov32i => {
                let (d, v) = match i.op {
                    Op::MovR => {
                        let o = op!(InstMovR);
                        (o.dest(), self.r(o.src_a()))
                    }
                    Op::MovI => {
                        let o = op!(InstMovI);
                        (o.dest(), format!("{:#x}u", ((o.imm20() << 12) >> 12) as u32))
                    }
                    Op::MovC => {
                        let o = op!(InstMovC);
                        if o.cbuf_slot() == TEXTURE_HANDLE_BANK && o.cbuf_offset() * 4 >= 0x20 {
                            self.handle_regs.insert(o.dest(), ((o.cbuf_offset() * 4 - 0x20) / 8) as u32);
                        }
                        (o.dest(), self.cb(o.cbuf_slot(), o.cbuf_offset()))
                    }
                    _ => {
                        let o = op!(InstMov32i);
                        (o.dest(), format!("{:#x}u", o.imm32() as u32))
                    }
                };
                self.set(d, v);
            }
            Op::Ldc => {
                let o = op!(InstLdc);
                let mut a = self.r(o.src_a());

                let slot = if matches!(o.address_mode().0, 2 | 3) {
                    let s = format!("min({}u + ({a} >> 16), 17u)", o.cbuf_slot());
                    a = format!("({a} & 0xffffu)");
                    s
                } else {
                    o.cbuf_slot().to_string()
                };

                let off = (o.cbuf_offset() as u16 as i16) as i32;
                let addr = format!("({a} + {:#x}u)", off as u32);
                match o.ls_size().0 {
                    5 => {

                        self.line(format!("{{ uint2 cv = uint2(res.cb[{slot}][{addr} >> 2], res.cb[{slot}][({addr} >> 2) + 1u]);"));
                        self.set(o.dest(), "cv.x".into());
                        if o.dest() != RZ_ {
                            self.set(o.dest() + 1, "cv.y".into());
                        }
                        self.line("}");
                    }
                    4 => self.set(o.dest(), format!("res.cb[{slot}][{addr} >> 2]")),
                    sz => {
                        let (bits, signed) = match sz {
                            0 => (8, false),
                            1 => (8, true),
                            2 => (16, false),
                            _ => (16, true),
                        };
                        self.set(o.dest(), format!("bfe(res.cb[{slot}][{addr} >> 2], ((({addr} & 3u) * 8u) | ({bits}u << 8)), {signed})"));
                    }
                }
            }
            Op::Ldl | Op::Stl => {
                self.uses_lmem = true;
                let (d, a, off, size) = if i.op == Op::Ldl {
                    let o = op!(InstLdl);
                    (o.dest(), self.r(o.src_a()), o.imm24(), o.ls_size().0)
                } else {
                    let o = op!(InstStl);
                    (o.dest(), self.r(o.src_a()), o.imm24(), o.ls_size().0)
                };
                let words = match size {
                    5 => 2,
                    6 => 4,
                    _ => 1,
                };

                self.line(format!("{{ uint la = ({a} + {:#x}u) >> 2;", off as u32));
                for k in 0..words {
                    let idx = format!("(la + {k}u) & LMEM_MASK");
                    if i.op == Op::Ldl {
                        self.line(format!("uint lv{k} = lmem[{idx}];"));
                    } else {
                        let v = self.r(if d == RZ_ { RZ_ } else { d + k });
                        self.line(format!("lmem[{idx}] = {v};"));
                    }
                }
                if i.op == Op::Ldl && d != RZ_ {
                    for k in 0..words {
                        self.set(d + k, format!("lv{k}"));
                    }
                }
                self.line("}");
            }
            Op::Ldg | Op::Stg => {

                let (d, ra, imm, size, e) = if i.op == Op::Ldg {
                    let o = op!(InstLdg);
                    (o.dest(), o.src_a(), o.imm24(), o.ls_size().0, o.e())
                } else {
                    let o = op!(InstStg);
                    (o.dest(), o.src_a(), o.imm24(), o.ls_size().0, o.e())
                };
                let imm = ((imm << 8) >> 8) as i64;
                let lo = self.r(ra);
                let hi = if e && ra != RZ_ { self.r(ra + 1) } else { "0u".into() };
                self.line(format!("{{ ulong ga = ((ulong({hi}) << 32) | ulong({lo})) + ulong({imm}l); device uint* gp = gmem(res, ga & ~3ul);"));
                let words = match size {
                    5 => 2,
                    6 | 7 => 4,
                    _ => 1,
                };
                if i.op == Op::Ldg {
                    if size < 4 {
                        let (bits, signed) = match size {
                            0 => (8, false),
                            1 => (8, true),
                            2 => (16, false),
                            _ => (16, true),
                        };
                        self.set(d, format!("bfe(gp[0], ((uint(ga & 3ul) * 8u) | ({bits}u << 8)), {signed})"));
                    } else {

                        self.line(format!("uint gv[{words}];"));
                        for k in 0..words {
                            self.line(format!("gv[{k}] = gmem(res, (ga & ~3ul) + {k}ul * 4ul)[0];"));
                        }
                        for k in 0..words {
                            if d != RZ_ {
                                self.set(d + k, format!("gv[{k}]"));
                            }
                        }
                    }
                } else if size >= 4 {
                    for k in 0..words {
                        let v = self.r(if d == RZ_ { RZ_ } else { d + k });
                        self.line(format!("if (ga + {k}ul * 4ul - res.heap_base < res.heap_size) gmem(res, (ga & ~3ul) + {k}ul * 4ul)[0] = {v};"));
                    }
                } else {
                    self.warnings.push("sub-word global stores ignored".into());
                }
                self.line("}");
            }
            Op::S2r => {
                let o = op!(InstS2r);
                let compute = self.sph.stage == Stage::Compute;
                let v = match o.s_reg().0 {
                    0 => "simd_lane",

                    0x20 if compute => "(tid.x | tid.y << 16 | tid.z << 26)",
                    0x21 if compute => "tid.x",
                    0x22 if compute => "tid.y",
                    0x23 if compute => "tid.z",

                    0x25 if compute => "ctaid.x",
                    0x26 if compute => "ctaid.y",
                    0x27 if compute => "ctaid.z",
                    _ => "0u",
                };
                self.set(o.dest(), v.into());
            }

            Op::Ald => {
                let o = op!(InstAld);
                let n = o.al_size().0 as i32 + 1;
                if o.src_a() != RZ_ {
                    self.warnings.push("indexed attribute load".into());
                }
                for k in 0..n {
                    let v = self.attr_load(o.imm11() + 4 * k, o.o());
                    self.set(o.dest() + k, v);
                }
            }
            Op::Ast => {
                let o = op!(InstAst);
                let n = o.al_size().0 as i32 + 1;
                if o.src_a() != RZ_ {
                    self.warnings.push("indexed attribute store".into());
                }
                for k in 0..n {
                    let v = self.fr(o.src_b() + k);
                    match self.vs_output_ref(o.imm11() + 4 * k) {
                        Some(lv) => self.line(format!("{lv} = {v};")),
                        None => self.warnings.push(format!("attribute store {:#x}", o.imm11() + 4 * k)),
                    }
                }
            }
            Op::Ipa => {
                let o = op!(InstIpa);
                let mut v = if o.idx() {
                    self.warnings.push("indexed ipa".into());
                    "0u".into()
                } else {
                    self.fs_input(o.imm10(), true)
                };
                let fixed = matches!(o.imm10(), ATTR_POSITION..=0x7c | ATTR_FRONT_FACING);
                if o.ipa_op() == IpaOp::Multiply && !fixed {
                    let b = self.fr(o.src_b());
                    v = format!("as_type<uint>(as_type<float>({v}) * {b})");
                }
                if o.sat() {
                    v = format!("as_type<uint>(saturate(as_type<float>({v})))");
                }
                self.set(o.dest(), v);
            }
            Op::Isberd => {
                let o = op!(InstIsberd);
                let a = self.r(o.src_a());
                self.set(o.dest(), a);
            }
            Op::OutR | Op::OutI | Op::OutC => {
                self.warnings.push("geometry OUT".into());
            }

            Op::Texs => {
                let o = op!(InstTexs);
                self.texs(o)?;
            }
            Op::Tex => {
                let o = op!(InstTex);
                self.tex_full(o)?;
            }
            Op::Tlds => {
                let o = op!(InstTlds);
                self.tlds(o)?;
            }
            Op::Tld4 | Op::Tld4s => self.tld4(i)?,
            Op::TmmlB | Op::Tmml => {
                let (d, a, mask, dim, tid) = if i.op == Op::Tmml {
                    let o = op!(InstTmml);
                    (o.dest(), o.src_a(), o.w_mask() as u32, o.dim().0, o.tid_b())
                } else {

                    let o = op!(InstTmmlB);
                    let Some(&binding) = self.handle_regs.get(&o.src_b()) else {
                        self.warnings.push("unresolved bindless texture".into());
                        let dd = o.dest();
                        self.set(dd, "0u".into());
                        return Ok(());
                    };
                    (o.dest(), o.src_a(), o.w_mask() as u32, o.dim().0, 8 + 2 * binding as i32)
                };
                let kind = match dim {
                    3 => TexKind::T2DArray,
                    4 => TexKind::T3D,
                    6 => TexKind::Cube,
                    _ => TexKind::T2D,
                };
                let b = self.tex(tid, kind);
                let ra: Vec<i32> = (0..3).map(|k| if a == RZ_ { RZ_ } else { a + k }).collect();
                let coords: Vec<i32> = if kind.is_array() { ra[1..].to_vec() } else { ra };
                let c = self.coord(&coords, kind.coords());

                self.line(format!("{{ float l = t{b}.calculate_clamped_lod(s{b}, {c}); float4 v = float4(l, l, 0.0f, 0.0f);"));
                let dests: Vec<i32> = (0..4).map(|k| if d == RZ_ { RZ_ } else { d + k }).collect();
                self.write_components("v", mask, &dests);
                self.line("}");
            }
            Op::Txq | Op::TxqB => {
                let (d, a, mask, tid, query) = if i.op == Op::Txq {
                    let o = op!(InstTxq);
                    (o.dest(), o.src_a(), o.w_mask() as u32, o.tid_b(), o.tex_query().0)
                } else {
                    let o = op!(InstTxqB);
                    (o.dest(), o.src_a(), o.w_mask() as u32, 8, o.tex_query().0)
                };
                let binding = ((tid - 8) / 2).max(0) as u32;
                let kind = self.iface.textures.get(&binding).copied().unwrap_or(TexKind::T2D);
                let b = self.tex(tid, kind);
                let lod = self.r(a);
                if query == 1 {

                    self.line(format!("{{ uint l = {lod}; uint4 v = uint4(t{b}.get_width(l), t{b}.get_height(l), 1u, t{b}.get_num_mip_levels());"));
                } else {
                    self.line("{ uint4 v = uint4(0u);");
                }
                let dests: Vec<i32> = (0..4).map(|k| if d == RZ_ { RZ_ } else { d + k }).collect();
                let mut k = 0;
                for c in 0..4 {
                    if mask >> c & 1 != 0 {
                        if let Some(&dd) = dests.get(k) {
                            self.set(dd, format!("v[{c}]"));
                        }
                        k += 1;
                    }
                }
                self.line("}");
            }

            Op::Psetp => {
                let o = op!(InstPsetp);
                let (a, b, c) = (self.p(o.src_pred(), o.src_pred_inv()), self.p(o.src2_pred(), o.src2_pred_inv()), self.p(o.src1_pred(), o.src1_pred_inv()));
                let _ = c;
                let (ab, cop) = (o.bool_op_ab().0, o.bool_op_c().0);
                let src_c = self.p(o.src_pred(), o.src_pred_inv());
                let (pa, pb) = (self.p(o.src1_pred(), o.src1_pred_inv()), self.p(o.src2_pred(), o.src2_pred_inv()));
                let _ = (a, b);
                self.line(format!("{{ bool t = bop({ab}u, {pa}, {pb});"));
                self.setp(o.dest_pred(), format!("bop({cop}u, t, {src_c})"));
                self.setp(o.dest_pred_inv(), format!("bop({cop}u, !t, {src_c})"));
                self.line("}");
            }
            Op::Pset => {
                let o = op!(InstPset);
                let (pa, pb) = (self.p(o.src1_pred(), o.src1_pred_inv()), self.p(o.src2_pred(), o.src2_pred_inv()));
                let pc = self.p(o.src_pred(), o.src_pred_inv());
                let c = format!("bop({}u, bop({}u, {pa}, {pb}), {pc})", o.bool_op_c().0, o.bool_op_ab().0);
                self.set(o.dest(), if o.b_val() { format!("({c} ? 0x3f800000u : 0u)") } else { format!("({c} ? 0xffffffffu : 0u)") });
            }
            Op::Csetp => {
                let o = op!(InstCsetp);
                self.uses_cc = true;
                let p = self.p(o.src_pred(), o.src_pred_inv());
                let c = format!("ccc({}u, zf, nf, cf, vf)", o.ccc().0);
                self.setp(o.dest_pred(), format!("bop({}u, {c}, {p})", o.bop().0));
                self.setp(o.dest_pred_inv(), format!("bop({}u, !{c}, {p})", o.bop().0));
            }
            Op::Vsetp => {
                let o = op!(InstVsetp);
                self.setp(o.dest_pred(), "false".into());
                self.setp(o.dest_pred_inv(), "true".into());
            }
            Op::Vmad => {
                let o = op!(InstVmad);
                let (a, b, c) = (self.r(o.src_a()), self.r(o.src_b()), self.r(o.src_c()));
                self.set(o.dest(), format!("(({a} & 0xffffu) * ({b} & 0xffffu) + {c})"));
            }
            Op::Shfl => {
                let o = op!(InstShfl);
                let a = self.r(o.src_a());
                let lane = if o.b_fix_shfl() { format!("{}u", o.src_b_imm()) } else { self.r(o.src_b()) };
                let f = ["simd_shuffle", "simd_shuffle_up", "simd_shuffle_down", "simd_shuffle_xor"][o.shfl_mode().0 as usize & 3];
                self.set(o.dest(), format!("{f}({a}, ushort({lane}))"));
            }

            Op::Bra => {
                let o = op!(InstBra);
                let t = branch_target(i);
                if o.ccc() != Ccc::T {
                    self.uses_cc = true;
                    self.line(format!("if (ccc({}u, zf, nf, cf, vf)) {{ pc = {t}u; continue; }}", o.ccc().0));
                } else {
                    self.label_target(t);
                }
            }
            Op::Sync | Op::Brk => match self.prog.converge.get(&i.addr) {
                Some(&t) => self.label_target(t),
                None => {
                    self.warnings.push(format!("unresolved {:?} at {:#x}", i.name, i.addr));
                    self.line("return epilogue();");
                }
            },
            Op::Exit => {
                let o = op!(InstExit);
                if o.ccc() != Ccc::T {
                    self.uses_cc = true;
                    self.line(format!("if (ccc({}u, zf, nf, cf, vf)) return epilogue();", o.ccc().0));
                } else {
                    self.line("return epilogue();");
                }
            }
            Op::Kil => {
                let o = op!(InstKil);
                if o.ccc() != Ccc::T {
                    self.uses_cc = true;
                    self.line(format!("if (ccc({}u, zf, nf, cf, vf)) discard_fragment();", o.ccc().0));
                } else {
                    self.line("discard_fragment();");
                }
            }
            other => bail!("unimplemented instruction {other:?} ({:?}) at {:#x}", i.name, i.addr),
        }
        Ok(())
    }

    fn texs(&mut self, o: InstTexs) -> Result<()> {
        let (dest, dest2) = (o.dest(), o.dest2());
        if dest == RZ_ && dest2 == RZ_ {
            return Ok(());
        }
        let mut ra = o.src_a();
        let mut rb = o.src_b();
        let mut next_a = |g: &mut Self| {
            let r = ra;
            if ra != RZ_ {
                ra += 1;
            }
            let _ = g;
            r
        };
        let mut a_regs = Vec::new();
        let mut b_regs = Vec::new();
        let t = o.target().0;
        let kind = match t {
            4..=6 => TexKind::Depth2D,
            9 => TexKind::Depth2DArray,
            7 | 8 => TexKind::T2DArray,
            10 | 11 => TexKind::T3D,
            12 | 13 => TexKind::Cube,
            _ => TexKind::T2D,
        };
        let mut lod = None;
        let mut depth_ref = None;
        let mut array = None;
        let take_b = |rb: &mut i32| {
            let r = *rb;
            if *rb != RZ_ {
                *rb += 1;
            }
            r
        };
        match t {
            7 | 8 | 9 => {

                array = Some(next_a(self));
                a_regs.push(next_a(self));
                b_regs.push(take_b(&mut rb));
                if t == 9 {
                    depth_ref = Some(take_b(&mut rb));
                }
                if t != 7 {
                    lod = Some("0.0f".to_string());
                }
            }
            0 => {
                a_regs.push(next_a(self));
                lod = Some("0.0f".into());
            }
            1 => {
                a_regs.push(next_a(self));
                b_regs.push(take_b(&mut rb));
            }
            2 => {
                a_regs.push(next_a(self));
                b_regs.push(take_b(&mut rb));
                lod = Some("0.0f".into());
            }
            3 | 4 | 10 | 12 => {
                a_regs.push(next_a(self));
                a_regs.push(next_a(self));
                let x = take_b(&mut rb);
                match t {
                    3 => {
                        let l = self.fr(x);
                        lod = Some(l);
                    }
                    4 => depth_ref = Some(x),
                    _ => b_regs.push(x),
                }
            }
            6 | 11 => {
                a_regs.push(next_a(self));
                a_regs.push(next_a(self));
                let x = take_b(&mut rb);
                if t == 6 { depth_ref = Some(x) } else { b_regs.push(x) }
                lod = Some("0.0f".into());
            }
            5 | 13 => {
                a_regs.push(next_a(self));
                a_regs.push(next_a(self));
                let x = take_b(&mut rb);
                let y = take_b(&mut rb);
                if t == 5 {
                    depth_ref = Some(x);
                    lod = Some(self.fr(y));
                } else {
                    b_regs.push(x);
                    lod = Some(self.fr(y));
                }
            }
            _ => bail!("texs target {t}"),
        }
        let coords: Vec<i32> = a_regs.into_iter().chain(b_regs).collect();
        let coords = if t == 0 { vec![coords[0], RZ_] } else { coords };
        let b = self.tex(o.tid_b(), kind);
        let v = self.sample(b, kind, &coords, array, lod, None, depth_ref);
        let mask = TEXS_MASK_LUT[if dest2 == RZ_ { 0 } else { 1 }][o.w_mask() as usize & 7];
        let dests = [dest, if dest == RZ_ { RZ_ } else { dest + 1 }, dest2, if dest2 == RZ_ { RZ_ } else { dest2 + 1 }];
        self.line(format!("{{ float4 v = {v};"));
        self.write_components("v", mask, &dests);
        self.line("}");
        Ok(())
    }

    fn tex_full(&mut self, o: InstTex) -> Result<()> {
        let d = o.dest();
        if d == RZ_ {
            return Ok(());
        }
        let (mut ra, mut rb) = (o.src_a(), o.src_b());
        let mut na = || {
            let r = ra;
            if ra != RZ_ {
                ra += 1;
            }
            r
        };
        let mut nb = || {
            let r = rb;
            if rb != RZ_ {
                rb += 1;
            }
            r
        };
        let dim = o.dim().0;
        let dc = o.dc();
        let kind = match (dim, dc) {
            (3 | 1, true) => TexKind::Depth2DArray,
            (6 | 7, true) => TexKind::DepthCube,
            (_, true) => TexKind::Depth2D,
            (3 | 1, false) => TexKind::T2DArray,
            (4 | 5, _) => TexKind::T3D,
            (6, _) => TexKind::Cube,
            (7, _) => TexKind::CubeArray,
            _ => TexKind::T2D,
        };
        let array = if matches!(dim, 1 | 3 | 5 | 7) { Some(na()) } else { None };
        let n = match dim {
            0 | 1 => 1,
            4 | 5 | 6 | 7 => 3,
            _ => 2,
        };
        let mut coords: Vec<i32> = (0..n).map(|_| na()).collect();
        if n == 1 {
            coords.push(RZ_);
        }
        let lodm = o.lod().0;
        let lod_reg = if lodm >= 2 { Some(nb()) } else { None };
        if o.aoffi() {
            let _ = nb();
            self.warnings.push("texture offsets ignored".into());
        }
        let depth_ref = if dc { Some(nb()) } else { None };
        let (lod, bias) = match lodm {
            1 => (Some("0.0f".to_string()), None),
            3 | 7 => (Some(self.fr(lod_reg.unwrap())), None),
            2 | 6 => (None, Some(self.fr(lod_reg.unwrap()))),
            _ => (None, None),
        };
        let b = self.tex(o.tid_b(), kind);
        let v = self.sample(b, kind, &coords, array, lod, bias, depth_ref);
        self.line(format!("{{ float4 v = {v};"));
        let dests: Vec<i32> = (0..4).map(|k| if d + k >= RZ_ { RZ_ } else { d + k }).collect();
        self.write_components("v", o.w_mask() as u32, &dests);
        self.line("}");
        Ok(())
    }

    fn tlds(&mut self, o: InstTlds) -> Result<()> {
        let (dest, dest2) = (o.dest(), o.dest2());
        if dest == RZ_ && dest2 == RZ_ {
            return Ok(());
        }
        let (mut ra, mut rb) = (o.src_a(), o.src_b());
        let mut na = || {
            let r = ra;
            if ra != RZ_ {
                ra += 1;
            }
            r
        };
        let mut nb = || {
            let r = rb;
            if rb != RZ_ {
                rb += 1;
            }
            r
        };
        let t = o.target().0;
        let (x, y, lod, kind, arr) = match t {
            0 => (na(), RZ_, None, TexKind::T2D, None),
            1 => (na(), RZ_, Some(nb()), TexKind::T2D, None),
            2 => (na(), nb(), None, TexKind::T2D, None),
            4 => (na(), na(), None, TexKind::T2D, None),
            5 | 0xc => (na(), na(), Some(nb()), TexKind::T2D, None),
            6 => (na(), na(), None, TexKind::T2D, None),
            7 => (na(), na(), None, TexKind::T3D, None),
            8 => {
                let x = nb();
                let y = nb();
                (x, y, None, TexKind::T2DArray, Some(na()))
            }
            _ => bail!("tlds target {t}"),
        };
        let b = self.tex(o.tid_b(), kind);
        let (xs, ys) = (self.r(x), self.r(y));
        let l = lod.map(|r| self.r(r)).unwrap_or("0u".into());
        let v = match (kind, arr) {
            (TexKind::T2DArray, Some(a)) => {
                let av = self.r(a);
                format!("t{b}.read(uint2({xs}, {ys}), {av}, {l})")
            }
            (TexKind::T3D, _) => format!("t{b}.read(uint3({xs}, {ys}, 0u), {l})"),
            _ => format!("t{b}.read(uint2({xs}, {ys}), {l})"),
        };
        let mask = TEXS_MASK_LUT[if dest2 == RZ_ { 0 } else { 1 }][o.w_mask() as usize & 7];
        let dests = [dest, if dest == RZ_ { RZ_ } else { dest + 1 }, dest2, if dest2 == RZ_ { RZ_ } else { dest2 + 1 }];
        self.line(format!("{{ float4 v = {v};"));
        self.write_components("v", mask, &dests);
        self.line("}");
        Ok(())
    }

    fn tld4(&mut self, i: &Inst) -> Result<()> {
        let w = i.raw;
        let (d, d2, a, bsrc, comp, dc, tid, mask, two_dest) = if i.op == Op::Tld4s {
            let o = InstTld4s(w);
            (o.dest(), o.dest2(), o.src_a(), o.src_b(), o.tex_comp().0, o.dc(), o.tid_b(), 0xF, true)
        } else {
            let o = InstTld4(w);
            (o.dest(), RZ_, o.src_a(), o.src_b(), o.tex_comp().0, o.dc(), o.tid_b(), o.w_mask() as u32, false)
        };
        let kind = if dc { TexKind::Depth2D } else { TexKind::T2D };
        let b = self.tex(tid, kind);
        let coords = if two_dest && dc { vec![a, if a == RZ_ { RZ_ } else { a + 1 }] } else if two_dest { vec![a, bsrc] } else { vec![a, if a == RZ_ { RZ_ } else { a + 1 }] };
        let c = self.coord(&coords, 2);
        let comp_name = ["x", "y", "z", "w"][comp as usize & 3];
        let v = if dc {
            let rf = if two_dest { bsrc } else { bsrc };
            let rv = self.fr(rf);
            format!("t{b}.gather_compare(s{b}, {c}, {rv})")
        } else {
            format!("t{b}.gather(s{b}, {c}, int2(0), component::{comp_name})")
        };
        self.line(format!("{{ float4 v = {v};"));
        let dests: Vec<i32> = if two_dest {
            vec![d, if d == RZ_ { RZ_ } else { d + 1 }, d2, if d2 == RZ_ { RZ_ } else { d2 + 1 }]
        } else {
            (0..4).map(|k| if d == RZ_ { RZ_ } else { d + k }).collect()
        };
        self.write_components("v", mask, &dests);
        self.line("}");
        Ok(())
    }
}

const TEXS_MASK_LUT: [[u32; 8]; 2] = [
    [0b0001, 0b0010, 0b0100, 0b1000, 0b0011, 0b1001, 0b1010, 0b1100],
    [0b0111, 0b1011, 0b1101, 0b1110, 0b1111, 0b0000, 0b0000, 0b0000],
];

const PRELUDE: &str = r#"#include <metal_stdlib>
using namespace metal;
#pragma METAL fp contract(off)

struct Res {
    const device uint* cb[18];
    const device uchar* vb[16];
    uint vb_stride[16];
    uint vb_divisor[16];
    uint attr_format[16];
    uint attr_stream[16];
    uint attr_offset[16];
    int base_vertex;
    uint base_instance;
    uint flags;
    float alpha_ref;
    uint alpha_func;
    ulong heap_base;
    ulong heap_gpu[16];
    ulong heap_size;
    ulong heap_chunk_size;
    ulong zero_gpu;
};

static inline device uint* gmem(constant Res& res, ulong ga) {
    ulong off = ga - res.heap_base;
    if (off >= res.heap_size || res.heap_chunk_size == 0ul) return reinterpret_cast<device uint*>(res.zero_gpu);
    // Most devices fit the entire heap in one buffer; A15 needs only two.
    // Keep both paths free of the expensive runtime 64-bit divide/remainder.
    if (off < res.heap_chunk_size) return reinterpret_cast<device uint*>(res.heap_gpu[0] + off);
    ulong second = off - res.heap_chunk_size;
    if (second < res.heap_chunk_size) return reinterpret_cast<device uint*>(res.heap_gpu[1] + second);
    ulong chunk = off / res.heap_chunk_size;
    return reinterpret_cast<device uint*>(res.heap_gpu[chunk] + off % res.heap_chunk_size);
}

static inline bool bop(uint op, bool a, bool b) {
    return op == 0u ? (a && b) : op == 1u ? (a || b) : (a != b);
}
static inline bool fcmp(uint op, float a, float b) {
    bool un = isnan(a) || isnan(b);
    switch (op) {
    case 0u: return false;
    case 1u: return a < b;
    case 2u: return a == b;
    case 3u: return a <= b;
    case 4u: return a > b;
    case 5u: return !un && a != b;
    case 6u: return a >= b;
    case 7u: return !un;
    case 8u: return un;
    case 9u: return un || a < b;
    case 10u: return un || a == b;
    case 11u: return un || a <= b;
    case 12u: return un || a > b;
    case 13u: return a != b;
    case 14u: return un || a >= b;
    default: return true;
    }
}
static inline bool icmp(uint op, uint a, uint b, bool s) {
    int sa = int(a), sb = int(b);
    switch (op) {
    case 1u: return s ? sa < sb : a < b;
    case 2u: return a == b;
    case 3u: return s ? sa <= sb : a <= b;
    case 4u: return s ? sa > sb : a > b;
    case 5u: return a != b;
    case 6u: return s ? sa >= sb : a >= b;
    case 7u: return true;
    default: return false;
    }
}
static inline bool ccc(uint c, bool zf, bool nf, bool cf, bool vf) {
    switch (c) {
    case 0u: return false;
    case 1u: return nf != vf;
    case 2u: return zf;
    case 3u: return zf || (nf != vf);
    case 4u: return !zf && (nf == vf);
    case 5u: return !zf;
    case 6u: return nf == vf;
    case 15u: return true;
    case 16u: return vf;
    case 17u: return cf;
    case 18u: return nf;
    case 19u: return !cf || zf;
    case 20u: return cf && !zf;
    case 21u: return !nf;
    case 22u: return !cf;
    case 23u: return !vf;
    default: return true;
    }
}
static inline uint lop3(uint a, uint b, uint c, uint lut) {
    uint r = 0u;
    for (uint i = 0u; i < 8u; i++) {
        if ((lut >> i) & 1u) {
            r |= ((i & 4u) ? a : ~a) & ((i & 2u) ? b : ~b) & ((i & 1u) ? c : ~c);
        }
    }
    return r;
}
static inline uint bfe(uint a, uint b, bool s) {
    uint pos = b & 0xffu, len = (b >> 8) & 0xffu;
    if (len == 0u) return 0u;
    if (pos >= 32u) return s && int(a) < 0 ? 0xffffffffu : 0u;
    len = min(len, 32u - pos);
    uint v = a >> pos;
    if (len < 32u) {
        uint m = (1u << len) - 1u;
        v &= m;
        if (s && ((v >> (len - 1u)) & 1u)) v |= ~m;
    }
    return v;
}
static inline uint ext16(uint x, bool high, bool s) {
    if (s) return high ? uint(int(x) >> 16) : uint(int(x << 16) >> 16);
    return high ? (x >> 16) : (x & 0xffffu);
}
static inline uint xmad(uint a, uint b, uint c, uint mode, bool sa, bool sb, bool ha, bool hb, bool psl, bool mrg, uint carry) {
    uint ea = ext16(a, ha, sa), eb = ext16(b, hb, sb);
    uint p = ea * eb;
    if (psl) p <<= 16;
    switch (mode) {
    case 1u: c = c & 0xffffu; break;
    case 2u: c = c >> 16; break;
    case 3u: c = c - (((ea >> 31) << 16) + ((eb >> 31) << 16)); break;
    case 4u: c = c + (b << 16); break;
    default: break;
    }
    uint r = carry != 0xffffffffu ? p + carry : p + c;
    if (mrg) r = (r & 0xffffu) | (b << 16);
    return r;
}
static inline float i2f(uint x, uint fmt, uint sel) {
    switch (fmt) {
    case 0u: return float((x >> (sel * 8u)) & 0xffu);
    case 1u: return float((x >> ((sel >> 1) * 16u)) & 0xffffu);
    case 4u: return float(int(x << (24u - sel * 8u)) >> 24);
    case 5u: return float(int(x << (16u - (sel >> 1) * 16u)) >> 16);
    case 6u: case 7u: return float(int(x));
    default: return float(x);
    }
}
static inline float tof32(uint x, uint fmt, bool high) {
    if (fmt == 1u) return float(as_type<half2>(x)[high ? 1 : 0]);
    return as_type<float>(x);
}
static inline float fround(float x, uint mode) {
    switch (mode) {
    case 4u: return rint(x);
    case 5u: return floor(x);
    case 6u: return ceil(x);
    case 7u: return trunc(x);
    default: return x;
    }
}
static inline uint f2i(float x, uint rnd, uint fmt) {
    if (isnan(x)) return 0u;
    x = rnd == 1u ? floor(x) : rnd == 2u ? ceil(x) : rnd == 3u ? trunc(x) : rint(x);
    switch (fmt) {
    case 1u: return uint(clamp(x, 0.0f, 65535.0f));
    case 5u: return uint(int(clamp(x, -32768.0f, 32767.0f)));
    case 6u: case 7u: return x >= 2147483648.0f ? 0x7fffffffu : x <= -2147483648.0f ? 0x80000000u : uint(int(x));
    default: return x >= 4294967296.0f ? 0xffffffffu : x <= 0.0f ? 0u : uint(x);
    }
}
static inline uint i2i(uint x, uint sel, uint src, uint dst, bool sat, bool ab, bool ng) {
    int v;
    switch (src) {
    case 0u: v = int((x >> (sel * 8u)) & 0xffu); break;
    case 1u: v = int((x >> ((sel >> 1) * 16u)) & 0xffffu); break;
    case 4u: v = int(x << (24u - sel * 8u)) >> 24; break;
    case 5u: v = int(x << (16u - (sel >> 1) * 16u)) >> 16; break;
    default: v = int(x); break;
    }
    if (ab) v = abs(v);
    if (ng) v = -v;
    if (sat) {
        switch (dst) {
        case 0u: v = clamp(v, 0, 255); break;
        case 1u: v = clamp(v, 0, 65535); break;
        case 4u: v = clamp(v, -128, 127); break;
        case 5u: v = clamp(v, -32768, 32767); break;
        default: break;
        }
    }
    return uint(v);
}

static inline uint4 fetch_attr(constant Res& res, uint i, uint vid, uint iid) {
    uint fmt = res.attr_format[i];
    if (fmt == 0u) return uint4(0u, 0u, 0u, 0x3f800000u);
    uint s = res.attr_stream[i];
    uint div = res.vb_divisor[s];
    uint index = div == 0u ? vid : res.base_instance + iid / div;
    const device uchar* p = res.vb[s] + index * res.vb_stride[s] + res.attr_offset[i];
    float4 f = float4(0.0f, 0.0f, 0.0f, 1.0f);
    uint4 u = uint4(0u, 0u, 0u, 1u);
    bool integer = false;
    switch (fmt) {
    case 10u: f.x = *(const device float*)p; break;
    case 22u: f.xy = float2(*(const device packed_float2*)p); break;
    case 34u: f.xyz = float3(*(const device packed_float3*)p); break;
    case 46u: f = float4(*(const device packed_float4*)p); break;
    case 11u: case 12u: u.x = *(const device uint*)p; integer = true; break;
    case 23u: case 24u: u.xy = uint2(*(const device packed_uint2*)p); integer = true; break;
    case 35u: case 36u: u.xyz = uint3(*(const device packed_uint3*)p); integer = true; break;
    case 47u: case 48u: u = uint4(*(const device packed_uint4*)p); integer = true; break;
    case 5u: f.x = float(*(const device half*)p); break;
    case 17u: f.xy = float2(*(const device packed_half2*)p); break;
    case 29u: f.xyz = float3(*(const device packed_half3*)p); break;
    case 41u: f = float4(*(const device packed_half4*)p); break;
    case 6u: f.x = float(*(const device ushort*)p) / 65535.0f; break;
    case 18u: f.xy = float2(*(const device packed_ushort2*)p) / 65535.0f; break;
    case 30u: f.xyz = float3(*(const device packed_ushort3*)p) / 65535.0f; break;
    case 42u: f = float4(*(const device packed_ushort4*)p) / 65535.0f; break;
    case 7u: f.x = max(float(*(const device short*)p) / 32767.0f, -1.0f); break;
    case 19u: f.xy = max(float2(*(const device packed_short2*)p) / 32767.0f, -1.0f); break;
    case 31u: f.xyz = max(float3(*(const device packed_short3*)p) / 32767.0f, -1.0f); break;
    case 43u: f = max(float4(*(const device packed_short4*)p) / 32767.0f, -1.0f); break;
    case 8u: u.x = *(const device ushort*)p; integer = true; break;
    case 20u: u.xy = uint2(*(const device packed_ushort2*)p); integer = true; break;
    case 32u: u.xyz = uint3(*(const device packed_ushort3*)p); integer = true; break;
    case 44u: u = uint4(*(const device packed_ushort4*)p); integer = true; break;
    case 9u: u.x = uint(int(*(const device short*)p)); integer = true; break;
    case 21u: u.xy = uint2(int2(*(const device packed_short2*)p)); integer = true; break;
    case 33u: u.xyz = uint3(int3(*(const device packed_short3*)p)); integer = true; break;
    case 45u: u = uint4(int4(*(const device packed_short4*)p)); integer = true; break;
    case 1u: f.x = float(p[0]) / 255.0f; break;
    case 13u: f.xy = float2(p[0], p[1]) / 255.0f; break;
    case 25u: f.xyz = float3(p[0], p[1], p[2]) / 255.0f; break;
    case 37u: case 56u: f = float4(p[0], p[1], p[2], p[3]) / 255.0f; break;
    case 2u: f.x = max(float(char(p[0])) / 127.0f, -1.0f); break;
    case 14u: f.xy = max(float2(char(p[0]), char(p[1])) / 127.0f, -1.0f); break;
    case 26u: f.xyz = max(float3(char(p[0]), char(p[1]), char(p[2])) / 127.0f, -1.0f); break;
    case 38u: f = max(float4(char(p[0]), char(p[1]), char(p[2]), char(p[3])) / 127.0f, -1.0f); break;
    case 3u: u.x = p[0]; integer = true; break;
    case 15u: u.xy = uint2(p[0], p[1]); integer = true; break;
    case 27u: u.xyz = uint3(p[0], p[1], p[2]); integer = true; break;
    case 39u: u = uint4(p[0], p[1], p[2], p[3]); integer = true; break;
    case 4u: u.x = uint(int(char(p[0]))); integer = true; break;
    case 16u: u.xy = uint2(int2(char(p[0]), char(p[1]))); integer = true; break;
    case 28u: u.xyz = uint3(int3(char(p[0]), char(p[1]), char(p[2]))); integer = true; break;
    case 40u: u = uint4(int4(char(p[0]), char(p[1]), char(p[2]), char(p[3]))); integer = true; break;
    case 81u: f.x = float(p[0]); break;
    case 82u: f.x = float(char(p[0])); break;
    case 83u: f.x = float(*(const device ushort*)p); break;
    case 84u: f.x = float(*(const device short*)p); break;
    case 85u: f.x = float(*(const device uint*)p); break;
    case 86u: f.x = float(*(const device int*)p); break;
    case 87u: f.xy = float2(p[0], p[1]); break;
    case 88u: f.xy = float2(char(p[0]), char(p[1])); break;
    case 89u: f.xy = float2(*(const device packed_ushort2*)p); break;
    case 90u: f.xy = float2(*(const device packed_short2*)p); break;
    case 91u: f.xy = float2(*(const device packed_uint2*)p); break;
    case 92u: f.xy = float2(*(const device packed_int2*)p); break;
    case 93u: f.xyz = float3(p[0], p[1], p[2]); break;
    case 94u: f.xyz = float3(char(p[0]), char(p[1]), char(p[2])); break;
    case 95u: f.xyz = float3(*(const device packed_ushort3*)p); break;
    case 96u: f.xyz = float3(*(const device packed_short3*)p); break;
    case 97u: f.xyz = float3(*(const device packed_uint3*)p); break;
    case 98u: f.xyz = float3(*(const device packed_int3*)p); break;
    case 99u: f = float4(p[0], p[1], p[2], p[3]); break;
    case 100u: f = float4(char(p[0]), char(p[1]), char(p[2]), char(p[3])); break;
    case 101u: f = float4(*(const device packed_ushort4*)p); break;
    case 102u: f = float4(*(const device packed_short4*)p); break;
    case 103u: f = float4(*(const device packed_uint4*)p); break;
    case 104u: f = float4(*(const device packed_int4*)p); break;
    case 105u: case 106u: case 108u: {
        int v = *(const device int*)p;
        int4 c = int4((v << 22) >> 22, (v << 12) >> 22, (v << 2) >> 22, v >> 30);
        if (fmt == 105u) f = max(float4(c) / float4(511.0f, 511.0f, 511.0f, 1.0f), -1.0f);
        else if (fmt == 108u) f = float4(c);
        else { u = as_type<uint4>(c); integer = true; }
        break;
    }
    case 107u: { uint v = *(const device uint*)p; f = float4(v & 0x3ffu, (v >> 10) & 0x3ffu, (v >> 20) & 0x3ffu, v >> 30); break; }
    case 61u: { uint v = *(const device uint*)p; f = float4(v & 0x3ffu, (v >> 10) & 0x3ffu, (v >> 20) & 0x3ffu, v >> 30) / float4(1023.0f, 1023.0f, 1023.0f, 3.0f); break; }
    case 62u: { uint v = *(const device uint*)p; u = uint4(v & 0x3ffu, (v >> 10) & 0x3ffu, (v >> 20) & 0x3ffu, v >> 30); integer = true; break; }
    default: break;
    }
    return integer ? u : as_type<uint4>(f);
}
"#;

pub fn translate(blob: &[u8]) -> Result<Translated> {
    translate_with(blob, 0, 0, 0)
}

pub fn translate_with(blob: &[u8], extra_outputs: u32, uint_targets: u32, sint_targets: u32) -> Result<Translated> {
    let sph = Sph::parse(&blob[0x30..]).ok_or_else(|| anyhow::anyhow!("bad SPH"))?;
    translate_code(&sph, &blob[0x80..], extra_outputs, uint_targets, sint_targets)
}

pub fn translate_compute(code: &[u8], local_memory: u32) -> Result<Translated> {
    translate_code(&Sph::compute(local_memory), code, 0, 0, 0)
}

fn translate_code(sph: &Sph, code: &[u8], extra_outputs: u32, uint_targets: u32, sint_targets: u32) -> Result<Translated> {
    let prog = Program::decode(code)?;
    let mut g = Gen {
        sph,
        prog: &prog,
        body: String::new(),
        regs: BTreeSet::new(),
        preds: BTreeSet::new(),
        uses_cc: false,
        uses_lmem: false,
        uses_point_coord: false,
        iface: Interface::default(),
        warnings: Vec::new(),
        handle_regs: BTreeMap::new(),
    };
    for i in &prog.insts {
        if prog.targets.contains(&i.addr) {
            g.body.push_str(&format!("    case {}u:\n", i.addr));
        }

        let (pred, inv) = ((i.raw >> 16) & 7, (i.raw >> 19) & 1 != 0);
        let no_pred = i.props.has(crate::table::InstProps::NOPRED);
        let guarded = !no_pred && (pred != 7 || inv) && i.name != InstName::Ssy;
        if guarded {
            let p = g.p(pred as i32, inv);
            g.line(format!("if ({p}) {{"));
        }
        g.inst(i)?;
        if guarded {
            g.line("}");
        }
    }
    g.line("return epilogue();");

    let stage = sph.stage;
    if stage == Stage::Vertex {
        g.iface.outputs |= extra_outputs;
    }
    let entry = match stage {
        Stage::Vertex => "vmain",
        Stage::Fragment => "fmain",
        Stage::Compute => "cmain",
        s => bail!("unsupported stage {s:?}"),
    };
    let mut out = String::from(PRELUDE);
    let c = ["x", "y", "z", "w"];
    let _ = c;

    if stage == Stage::Vertex {
        out.push_str("struct VOut {\n    float4 pos [[position, invariant]];\n");
        for i in 0..32 {
            if g.iface.outputs >> i & 1 != 0 {
                writeln!(out, "    float4 v{i} [[user(locn{i})]];").unwrap();
            }
        }
        out.push_str("    float psize [[point_size]];\n};\n");
    } else if stage == Stage::Fragment {
        out.push_str("struct FIn {\n    float4 pos [[position]];\n");
        for i in 0..32 {
            if g.iface.attributes >> i & 1 != 0 {
                let q = match sph.imap[i] {
                    Imap::Constant => " [[flat]]",
                    Imap::ScreenLinear => " [[center_no_perspective]]",
                    _ => "",
                };
                writeln!(out, "    float4 v{i} [[user(locn{i})]]{q};").unwrap();
            }
        }
        out.push_str("};\nstruct FOut {\n");
        let mut base = 0;
        for rt in 0..8 {
            if sph.omap_targets >> (rt * 4) & 0xF != 0 {
                let ty = if uint_targets >> rt & 1 != 0 { "uint4" } else if sint_targets >> rt & 1 != 0 { "int4" } else { "float4" };
                writeln!(out, "    {ty} c{rt} [[color({rt})]];").unwrap();
                g.iface.color_targets |= 1 << rt;
                base += 4;
            }
        }
        let _ = base;
        if sph.omap_depth {
            out.push_str("    float depth [[depth(any)]];\n");
            g.iface.writes_depth = true;
        }
        if g.iface.color_targets == 0 && !sph.omap_depth {
            out.push_str("    float4 c0 [[color(0)]];\n");
        }
        out.push_str("};\n");
    }

    let mut params = vec!["constant Res& res [[buffer(0)]]".to_string()];
    if stage == Stage::Vertex {
        params.insert(0, "uint vid [[vertex_id]], uint iid [[instance_id]]".into());
    } else if stage == Stage::Compute {
        params.insert(0, "uint3 tid [[thread_position_in_threadgroup]], uint3 ctaid [[threadgroup_position_in_grid]]".into());
    } else {
        params.insert(0, "FIn in [[stage_in]], bool front [[front_facing]]".into());
        if g.uses_point_coord {
            params.push("float2 pcoord [[point_coord]]".into());
        }
    }
    let bindings: Vec<(u32, TexKind)> = g.iface.textures.iter().map(|(b, k)| (*b, *k)).collect();
    for (slot, (b, k)) in bindings.iter().enumerate() {
        if slot >= 16 {
            bail!("more than 16 sampled textures");
        }
        g.iface.samplers.insert(*b, slot as u32);
        params.push(format!("{} t{b} [[texture({b})]], sampler s{b} [[sampler({slot})]]", k.decl()));
    }
    let (kind, ret) = match stage {
        Stage::Vertex => ("vertex", "VOut"),
        Stage::Compute => ("kernel", "void"),
        _ => ("fragment", "FOut"),
    };
    writeln!(out, "{kind} {ret} {entry}({}) {{", params.join(", ")).unwrap();

    for r in &g.regs {
        writeln!(out, "    uint r{r} = 0u;").unwrap();
    }
    for p in &g.preds {
        writeln!(out, "    bool p{p} = false;").unwrap();
    }
    if g.uses_cc {
        out.push_str("    bool zf = false, nf = false, cf = false, vf = false;\n");
    }
    if g.uses_lmem {
        let words = (sph.local_memory / 4).clamp(1, 4096).next_power_of_two();
        writeln!(out, "    uint lmem[{words}];\n    const uint LMEM_MASK = {}u;", words - 1).unwrap();
    }
    if stage == Stage::Vertex {
        out.push_str("    VOut o;\n    o.pos = float4(0.0f, 0.0f, 0.0f, 1.0f);\n    o.psize = 1.0f;\n");
        for i in 0..32 {
            if g.iface.outputs >> i & 1 != 0 {
                writeln!(out, "    o.v{i} = float4(0.0f, 0.0f, 0.0f, 1.0f);").unwrap();
            }
        }
        for i in 0..16 {
            if g.iface.attributes >> i & 1 != 0 {
                writeln!(out, "    uint4 a{i} = fetch_attr(res, {i}u, vid, iid);").unwrap();
            }
        }

        out.push_str(
            "    auto epilogue = [&]() -> VOut {\n        VOut r = o;\n        if (res.flags & 1u) r.pos.y = -r.pos.y;\n        if (res.flags & 2u) r.pos.z = (r.pos.z + r.pos.w) * 0.5f;\n        return r;\n    };\n",
        );
    } else if stage == Stage::Compute {
        out.push_str("    auto epilogue = [&]() {};\n");
    } else {

        let mut s = String::from("    auto epilogue = [&]() -> FOut {\n        FOut r;\n");
        let mut base = 0;
        for rt in 0..8 {
            let m = sph.omap_targets >> (rt * 4) & 0xF;
            if m != 0 {
                let (ty, conv, zero) = if uint_targets >> rt & 1 != 0 {
                    ("uint4", "", "0u")
                } else if sint_targets >> rt & 1 != 0 {
                    ("int4", "int", "0")
                } else {
                    ("float4", "as_type<float>", "0.0f")
                };
                let comps: Vec<String> = (0..4)
                    .map(|k| {
                        if m >> k & 1 != 0 {
                            g.regs.insert(base + k);
                            format!("{conv}(r{})", base + k)
                        } else {
                            zero.into()
                        }
                    })
                    .collect();
                writeln!(s, "        r.c{rt} = {ty}({});", comps.join(", ")).unwrap();
                base += 4;
            }
        }
        if sph.omap_depth {
            let dr = sph.omap_targets.count_ones() as i32 + 1;
            g.regs.insert(dr);
            writeln!(s, "        r.depth = as_type<float>(r{dr});").unwrap();
        }
        if g.iface.color_targets == 0 && !sph.omap_depth {
            s.push_str("        r.c0 = float4(0.0f);\n");
        }

        if sph.omap_targets & 0xF != 0 && (uint_targets | sint_targets) & 1 == 0 {
            s.push_str("        if ((res.flags & 4u) != 0u && !fcmp(res.alpha_func, r.c0.w, res.alpha_ref)) discard_fragment();\n");
        }
        s.push_str("        return r;\n    };\n");

        let declared: String = out.clone();
        for r in &g.regs {
            if !declared.contains(&format!("uint r{r} = 0u;")) {
                writeln!(out, "    uint r{r} = 0u;").unwrap();
            }
        }
        out.push_str(&s);
    }
    writeln!(out, "    uint pc = {}u;\n    for (;;) {{\n    switch (pc) {{", prog.insts[0].addr).unwrap();
    out.push_str(&g.body);
    out.push_str("    default:\n        return epilogue();\n    }\n    }\n}\n");
    let mut interface = g.iface;
    if stage == Stage::Fragment && interface.color_targets == 0 && !interface.writes_depth {
        interface.color_targets = 1;
    }
    if !g.warnings.is_empty() {
        let mut w: Vec<_> = g.warnings.clone();
        w.sort();
        w.dedup();
        out.push_str(&format!("// warnings: {}\n", w.join("; ")));
    }
    Ok(Translated { stage, msl: out, entry: entry.into(), interface })
}
