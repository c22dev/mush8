use std::sync::{
    atomic::{AtomicU32, Ordering::Relaxed},
    OnceLock,
};

#[cfg(target_os = "ios")]
const ORIGINAL_ASPECT: f64 = 16.0 / 9.0;
const TEXTURED_UI_VERTEX: u64 = 0x77264c28fdc0921b;
const PATTERN_UI_VERTEX: u64 = 0x8a167a1501240ee5;
const INTRO_ART_VERTEX: u64 = 0xff58c483a97cf8ef;
const INTRO_ART_FRAGMENT: u64 = 0xcc03897b25330f90;
const INTRO_COMPOSITE_VERTEX: u64 = 0x305d4f5c810d7403;
const INTRO_COMPOSITE_FRAGMENT: u64 = 0x16182a4ea0da75ec;
const TEXTURED_UI_FRAGMENT: u64 = 0x5bb31027ff12d02e;
const RGBA16_FLOAT: u64 = 90;
const OPAQUE_COLOR_STATE: u32 = 0x08030000;

static DISPLAY_SCALE: AtomicU32 = AtomicU32::new(1.0f32.to_bits());
static SCALE: OnceLock<f32> = OnceLock::new();

#[cfg(target_os = "ios")]
pub fn set_display(w: f64, h: f64) {
    if w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite() {
        DISPLAY_SCALE.store(((w.max(h) / w.min(h) / ORIGINAL_ASPECT) as f32).clamp(1.0, 2.0).to_bits(), Relaxed);
    }
}

pub fn scale() -> f32 {
    *SCALE.get_or_init(|| {
        let scale = if std::env::var("NX_ULTRAWIDE").as_deref() == Ok("1") { f32::from_bits(DISPLAY_SCALE.load(Relaxed)) } else { 1.0 };
        eprintln!("[gfx] ultrawide aspect scale {scale:.6}");
        scale
    })
}

// These are MK8DX 4.0.1's observed UI vertex programs, not scene or resolve programs.
pub fn is_ui(hash: u64, w: u32, h: u32) -> bool {
    w >= 640
        && h >= 360
        && w * 9 == h * 16
        && matches!(
            hash,
            TEXTURED_UI_VERTEX | PATTERN_UI_VERTEX | INTRO_ART_VERTEX | 0xddb6b360577ab6ff | 0x52454081518e814e | INTRO_COMPOSITE_VERTEX | 0x684ee55e995f1d67 | 0x861a8981f7511caa
        )
}

pub fn center_x(x: f32, width: f32, scale: f32) -> f32 {
    width * 0.5 + (x - width * 0.5) / scale
}

pub fn cover_y(y: f32, height: f32, scale: f32) -> f32 {
    height * 0.5 + (y - height * 0.5) * scale
}

pub fn starts_menu_art(hash: u64, depth_stencil: u32, texture: Option<(u32, u32, bool)>) -> bool {
    match texture {
        Some((8, 1048, _)) => matches!(hash, TEXTURED_UI_VERTEX | PATTERN_UI_VERTEX) && matches!(menu_depth_state(depth_stencil), 0x40 | 0x48),
        Some(texture) if is_menu_illustration(texture) => hash == TEXTURED_UI_VERTEX && menu_depth_state(depth_stencil) == 0x40,
        _ => false,
    }
}

// The main menu uses both full-resolution artwork and half-resolution artwork
// (records, amiibo, MKTV, help and Toy-Con). Both have the same overlay passes.
pub fn is_menu_illustration(texture: (u32, u32, bool)) -> bool {
    matches!(texture, (1920, 1080, true) | (960, 540, true))
}

pub fn is_menu_fade(hash: u64, depth_stencil: u32, texture: Option<(u32, u32, bool)>) -> bool {
    matches!(hash, TEXTURED_UI_VERTEX | PATTERN_UI_VERTEX) && menu_depth_state(depth_stencil) == 0x40 && texture == Some((8, 8, true))
}

