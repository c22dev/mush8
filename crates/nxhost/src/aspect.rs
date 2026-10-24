// Mush8 - aspect.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Context, Result};
use nxfs::bflan::Animation;
use nxfs::bflyt::{Axis, Layout, PaneEdit};
use nxfs::sarc;
use nxfs::storage::DynStorage;
use std::sync::{LazyLock, OnceLock};

const NATIVE: f64 = 16.0 / 9.0;

#[derive(Clone, Copy, Debug)]
pub struct Screen {
    pub ratio: f64,
    pub safe_ratio: f64,
}

static SCREEN: OnceLock<Screen> = OnceLock::new();

pub fn set_screen(width: f64, height: f64, safe_width: f64) {
    if width > 0.0 && height > 0.0 {
        let _ = SCREEN.set(Screen { ratio: width / height, safe_ratio: safe_width.clamp(1.0, width) / height });
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Target {
    ratio: f64,
    hud_ratio: f64,
}

static TARGET: LazyLock<Option<Target>> = LazyLock::new(|| {
    let t = target(std::env::var("NX_ASPECT").ok().as_deref(), std::env::var("NX_HUD").ok().as_deref(), screen());
    match t {
        Some(t) => eprintln!("[aspect] {:.3}:1 (HUD spread to {:.3}:1)", t.ratio, t.hud_ratio),
        None if std::env::var("NX_ASPECT").is_ok_and(|v| v == "auto") => eprintln!("[aspect] screen is 16:9 or unknown: staying at 16:9"),
        None => {}
    }
    t
});

#[cfg(target_os = "macos")]
fn screen() -> Option<Screen> {
    if let Some(s) = SCREEN.get() {
        return Some(*s);
    }
    let mtm = objc2::MainThreadMarker::new()?;
    let s = objc2_app_kit::NSScreen::mainScreen(mtm)?;
    let size = s.frame().size;
    let height = size.height - s.safeAreaInsets().top;
    set_screen(size.width, height, size.width);
    SCREEN.get().copied()
}

#[cfg(target_os = "ios")]
fn screen() -> Option<Screen> {
    SCREEN.get().copied()
}

fn parse_ratio(v: &str) -> Option<f64> {
    let r = match v.split_once(':') {
        Some((w, h)) => w.trim().parse::<f64>().ok()? / h.trim().parse::<f64>().ok()?,
        None => v.trim().parse().ok()?,
    };
    (r.is_finite() && r > 0.0).then_some(r)
}

fn target(aspect: Option<&str>, hud: Option<&str>, screen: Option<Screen>) -> Option<Target> {
    let (ratio, safe) = match aspect?.trim() {
        "auto" => {
            let s = screen?;
            (s.ratio, s.safe_ratio)
        }
        v => {
            let r = parse_ratio(v)?;
            (r, r)
        }
    };
    let ratio = ratio.clamp(1.0, 4.0);
    if (ratio / NATIVE - 1.0).abs() < 0.01 {
        return None;
    }
    let hud_ratio = match hud {
        Some("center") => NATIVE,
        _ if ratio > NATIVE => safe.clamp(NATIVE, ratio),
        _ => ratio,
    };
    Some(Target { ratio, hud_ratio })
}

pub fn ratio() -> f64 {
    TARGET.map_or(NATIVE, |t| t.ratio)
}

pub fn stretch() -> f64 {
    ratio() / NATIVE
}

fn hud_scale(v: Option<&str>) -> f32 {
    v.and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).map_or(1.0, |v| v.clamp(0.5, 1.5))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Fit,
    Fill,
    Cover,
    Zoom,
    Hud,
}

struct Rule {
    layout: &'static str,
    panes: &'static [&'static str],
    op: Op,
}

const CONTENT_FITTED: &[&str] = &["cm_LoadScreen_00", "mn_L_CharaWin_00"];

const STRETCHED_ROOTS: &[&str] = &["rc_RaceView_1P_00", "rc_Viewer_00", "cm_Fade_Pause_00", "cm_Page_Fader", "cm_Page_FaderPause", "mn_L_BlurBG_00", "mn_Page_Present_Parts", "mn_Page_Present_Chara", "rc_Page_RaceReplay_Award"];

const RULES: &[Rule] = &[
    Rule { layout: "rc_RaceView_1P_00", panes: &["N_Pause_00", "N_Pause_01"], op: Op::Fit },
    Rule { layout: "rc_RaceView_1P_00", panes: &["N_ItemBoxPos_00", "L_Rank_00", "L_LapCoin_00", "L_BtScore_00", "L_BtCoinCount_00", "L_BtKeidoroCoin_00"], op: Op::Hud },
    Rule { layout: "rc_RaceView_Cmn_00", panes: &["L_BtTimer_Ml", "L_BtKeidoroWantedPos00", "L_BtTeamScore_00", "L_ShineScore_00"], op: Op::Hud },
    Rule { layout: "rc_RaceView_2P_Ml", panes: &["N_BtScore_MlTeamPos_00", "L_BtCoinCount_00", "L_BtKeidoroCoin_00"], op: Op::Hud },
    Rule { layout: "rc_RaceView_4P_Ml", panes: &["N_BtScore_MlTeamPos_00", "L_BtCoinCount_00", "L_BtKeidoroCoin_00"], op: Op::Hud },
    Rule { layout: "rc_RaceView_Cmn_00", panes: &["L_Fade_00"], op: Op::Fill },
    Rule { layout: "rc_RaceView_2P_Ml", panes: &["L_Fade_00"], op: Op::Fill },
    Rule { layout: "rc_RaceView_4P_Ml", panes: &["L_Fade_00"], op: Op::Fill },
    Rule { layout: "mn_Background_00", panes: &["P_Sparkle_00", "P_Monogram_00", "N_TitelePicAll_00"], op: Op::Cover },
    Rule { layout: "mn_TitleScene_00", panes: &["N_Capture_00", "N_Capture_01"], op: Op::Cover },
    Rule { layout: "mn_ModeSlct_00", panes: &["N_Capture_00"], op: Op::Zoom },
    Rule { layout: "mn_WifiModeSlct_00", panes: &["N_Capture_00"], op: Op::Zoom },
    Rule { layout: "mn_BattleSlct_00", panes: &["N_Capture_00"], op: Op::Zoom },
    Rule { layout: "mn_GrprClassSlct_00", panes: &["N_Capture_00"], op: Op::Zoom },
    Rule { layout: "bt_TitleLoading_00", panes: &["N_CheckBG_00"], op: Op::Cover },
    Rule { layout: "cm_LoadWin_00", panes: &["N_All_00"], op: Op::Cover },
    // the game puts each unlock gift on its slot's on-screen position, which is already narrowed: keep the page
    // root as is (STRETCHED_ROOTS) and narrow only the gifts
    Rule {
        layout: "mn_Page_Present_Parts",
        panes: &[
            "mn_L_KartParts_Gift_00_0",
            "mn_L_KartParts_Gift_00_1",
            "mn_L_KartParts_Gift_00_2",
            "mn_L_KartParts_Gift_00_3",
            "mn_L_KartParts_Gift_00_4",
            "mn_L_KartParts_Gift_00_5",
            "mn_L_KartParts_Gift_00_6",
            "mn_L_KartParts_Gift_00_7",
        ],
        op: Op::Fit,
    },
    Rule { layout: "mn_Page_Present_Chara", panes: &["mn_L_CharaSlct_Gift_00_0"], op: Op::Fit },
];

const NOT_BACKDROPS: &[&str] = &["Logo", "Capture", "Movie", "View"];
const PANELS: &[&str] = &["Blur", "Base", "BG", "Board", "Black", "Mask", "Win", "Partition"];

const FULL: [f32; 2] = [1270.0, 700.0];

fn extent(p: &nxfs::bflyt::Pane, i: usize) -> f32 {
    p.size[i] * p.scale[i].abs()
}

fn is_pattern(p: &nxfs::bflyt::Pane) -> bool {
    &p.kind == b"pic1" && p.name.contains("Ptn")
}