pub fn starts_intro_art(vs: u64, fs: u64, depth_stencil: u32, texture: Option<(u32, u32, bool)>) -> bool {
    // The opening animation composites its artwork with a separate mask program.
    vs == INTRO_ART_VERTEX && fs == INTRO_ART_FRAGMENT && matches!(menu_depth_state(depth_stencil), 0x40 | 0x48) && matches!(texture, Some((1280, 720 | 420, false)))
}

pub fn is_intro_composite(vs: u64, fs: u64) -> bool {
    // This pass samples the already covered title framebuffer, not original art.
    vs == INTRO_COMPOSITE_VERTEX && fs == INTRO_COMPOSITE_FRAGMENT
}

pub fn is_menu_backdrop(depth: u32, mask: (u32, u32), background: (u32, u32, u64)) -> bool {
    // Menu masks sample the downscaled screen, which already contains the
    // widescreen background. Applying the UI correction again creates seams.
    matches!(menu_depth_state(depth), 0x40 | 0x48) && matches!(mask, (80, 45 | 39) | (4, 45) | (8, 8) | (16, 16)) && background == (320, 180, RGBA16_FLOAT)
}

pub fn is_frame_copy(vs: u64, fs: u64, depth: u32, color: u32, texture: (u32, u32, u64)) -> bool {
    let (w, h, format) = texture;
    vs == TEXTURED_UI_VERTEX
        && fs == TEXTURED_UI_FRAGMENT
        && menu_depth_state(depth) == 0x40
        && color == OPAQUE_COLOR_STATE
        && format == RGBA16_FLOAT
        && w >= 640
        && h >= 360
        && w * 9 == h * 16
}

pub fn menu_depth_state(depth_stencil: u32) -> u32 {
    // The renderer decodes flags and depth comparison from the low byte only.
    // Guest state padding changes during transitions (e.g. 0x40 -> 0x3f800040).
    depth_stencil & 0xff
}

// A composited screen is already in display space. Its blend mode and pixel
// format do not change that space (fades and downsampled masks included).
pub fn is_screen_composite(size: (u32, u32), viewport: Option<[f32; 4]>, depth: u32, vertices: u32, instances: u32, rendered_source: impl FnOnce() -> bool) -> bool {
    let (w, h) = size;
    matches!(menu_depth_state(depth), 0x40 | 0x48)
        && matches!(vertices, 3 | 4 | 6)
        && instances == 1
        && viewport.is_none_or(|v| v == [0.0, 0.0, w as f32, h as f32])
        && rendered_source()
}

pub fn is_screen_size(width: u32, height: u32) -> bool {
    width >= 320 && height >= 180 && u64::from(width) * 9 == u64::from(height) * 16
}

#[derive(Clone, Copy, Default)]
pub struct ArtworkState {
    pub covering: bool,
    pub main_menu: bool,
}

impl ArtworkState {
    pub fn layout(self, screen_space: bool, fade: bool, depth: u32) -> Layout {
        if screen_space {
            Layout::Original
        } else if self.covering {
            Layout::CoverArtwork
        } else if fade || (self.main_menu && menu_depth_state(depth) == 0x48) {
            Layout::Original
        } else {
            Layout::CenterUi
        }
    }

    pub fn observe(&mut self, ui: bool, vs: u64, fs: u64, depth: u32, texture: Option<(u32, u32, bool)>) {
        if !ui || !matches!(menu_depth_state(depth), 0x40 | 0x48) {
            *self = Self::default();
            return;
        }
        self.covering |= starts_menu_art(vs, depth, texture) || starts_intro_art(vs, fs, depth, texture);
        self.main_menu |= self.covering && texture.is_some_and(is_menu_illustration);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Original,
    CoverArtwork,
    CenterUi,
}

pub struct DisplayTransform {
    pub size: (u32, u32),
    pub scale: f32,
}

impl DisplayTransform {
    pub fn apply(&self, layout: Layout, viewport: Option<[f32; 4]>, scissor: [u32; 4]) -> (Option<[f32; 4]>, [u32; 4]) {
        if layout == Layout::Original {
            return (viewport, scissor);
        }
        let (width, height) = self.size;
        let mut viewport = viewport.unwrap_or([0.0, 0.0, width as f32, height as f32]);
        let mut scissor = scissor;
        let (axis, extent) = match layout {
            Layout::CoverArtwork => (1, height as f32),
            Layout::CenterUi => (0, width as f32),
            Layout::Original => unreachable!(),
        };
        let transform = |position| match layout {
            Layout::CoverArtwork => cover_y(position, extent, self.scale),
            _ => center_x(position, extent, self.scale),
        };
        viewport[axis] = transform(viewport[axis]);
        viewport[axis + 2] = match layout {
            Layout::CenterUi => viewport[axis + 2] / self.scale,
            _ => viewport[axis + 2] * self.scale,
        };
        let start = transform(scissor[axis] as f32).floor().clamp(0.0, extent) as u32;
        let end = transform((scissor[axis] + scissor[axis + 2]) as f32).ceil().clamp(0.0, extent) as u32;
        scissor[axis] = start;
        scissor[axis + 2] = end.saturating_sub(start);
        (Some(viewport), scissor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_screens_keep_their_space_through_compositing() {
        for size in [(1280, 720), (1920, 1080)] {
            for depth in [0x40, 0x48, 0x3f800040, 0x500fef48] {
                for count in [3, 4, 6] {
                    for viewport in [None, Some([0.0, 0.0, size.0 as f32, size.1 as f32])] {
                        assert!(is_screen_composite(size, viewport, depth, count, 1, || true));
                        assert!(!is_screen_composite(size, viewport, depth, count, 1, || false));
                    }
                }
            }
        }
    }

    #[test]
    fn partial_viewports_and_scene_geometry_are_not_screen_composites() {
        let size = (1920, 1080);
        assert!(!is_screen_composite(size, Some([100.0, 100.0, 640.0, 360.0]), 0x40, 6, 1, || panic!("non-composite draw must not query texture sources")));
        for depth in [0x41, 0x43, 0x44] {
            assert!(!is_screen_composite(size, None, depth, 6, 1, || panic!("non-composite draw must not query texture sources")));
        }
        assert!(!is_screen_composite(size, None, 0x40, 6, 2, || panic!("non-composite draw must not query texture sources")));
        assert!(!is_screen_composite(size, None, 0x40, 300, 1, || panic!("non-composite draw must not query texture sources")));
    }

    #[test]
    fn screen_sources_include_downsampled_buffers_but_not_icons() {
        for size in [(320, 180), (640, 360), (1280, 720), (1920, 1080)] {
            assert!(is_screen_size(size.0, size.1));
        }
        for size in [(0, 0), (16, 16), (170, 105), (400, 400), (1280, 420)] {
            assert!(!is_screen_size(size.0, size.1));
        }
    }

    #[test]
    fn artwork_sequences_are_independent_and_scene_draws_end_them() {
        let mut background = ArtworkState::default();
        background.observe(true, TEXTURED_UI_VERTEX, TEXTURED_UI_FRAGMENT, 0x40, Some((1920, 1080, true)));
        let mut overlay = ArtworkState::default();
        overlay.observe(true, TEXTURED_UI_VERTEX, TEXTURED_UI_FRAGMENT, 0x48, Some((16, 16, true)));
        assert!(background.covering && background.main_menu);
        assert!(!overlay.covering && !overlay.main_menu);
        background.observe(false, 0, 0, 0x43, None);
        assert!(!background.covering && !background.main_menu);
    }

    #[test]
    fn composited_screens_take_priority_over_artwork_and_fades() {
        let artwork = ArtworkState { covering: true, main_menu: true };
        assert_eq!(artwork.layout(true, true, 0x48), Layout::Original);
        assert_eq!(artwork.layout(false, true, 0x48), Layout::CoverArtwork);
        let overlay = ArtworkState { covering: false, main_menu: true };
        assert_eq!(overlay.layout(false, false, 0x500fef48), Layout::Original);
        assert_eq!(overlay.layout(false, false, 0x40), Layout::CenterUi);
        assert_eq!(ArtworkState::default().layout(false, true, 0x40), Layout::Original);
        assert_eq!(ArtworkState::default().layout(false, false, 0x40), Layout::CenterUi);
    }

    #[test]
    fn full_screen_artwork_covers_and_ui_stays_centered() {
        let transform = DisplayTransform { size: (1920, 1080), scale: 1.25 };
        let scissor = [0, 0, 1920, 1080];
        assert_eq!(transform.apply(Layout::CoverArtwork, None, scissor), (Some([0.0, -135.0, 1920.0, 1350.0]), scissor));
        assert_eq!(transform.apply(Layout::CenterUi, None, scissor), (Some([192.0, 0.0, 1536.0, 1080.0]), [192, 0, 1536, 1080]));
    }

    #[test]
    fn partial_viewports_and_scissors_transform_about_target_center() {
        let transform = DisplayTransform { size: (1920, 1080), scale: 1.25 };
        let viewport = Some([100.0, 50.0, 1720.0, 980.0]);
        let scissor = [240, 120, 1440, 840];
        assert_eq!(transform.apply(Layout::CoverArtwork, viewport, scissor), (Some([100.0, -72.5, 1720.0, 1225.0]), [240, 15, 1440, 1050]));
        assert_eq!(transform.apply(Layout::CenterUi, viewport, scissor), (Some([272.0, 50.0, 1376.0, 980.0]), [384, 120, 1152, 840]));
    }

    #[test]
    fn original_layout_preserves_implicit_and_explicit_viewports() {
        let transform = DisplayTransform { size: (1920, 1080), scale: 1.25 };
        let scissor = [240, 120, 1440, 840];
        for viewport in [None, Some([100.0, 50.0, 1720.0, 980.0])] {
            assert_eq!(transform.apply(Layout::Original, viewport, scissor), (viewport, scissor));
        }
    }

    #[test]
    fn half_resolution_menu_art_keeps_the_same_overlay_space() {
        for texture in [(1920, 1080, true), (960, 540, true)] {
            assert!(is_menu_illustration(texture));
            assert!(starts_menu_art(0x77264c28fdc0921b, 0x40, Some(texture)));
        }
        for texture in [(960, 540, false), (1280, 720, false), (170, 105, true), (900, 1024, false)] {
            assert!(!is_menu_illustration(texture));
        }
        assert!(!starts_menu_art(0x77264c28fdc0921b, 0x48, Some((960, 540, true))));
    }

    #[test]
    fn backdrop_masks_use_screen_space_without_a_main_menu_seed() {
        for depth in [0x40, 0x48, 0x500fef48] {
            for mask in [(80, 45), (80, 39), (4, 45), (8, 8), (16, 16)] {
                assert!(is_menu_backdrop(depth, mask, (320, 180, 90)));
            }
        }
        // Ordinary controls, previews and depth-tested geometry retain their
        // existing placement even when they share a menu shader or texture size.
        assert!(!is_menu_backdrop(0x48, (170, 105), (320, 180, 90)));
        assert!(!is_menu_backdrop(0x48, (8, 8), (320, 180, 71)));
        assert!(!is_menu_backdrop(0x48, (8, 8), (1280, 720, 90)));
        assert!(!is_menu_backdrop(0x44, (8, 8), (320, 180, 90)));
    }

    #[test]
    fn opaque_transition_frame_copy_keeps_full_viewport() {
        let vs = 0x77264c28fdc0921b;
        let fs = 0x5bb31027ff12d02e;
        assert!(is_frame_copy(vs, fs, 0x3af2b940, 0x8030000, (1280, 720, 90)));
        assert!(!is_frame_copy(vs, fs, 0x40, 0x8030100, (1280, 720, 90)));
        assert!(!is_frame_copy(vs, fs, 0x48, 0x8030000, (1280, 720, 90)));
        assert!(!is_frame_copy(vs, fs, 0x40, 0x8030000, (1280, 720, 71)));
        assert!(!is_frame_copy(vs, fs, 0x40, 0x8030000, (170, 105, 90)));
    }

    #[test]
    fn track_gradient_starts_cover_but_track_previews_do_not() {
        let hash = 0x8a167a1501240ee5;
        for state in [0x40, 0x48, 0x3f800040] {
            assert!(starts_menu_art(hash, state, Some((8, 1048, true))));
        }
        for texture in [(170, 105, true), (400, 400, true), (1920, 1080, true), (1280, 720, false)] {
            assert!(!starts_menu_art(hash, 0x40, Some(texture)));
        }
        assert!(!starts_menu_art(hash, 0x44, Some((8, 1048, true))));
        let scale = 1.222159;
        let y0 = cover_y(0.0, 1080.0, scale);
        let y1 = cover_y(1080.0, 1080.0, scale);
        assert!(y0 < 0.0 && y1 > 1080.0);
        assert!(((y1 - y0) / 1080.0 - scale).abs() < 0.00001);
    }

    #[test]
    fn intro_framebuffer_is_not_scaled_as_raw_artwork() {
        assert!(is_intro_composite(0x305d4f5c810d7403, 0x16182a4ea0da75ec));
        assert!(!is_intro_composite(0xff58c483a97cf8ef, 0xcc03897b25330f90));
        assert!(!is_intro_composite(0x305d4f5c810d7403, 0x1b73271e7a83233c));
    }

    #[test]
    fn opening_animation_is_artwork_not_a_menu_preview() {
        for state in [0x40, 0x500fef48] {
            for height in [720, 420] {
                assert!(starts_intro_art(0xff58c483a97cf8ef, 0xcc03897b25330f90, state, Some((1280, height, false))));
            }
        }
        assert!(!starts_intro_art(0xff58c483a97cf8ef, 0x1b73271e7a83233c, 0x40, Some((1280, 720, false))));
        assert!(!starts_intro_art(0xff58c483a97cf8ef, 0xcc03897b25330f90, 0x44, Some((1280, 720, false))));
        assert!(!starts_intro_art(0x77264c28fdc0921b, 0x5bb31027ff12d02e, 0x40, Some((1280, 720, false))));
    }

    #[test]
    fn mode_transition_background_and_fade_keep_full_coverage() {
        assert!(starts_menu_art(0x77264c28fdc0921b, 0x48, Some((8, 1048, true))));
        for hash in [0x77264c28fdc0921b, 0x8a167a1501240ee5] {
            assert!(is_menu_fade(hash, 0xac461340, Some((8, 8, true))));
            assert!(!is_menu_fade(hash, 0x500fef48, Some((8, 8, true))));
            assert!(!is_menu_fade(hash, 0xac461340, Some((72, 72, true))));
        }
    }

    #[test]
    fn title_background_does_not_require_gradient_draw() {
        assert!(starts_menu_art(0x77264c28fdc0921b, 0x40, Some((1920, 1080, true))));
        assert!(starts_menu_art(0x77264c28fdc0921b, 0x40, Some((8, 1048, false))));
        // A resolve, preview panel, or ordinary icon must not start a cover pass.
        assert!(!starts_menu_art(0xf8211eae2605bb61, 0x40, Some((1920, 1080, true))));
        assert!(!starts_menu_art(0x77264c28fdc0921b, 0x40, Some((1920, 1080, false))));
        assert!(!starts_menu_art(0x77264c28fdc0921b, 0x500fef48, Some((1920, 1080, true))));
        assert!(!starts_menu_art(0x77264c28fdc0921b, 0x40, Some((170, 105, true))));
    }

    #[test]
    fn transition_padding_does_not_change_background_classification() {
        for state in [0x40, 0x3f800040, 0x500fef40] {
            assert!(starts_menu_art(0x77264c28fdc0921b, state, Some((1920, 1080, true))));
            assert!(starts_menu_art(0x77264c28fdc0921b, state, Some((8, 1048, false))));
            assert_eq!(menu_depth_state(state), 0x40);
        }
        for state in [0x41, 0x42, 0x44, 0x500fef48] {
            assert!(!starts_menu_art(0x77264c28fdc0921b, state, Some((1920, 1080, true))));
        }
    }
}