fn is_backdrop(p: &nxfs::bflyt::Pane, axis: Axis) -> bool {
    let i = (axis == Axis::Y) as usize;
    let screen = (0..2).all(|j| extent(p, j) >= FULL[j] && p.translate[j].abs() < 1.0);
    let panel = (&p.kind == b"wnd1" || PANELS.iter().any(|n| p.name.contains(n)))
        && (FULL[i]..=FULL[i] * 1.06).contains(&extent(p, i))
        && p.translate[i].abs() < 1.0;
    (&p.kind == b"pic1" || &p.kind == b"wnd1") && (screen || panel) && !NOT_BACKDROPS.iter().any(|n| p.name.contains(n))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Edit {
    Scale(Axis, f32),
    Translate(Axis, f32),
    Shift(Axis, f32),
    Size(Axis, f32),
    Tile(Axis, f32),
}

struct Plan {
    axis: Axis,
    stretch: f32,
    edge: f32,
    hud: f32,
}

impl Plan {
    fn new(target: Option<Target>, hud: f32) -> Option<Self> {
        let Some(t) = target else {
            return (hud != 1.0).then_some(Self { axis: Axis::X, stretch: 1.0, edge: 1.0, hud });
        };
        Some(if t.ratio >= NATIVE {
            Self { axis: Axis::X, stretch: (t.ratio / NATIVE) as f32, edge: (t.hud_ratio / NATIVE) as f32, hud }
        } else {
            let s = (NATIVE / t.ratio) as f32;
            Self { axis: Axis::Y, stretch: s, edge: if t.hud_ratio == NATIVE { 1.0 } else { s }, hud }
        })
    }

    fn other(&self) -> Axis {
        if self.axis == Axis::X { Axis::Y } else { Axis::X }
    }

    fn edits(&self, name: &str, l: &Layout) -> Vec<(String, Edit)> {
        let (a, b, s) = (self.axis, self.other(), self.stretch);
        let rules: Vec<&Rule> = RULES.iter().filter(|r| r.layout == name).collect();
        let ruled = |pane: &str| rules.iter().any(|r| r.panes.contains(&pane));
        let content_fitted = CONTENT_FITTED.contains(&name) || l.panes().iter().any(|p| p.user_data.iter().any(|u| u == "FrameBufferUse") && is_backdrop(p, a));
        let stretched = STRETCHED_ROOTS.contains(&name) || content_fitted;
        let mut out = Vec::new();
        let mut add = |pane: &str, e: Edit| out.push((pane.to_string(), e));
        if s != 1.0 && content_fitted {
            for p in fitted_content(l, a) {
                add(&p.name, Edit::Scale(a, 1.0 / s));
                add(&p.name, Edit::Translate(a, 1.0 / s));
            }
            let i = (a == Axis::Y) as usize;
            for p in l.panes().iter().filter(|p| is_pattern(p) && p.translate[i].abs() < 1.0 && extent(p, i) >= FULL[i]) {
                add(&p.name, Edit::Tile(a, s));
            }
        }
        if s != 1.0 && !stretched {
            add("RootPane", Edit::Scale(a, 1.0 / s));
        }
        for rule in &rules {
            for p in rule.panes.iter().filter_map(|n| l.pane(n)) {
                match rule.op {
                    _ if s == 1.0 && rule.op != Op::Hud => {}
                    Op::Fit => add(&p.name, Edit::Scale(a, 1.0 / s)),
                    Op::Fill => add(&p.name, Edit::Scale(a, s)),
                    Op::Cover => {
                        add(&p.name, Edit::Scale(a, s));
                        add(&p.name, Edit::Scale(b, s));
                    }
                    Op::Zoom => add(&p.name, Edit::Scale(b, s)),
                    Op::Hud => {
                        let root = l.pane("RootPane").map_or([1280.0, 720.0], |r| r.size);
                        for (axis, spread) in [(a, self.edge), (b, 1.0)] {
                            let i = (axis == Axis::Y) as usize;
                            if spread * self.hud != 1.0 {
                                add(&p.name, Edit::Translate(axis, spread * self.hud));
                            }
                            if self.hud != 1.0 {
                                add(&p.name, Edit::Shift(axis, p.translate[i].signum() * root[i] / 2.0 * spread * (1.0 - self.hud)));
                                add(&p.name, Edit::Scale(axis, self.hud));
                            }
                        }
                    }
                }
            }
        }
        if s != 1.0 && !stretched {
            let leaf = |p: &nxfs::bflyt::Pane| !l.panes().iter().any(|q| l.ancestors(q).next().is_some_and(|a| std::ptr::eq(a, p)));
            for p in l.panes().iter().filter(|p| is_backdrop(p, a) && leaf(p) && !ruled(&p.name) && !l.ancestors(p).any(|q| ruled(&q.name))) {
                add(&p.name, Edit::Size(a, s));
                if is_pattern(p) {
                    add(&p.name, Edit::Tile(a, s));
                }
            }
        }
        out
    }
}

fn apply(edits: &[(String, Edit)], f: &mut impl PaneEdit) -> bool {
    edits.iter().fold(false, |changed, (pane, edit)| {
        changed
            | match *edit {
                Edit::Scale(axis, by) => f.scale(pane, axis, by),
                Edit::Translate(axis, by) => f.translate(pane, axis, by),
                Edit::Shift(axis, by) => f.shift(pane, axis, by),
                Edit::Size(axis, by) => f.resize(pane, axis, by),
                Edit::Tile(axis, by) => f.tile(pane, axis, by),
            }
    })
}

fn fitted_content<'l>(l: &'l Layout, axis: Axis) -> Vec<&'l nxfs::bflyt::Pane> {
    let i = (axis == Axis::Y) as usize;
    let panes = l.panes();
    let children = |parent: usize| panes.iter().filter(move |p| p.parent == Some(parent));
    let index = |p: &nxfs::bflyt::Pane| panes.iter().position(|q| std::ptr::eq(q, p));
    let mut out = Vec::new();
    let mut stack: Vec<usize> = panes.iter().position(|p| p.name == "RootPane").into_iter().collect();
    while let Some(parent) = stack.pop() {
        for p in children(parent) {
            let centred = p.translate[i].abs() < 1.0;
            let group = &p.kind == b"pan1" && centred && p.scale[i] == 1.0;
            let spans = centred && extent(p, i) >= FULL[i] && !NOT_BACKDROPS.iter().any(|n| p.name.contains(n));
            if group || spans {
                stack.extend(index(p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

static PLAN: LazyLock<Option<Plan>> = LazyLock::new(|| Plan::new(*TARGET, hud_scale(std::env::var("NX_HUD_SCALE").ok().as_deref())));

fn layout_name(entry: &str) -> &str {
    let file = entry.rsplit('/').next().unwrap_or(entry);
    file.strip_suffix(".szs").unwrap_or(file)
}

fn patch_layouts(name: &str, szs: &[u8], plan: &Plan) -> Result<Option<Vec<u8>>> {
    let mut raw = sarc::yaz0(szs)?;
    if !raw.starts_with(b"SARC") {
        return Ok(None);
    }
    let base = raw.as_ptr() as usize;
    let files: Vec<(usize, usize)> = sarc::sarc(&raw)?.iter().map(|e| (e.data.as_ptr() as usize - base, e.data.len())).collect();
    let mut changed = false;
    let mut edits = Vec::new();
    for &(o, len) in &files {
        let file = &mut raw[o..o + len];
        if file.starts_with(b"FLYT") {
            let mut l = Layout::parse(file).with_context(|| name.to_string())?;
            edits = plan.edits(name, &l);
            changed |= apply(&edits, &mut l);
        }
    }
    for &(o, len) in &files {
        let file = &mut raw[o..o + len];
        if file.starts_with(b"FLAN") {
            changed |= apply(&edits, &mut Animation::parse(file).with_context(|| name.to_string())?);
        }
    }
    Ok(changed.then(|| sarc::yaz0_compress(&raw, sarc::yaz0_tail(szs))))
}

fn is_layout_archive(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/UI/") else { return false };
    let file = rest.rsplit('/').next().unwrap_or(rest);
    (file.ends_with(".sarc") && !["font", "message", "filter"].iter().any(|p| file.starts_with(p))) || rest.starts_with("cmn/boot/")
}

fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(items.len().max(1));
    let mut out: Vec<(usize, R)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break done };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

pub fn romfs(storage: DynStorage) -> Result<DynStorage> {
    let Some(plan) = PLAN.as_ref() else { return Ok(storage) };
    let started = std::time::Instant::now();
    let fs = nxfs::romfs::RomFs::open(storage)?;
    let mut archives = Vec::new();
    for f in fs.files().into_iter().filter(|f| is_layout_archive(&f.path)) {
        let data = fs.storage().read_vec(f.offset, f.size as usize)?;
        archives.push((f.path, data));
    }
    let mut jobs = Vec::new();
    for (i, (path, data)) in archives.iter().enumerate() {
        if path.ends_with(".szs") {
            jobs.push((i, None, layout_name(path).to_string(), &data[..]));
        } else {
            for (k, e) in sarc::sarc(data).with_context(|| path.clone())?.into_iter().enumerate() {
                if e.name.ends_with(".szs") {
                    jobs.push((i, Some(k), layout_name(&e.name).to_string(), e.data));
                }
            }
        }
    }
    let results = parallel(&jobs, |(i, _, name, szs)| patch_layouts(name, szs, plan).with_context(|| format!("{} {name}", archives[*i].0)));
    let mut patched: Vec<Vec<(Option<usize>, Vec<u8>)>> = archives.iter().map(|_| Vec::new()).collect();
    for (&(i, k, _, _), r) in jobs.iter().zip(results) {
        if let Some(new) = r? {
            patched[i].push((k, new));
        }
    }
    let mut replacements = Vec::new();
    for ((path, data), mut changes) in archives.into_iter().zip(patched) {
        let whole_file = matches!(changes.as_slice(), [(None, _)]);
        let new = match changes.len() {
            0 => continue,
            _ if whole_file => changes.pop().unwrap().1,
            _ => {
                let entries = sarc::sarc(&data)?;
                let mut contents: Vec<&[u8]> = entries.iter().map(|e| e.data).collect();
                for (k, new) in &changes {
                    contents[k.context("layout archive inside a layout archive")?] = new;
                }
                sarc::rebuild(&data, &contents)?
            }
        };
        replacements.push((path, new));
    }
    eprintln!("[aspect] {} UI archives adapted in {:.2} s", replacements.len(), started.elapsed().as_secs_f32());
    fs.with_files(replacements)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(ratio: f64, safe_ratio: f64) -> Option<Screen> {
        Some(Screen { ratio, safe_ratio })
    }

    #[test]
    fn targets() {
        assert_eq!(target(None, None, screen(2.17, 2.0)), None);
        assert_eq!(target(Some("16:9"), None, None), None);
        assert_eq!(target(Some("auto"), None, None), None);
        assert_eq!(target(Some("auto"), None, screen(16.0 / 9.0, 16.0 / 9.0)), None);
        assert_eq!(target(Some("21:9"), None, None), Some(Target { ratio: 21.0 / 9.0, hud_ratio: 21.0 / 9.0 }));
        assert_eq!(target(Some("auto"), None, screen(2.17, 2.0)), Some(Target { ratio: 2.17, hud_ratio: 2.0 }));
        assert_eq!(target(Some("auto"), Some("center"), screen(2.17, 2.0)), Some(Target { ratio: 2.17, hud_ratio: NATIVE }));
        assert_eq!(target(Some("4:3"), None, None), Some(Target { ratio: 4.0 / 3.0, hud_ratio: 4.0 / 3.0 }));
        assert_eq!(target(Some("1.6"), Some("edges"), None), Some(Target { ratio: 1.6, hud_ratio: 1.6 }));
        assert_eq!(target(Some("wide"), None, None), None);
    }

    #[test]
    fn plans() {
        let p = Plan::new(Some(Target { ratio: 32.0 / 9.0, hud_ratio: 21.0 / 9.0 }), 1.0).unwrap();
        assert_eq!((p.axis, p.stretch, p.edge), (Axis::X, 2.0, 21.0 / 16.0));
        let p = Plan::new(Some(Target { ratio: 4.0 / 3.0, hud_ratio: 4.0 / 3.0 }), 1.0).unwrap();
        assert_eq!((p.axis, p.stretch, p.edge), (Axis::Y, 4.0 / 3.0, 4.0 / 3.0));
        assert!(Plan::new(None, 1.0).is_none());
        assert_eq!(Plan::new(None, 0.8).map(|p| (p.stretch, p.hud)), Some((1.0, 0.8)));
        assert_eq!((hud_scale(Some("0.75")), hud_scale(Some("9")), hud_scale(Some("x")), hud_scale(None)), (0.75, 1.5, 1.0, 1.0));
        assert_eq!(layout_name("blyt/x/rc_RaceView_1P_00.szs"), "rc_RaceView_1P_00");
        assert!(is_layout_archive("/UI/cmn/race.sarc") && is_layout_archive("/UI/EUen/menu.sarc") && is_layout_archive("/UI/cmn/boot/bt_TitleLoading_00.szs"));
        assert!(!is_layout_archive("/UI/EUen/font.sarc") && !is_layout_archive("/UI/cmn/menu_title/MnOpen_Link^l.szs") && !is_layout_archive("/Course/x.szs"));
    }

    fn layout(panes: &[(&[u8; 4], &str, [f32; 2], [f32; 2])]) -> Vec<u8> {
        let mut d = b"FLYT\xFF\xFE\x14\x00".to_vec();
        d.resize(0x14, 0);
        for (i, (kind, name, translate, size)) in panes.iter().enumerate() {
            let mut p = vec![0u8; 0x54];
            p[..4].copy_from_slice(*kind);
            p[4..8].copy_from_slice(&0x54u32.to_le_bytes());
            p[0xC..0xC + name.len()].copy_from_slice(name.as_bytes());
            for (at, v) in [(0x2C, translate[0]), (0x30, translate[1]), (0x44, 1.0), (0x48, 1.0), (0x4C, size[0]), (0x50, size[1])] {
                p[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
            d.extend(p);
            if i == 0 {
                d.extend(b"pas1\x08\0\0\0");
            }
        }
        d.extend(b"pae1\x08\0\0\0");
        d
    }

    #[test]
    fn edits_fit_fill_and_hud() {
        let mut d = layout(&[
            (b"pan1", "RootPane", [0.0, 0.0], [1280.0, 720.0]),
            (b"pic1", "P_BG_00", [0.0, 0.0], [1280.0, 720.0]),
            (b"pic1", "P_Logo_00", [0.0, 0.0], [1280.0, 720.0]),
            (b"wnd1", "W_Bar_00", [0.0, 300.0], [1280.0, 90.0]),
            (b"pic1", "P_Icon_00", [0.0, 0.0], [1280.0, 90.0]),
            (b"prt1", "L_BtTimer_Ml", [518.0, 305.0], [150.0, 64.0]),
        ]);
        let ratio = 2.17;
        let plan = Plan::new(Some(Target { ratio, hud_ratio: ratio }), 0.8).unwrap();
        let mut l = Layout::parse(&mut d).unwrap();
        let edits = plan.edits("rc_RaceView_Cmn_00", &l);
        assert!(apply(&edits, &mut l));
        let l = Layout::parse(&mut d).unwrap();
        let (s, k) = ((ratio / NATIVE) as f32, 0.8f32);
        let close = |a: [f32; 2], b: [f32; 2]| assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3, "{a:?} != {b:?}");
        close(l.pane("RootPane").unwrap().scale, [1.0 / s, 1.0]);
        close(l.pane("P_BG_00").unwrap().size, [1280.0 * s, 720.0]);
        close(l.pane("P_Logo_00").unwrap().scale, [1.0, 1.0]);
        close(l.pane("W_Bar_00").unwrap().size, [1280.0 * s, 90.0]);
        close(l.pane("P_Icon_00").unwrap().scale, [1.0, 1.0]);
        let hud = l.pane("L_BtTimer_Ml").unwrap();
        close(hud.scale, [k, k]);
        close(hud.translate, [518.0 * s * k + 640.0 * s * (1.0 - k), 305.0 * k + 360.0 * (1.0 - k)]);
    }

    #[test]
    fn content_fitted_layouts_keep_full_width_panes() {
        let mut d = layout(&[
            (b"pan1", "RootPane", [0.0, 0.0], [1280.0, 720.0]),
            (b"pic1", "P_FadeBG_00", [0.0, 0.0], [1280.0, 720.0]),
            (b"pic1", "P_Logo_00", [0.0, 0.0], [1280.0, 720.0]),
            (b"pan1", "N_Loop_00", [-300.0, 0.0], [30.0, 40.0]),
        ]);
        let ratio = 2.17;
        let plan = Plan::new(Some(Target { ratio, hud_ratio: ratio }), 1.0).unwrap();
        let l = Layout::parse(&mut d).unwrap();
        let s = (ratio / NATIVE) as f32;
        assert_eq!(
            plan.edits("cm_LoadScreen_00", &l),
            [
                ("P_Logo_00".into(), Edit::Scale(Axis::X, 1.0 / s)),
                ("P_Logo_00".into(), Edit::Translate(Axis::X, 1.0 / s)),
                ("N_Loop_00".into(), Edit::Scale(Axis::X, 1.0 / s)),
                ("N_Loop_00".into(), Edit::Translate(Axis::X, 1.0 / s)),
            ]
        );
    }

    // The game moves each gift onto the slot it unwraps, using the slot's position on screen. That position is
    // already narrowed, so the page must not narrow it again: only the gift itself is narrowed, around its centre.
    #[test]
    fn unlock_gifts_follow_their_slot() {
        let ratio = 2.17;
        let s = (ratio / NATIVE) as f32;
        let plan = Plan::new(Some(Target { ratio, hud_ratio: ratio }), 1.0).unwrap();
        for (page, gifts) in [
            ("mn_Page_Present_Parts", (0..8).map(|i| format!("mn_L_KartParts_Gift_00_{i}")).collect::<Vec<_>>()),
            ("mn_Page_Present_Chara", vec!["mn_L_CharaSlct_Gift_00_0".to_string()]),
        ] {
            let mut panes = vec![(b"pan1", "RootPane", [0.0, 0.0], [1280.0, 720.0]), (b"prt1", "mn_GiftBG_00", [0.0, 0.0], [1280.0, 720.0])];
            panes.extend(gifts.iter().map(|g| (b"prt1", g.as_str(), [0.0, 0.0], [253.7, 171.0])));
            let mut d = layout(&panes);
            let l = Layout::parse(&mut d).unwrap();
            let edits = plan.edits(page, &l);
            assert!(!edits.iter().any(|(p, _)| p == "RootPane"), "{page}: {edits:?}");
            for g in &gifts {
                assert_eq!(edits.iter().filter(|(p, _)| p == g).collect::<Vec<_>>(), [&(g.clone(), Edit::Scale(Axis::X, 1.0 / s))], "{page}");
            }
        }
    }

    // The end-of-cup highlight reel shows rc_HighlightAward_00 inside this page. That part already keeps its blurred
    // grey cover and its scrolling flag bands full width and narrows only its content, so the page must not narrow
    // it again, or the cover and bands shrink to 16:9 and the content is narrowed twice.
    #[test]
    fn cup_highlight_reel_page_is_not_narrowed() {
        let mut d = layout(&[
            (b"pan1", "RootPane", [0.0, 0.0], [1280.0, 720.0]),
            (b"prt1", "rc_HighlightAward_00", [0.0, 0.0], [1442.1, 2433.4]),
            (b"prt1", "rc_Viewer_00", [0.0, 0.0], [1280.0, 720.0]),
        ]);
        let plan = Plan::new(Some(Target { ratio: 2.17, hud_ratio: 2.17 }), 1.0).unwrap();
        let l = Layout::parse(&mut d).unwrap();
        assert_eq!(plan.edits("rc_Page_RaceReplay_Award", &l), []);
    }
}
