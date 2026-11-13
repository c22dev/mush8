// Mush8 - overlay.rs
// Constantin Clerc (c22dev) - 0.1
use crate::input::Kind;
use objc2::MainThreadMarker;
use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
pub use ui::{game_window, set_game_window};

fn on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(move || f(MainThreadMarker::new().expect("main queue runs on the main thread")));
}

const CARD_W: f64 = 120.0;
const CARD_H: f64 = 150.0;
const GAP: f64 = 14.0;
const PAD: f64 = 24.0;
const TOP: f64 = 84.0;
const BOTTOM: f64 = 52.0;
const MIN_WIDTH: f64 = 520.0;
const ICON: f64 = 40.0;
const PLAYERS: usize = 4;

const COLORS: [(f64, f64, f64); PLAYERS] = [(0.92, 0.25, 0.24), (0.20, 0.50, 0.96), (0.90, 0.66, 0.08), (0.20, 0.72, 0.34)];

static TAPS: Mutex<Vec<(usize, bool)>> = Mutex::new(Vec::new());

fn tap((i, badge): (usize, bool)) {
    TAPS.lock().unwrap().push((i, badge));
}

#[derive(Clone, PartialEq)]
struct Card {
    kind: Option<Kind>,
    name: String,
    selected: bool,
    adds: bool,
    removes: bool,
}

const ADD: &str = if cfg!(target_os = "ios") { "Add touch controls" } else { "Add keyboard" };

fn symbol(c: &Card) -> &'static str {
    if c.adds {
        return if cfg!(target_os = "ios") { "hand.tap.fill" } else { "keyboard" };
    }
    match c.kind {
        Some(Kind::Controller) => "gamecontroller.fill",
        Some(Kind::Keyboard) => "keyboard",
        Some(Kind::Touch) => "hand.tap.fill",
        Some(Kind::Iphone) => "iphone",
        Some(Kind::Browser) => "globe",
        Some(Kind::Virtual) => "gamecontroller",
        None => "plus.circle.dashed",
    }
}

fn dashed(color: &objc2_core_graphics::CGColor) -> objc2::rc::Retained<objc2_quartz_core::CAShapeLayer> {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSArray, NSNumber};
    let l = objc2_quartz_core::CAShapeLayer::new();
    let rect = CGRect::new(CGPoint::new(1.0, 1.0), CGSize::new(CARD_W - 2.0, CARD_H - 2.0));
    let path = unsafe { objc2_core_graphics::CGPath::with_rounded_rect(rect, 15.0, 15.0, std::ptr::null()) };
    l.setPath(Some(&path));
    l.setFillColor(None);
    l.setStrokeColor(Some(color));
    l.setLineWidth(1.5);
    l.setLineDashPattern(Some(&NSArray::from_retained_slice(&[NSNumber::new_f64(6.0), NSNumber::new_f64(5.0)])));
    l.setHidden(true);
    l
}

const BADGE: f64 = 20.0;
const BADGE_X: f64 = CARD_W / 2.0 - 36.0;
const BADGE_Y: f64 = 46.0;
const BADGE_REACH: f64 = 10.0;

fn badge_symbol(c: &Card) -> &'static str {
    if c.removes { "xmark.circle.fill" } else { "plus.circle.fill" }
}

fn adds_alpha(on: bool, adds: bool, (lit, add, off): (f64, f64, f64)) -> f64 {
    if on { lit } else if adds { add } else { off }
}

#[derive(Clone, Copy)]
struct Layout {
    count: usize,
    width: f64,
    height: f64,
}

impl Layout {
    fn new(count: usize) -> Self {
        let row = count as f64 * CARD_W + (count - 1) as f64 * GAP;
        Layout { count, width: (row + 2.0 * PAD).max(MIN_WIDTH), height: TOP + CARD_H + BOTTOM }
    }

    fn card(&self, i: usize) -> (f64, f64) {
        let row = self.count as f64 * CARD_W + (self.count - 1) as f64 * GAP;
        ((self.width - row) / 2.0 + i as f64 * (CARD_W + GAP), TOP)
    }

    fn hit(&self, x: f64, y: f64) -> Option<(usize, bool)> {
        let i = (0..self.count).find(|&i| {
            let (cx, cy) = self.card(i);
            x >= cx && x < cx + CARD_W && y >= cy && y < cy + CARD_H
        })?;
        let (cx, cy) = self.card(i);
        let (bx, by) = (x - cx - BADGE_X, y - cy - BADGE_Y);
        let reach = -BADGE_REACH..BADGE + BADGE_REACH;
        Some((i, reach.contains(&bx) && reach.contains(&by)))
    }
}

#[cfg(target_os = "macos")]
mod ui {
    use super::{BADGE, BADGE_X, BADGE_Y, CARD_H, CARD_W, COLORS, Card, ICON, Layout, adds_alpha};
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_quartz_core::CAShapeLayer;
    use objc2_app_kit::{
        NSBackingStoreType, NSBitmapImageFileType, NSBox, NSBoxType, NSColor, NSEvent, NSFont, NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration, NSImageView,
        NSLineBreakMode, NSResponder, NSTextAlignment, NSTextField, NSTitlePosition, NSView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    static WINDOW: AtomicUsize = AtomicUsize::new(0);

    pub fn set_game_window(w: &NSWindow) {
        WINDOW.store(w as *const NSWindow as usize, Relaxed);
    }

    pub fn available() -> bool {
        WINDOW.load(Relaxed) != 0
    }

    pub fn game_window() -> usize {
        WINDOW.load(Relaxed)
    }

    struct CardView {
        bg: Retained<NSBox>,
        pill: Retained<NSBox>,
        icon: Retained<NSImageView>,
        badge: Retained<NSImageView>,
        dash: Retained<CAShapeLayer>,
        name: Retained<NSTextField>,
    }

    struct Panel {
        window: Retained<NSWindow>,
        layout: Layout,
        cards: Vec<CardView>,
        hint: Retained<NSTextField>,
        footer: Retained<NSTextField>,
    }

    thread_local! {
        static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
    }

    define_class!(
        #[unsafe(super(NSView, NSResponder, NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "NXControllerClicks"]
        struct Clicks;

        impl Clicks {
            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                let p = self.convertPoint_fromView(event.locationInWindow(), None);
                let h = self.bounds().size.height;
                let hit = PANEL.with(|panel| panel.borrow().as_ref().and_then(|panel| panel.layout.hit(p.x, h - p.y)));
                if let Some(hit) = hit {
                    super::tap(hit);
                }
            }
        }
    );

    fn rgb((r, g, b): (f64, f64, f64)) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0)
    }

    fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool, frame: NSRect) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        let font = if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
        l.setFont(Some(&font));
        l.setTextColor(Some(&NSColor::whiteColor()));
        l.setAlignment(NSTextAlignment::Center);
        l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        l.setFrame(frame);
        l
    }

    fn rounded(mtm: MainThreadMarker, frame: NSRect, radius: f64) -> Retained<NSBox> {
        let b = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        b.setBoxType(NSBoxType::Custom);
        b.setTitlePosition(NSTitlePosition::NoTitle);
        b.setContentViewMargins(NSSize::new(0.0, 0.0));
        b.setCornerRadius(radius);
        b
    }

    fn image(name: &str, size: f64) -> Option<Retained<NSImage>> {
        let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
        img.imageWithSymbolConfiguration(&NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe { NSFontWeightSemibold }))
    }

    pub fn open(mtm: MainThreadMarker, count: usize) {
        let w = WINDOW.load(Relaxed);
        if w == 0 {
            return;
        }
        let game: &NSWindow = unsafe { &*(w as *const NSWindow) };
        let layout = Layout::new(count);
        let (width, height) = (layout.width, layout.height);
        let content = game.contentView().map_or(game.frame(), |v| game.convertRectToScreen(v.frame()));
        let origin = NSPoint::new(content.origin.x + (content.size.width - width) / 2.0, content.origin.y + (content.size.height - height) / 2.0);
        let rect = NSRect::new(origin, NSSize::new(width, height));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, NSWindowStyleMask::Borderless, NSBackingStoreType::Buffered, false)
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setIgnoresMouseEvents(false);
        window.setHasShadow(true);
        let at = |x: f64, y: f64, w: f64, h: f64, parent: f64| NSRect::new(NSPoint::new(x, parent - y - h), NSSize::new(w, h));
        let panel = rounded(mtm, at(0.0, 0.0, width, height, height), 20.0);
        panel.setFillColor(&NSColor::colorWithWhite_alpha(0.07, 0.92));
        panel.setBorderColor(&NSColor::colorWithWhite_alpha(1.0, 0.18));
        panel.setWantsLayer(true);
        window.setContentView(Some(&panel));
        panel.addSubview(&label(mtm, "Controllers", 24.0, true, at(20.0, 16.0, width - 40.0, 32.0, height)));
        let hint = label(mtm, "", 13.0, false, at(20.0, 52.0, width - 40.0, 20.0, height));
        hint.setTextColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.6)));
        panel.addSubview(&hint);
        let cards = (0..count)
            .map(|i| {
                let (x, y) = layout.card(i);
                let bg = rounded(mtm, at(x, y, CARD_W, CARD_H, height), 16.0);
                let pill = rounded(mtm, at((CARD_W - 46.0) / 2.0, 12.0, 46.0, 22.0, CARD_H), 11.0);
                pill.setBorderWidth(0.0);
                pill.addSubview(&label(mtm, &format!("P{}", i + 1), 13.0, true, NSRect::new(NSPoint::new(0.0, 2.0), NSSize::new(46.0, 18.0))));
                let icon = NSImageView::new(mtm);
                icon.setFrame(at(0.0, 44.0, CARD_W, 64.0, CARD_H));
                let name = label(mtm, "", 12.0, false, at(8.0, 116.0, CARD_W - 16.0, 18.0, CARD_H));
                let badge = NSImageView::new(mtm);
                badge.setFrame(at(BADGE_X, BADGE_Y, BADGE, BADGE, CARD_H));
                badge.setImage(image("plus.circle.fill", 15.0).as_deref());
                badge.setContentTintColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.55)));
                badge.setHidden(true);
                bg.addSubview(&pill);
                bg.addSubview(&icon);
                bg.addSubview(&badge);
                bg.addSubview(&name);
                bg.setWantsLayer(true);
                let dash = super::dashed(&NSColor::colorWithWhite_alpha(1.0, 0.3).CGColor());
                if let Some(layer) = bg.layer() {
                    layer.addSublayer(&dash);
                }
                panel.addSubview(&bg);
                CardView { bg, pill, icon, badge, dash, name }
            })
            .collect();
        let footer = label(mtm, "", 14.0, false, at(20.0, height - 36.0, width - 40.0, 20.0, height));
        panel.addSubview(&footer);
        let clicks: Retained<Clicks> = unsafe { msg_send![Clicks::alloc(mtm), initWithFrame: panel.bounds()] };
        panel.addSubview(&clicks);
        unsafe { game.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
        PANEL.with(|p| *p.borrow_mut() = Some(Panel { window, layout, cards, hint, footer }));
    }

    pub fn update(cards: Vec<Card>, hint: String, footer: String) {
        PANEL.with(|p| {
            let Some(p) = &*p.borrow() else { return };
            for (i, (v, c)) in p.cards.iter().zip(&cards).enumerate() {
                let color = rgb(COLORS[i]);
                let on = c.kind.is_some();
                let a = |levels| adds_alpha(on, c.adds, levels);
                v.bg.setFillColor(&NSColor::colorWithWhite_alpha(1.0, a((0.12, 0.03, 0.05))));
                let border = if c.selected { NSColor::whiteColor() } else if on { color.clone() } else { NSColor::colorWithWhite_alpha(1.0, 0.12) };
                v.bg.setBorderColor(&border);
                v.bg.setBorderWidth(if c.adds { 0.0 } else if c.selected { 3.0 } else { 1.5 });
                v.dash.setHidden(!c.adds);
                v.badge.setHidden(!(c.adds || c.removes));
                v.badge.setImage(image(super::badge_symbol(c), 15.0).as_deref());
                v.badge.setContentTintColor(Some(&NSColor::colorWithWhite_alpha(1.0, if c.removes { 0.75 } else { 0.55 })));
                let pill = if on { color } else { NSColor::colorWithWhite_alpha(1.0, a((1.0, 0.12, 0.2))) };
                v.pill.setFillColor(&pill);
                v.icon.setImage(image(super::symbol(c), ICON).as_deref());
                v.icon.setContentTintColor(Some(&NSColor::colorWithWhite_alpha(1.0, a((1.0, 0.4, 0.3)))));
                v.name.setStringValue(&NSString::from_str(&c.name));
                v.name.setTextColor(Some(&NSColor::colorWithWhite_alpha(1.0, a((0.9, 0.5, 0.4)))));
            }
            p.hint.setStringValue(&NSString::from_str(&hint));
            p.footer.setStringValue(&NSString::from_str(&footer));
        });
    }

    pub fn wiggle(angles: Vec<f64>) {
        PANEL.with(|p| {
            let Some(p) = &*p.borrow() else { return };
            for (v, a) in p.cards.iter().zip(angles) {
                v.icon.setFrameCenterRotation(a.to_degrees());
            }
        });
    }

    pub fn save_png(path: &str) {
        PANEL.with(|p| {
            let Some(p) = &*p.borrow() else { return };
            let Some(view) = p.window.contentView() else { return };
            let rect = view.bounds();
            let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(rect) else { return };
            view.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
            if let Some(data) = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) } {
                let _ = std::fs::write(path, data.to_vec());
                eprintln!("[overlay] controller screen saved to {path}");
            }
        });
    }

    pub fn close() {
        PANEL.with(|p| {
            if let Some(p) = p.borrow_mut().take() {
                let game = WINDOW.load(Relaxed);
                if game != 0 {
                    unsafe { &*(game as *const NSWindow) }.removeChildWindow(&p.window);
                }
                p.window.orderOut(None);
            }
        });
    }
}

#[cfg(target_os = "ios")]
mod ui {
    use super::{BADGE, BADGE_X, BADGE_Y, CARD_H, CARD_W, COLORS, Card, ICON, Layout, adds_alpha};
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
    use objc2_quartz_core::CAShapeLayer;
    use objc2_core_foundation::{CGAffineTransform, CGPoint, CGRect, CGSize};
    use objc2_foundation::{NSSet, NSString};
    use objc2_ui_kit::{
        NSTextAlignment, UIColor, UIEvent, UIFont, UIImage, UIImageSymbolConfiguration, UIImageSymbolWeight, UIImageView, UILabel, UIResponder, UITouch, UIView,
        UIViewAutoresizing, UIViewContentMode,
    };
    use std::cell::RefCell;

    pub fn available() -> bool {
        true
    }

    struct CardView {
        bg: Retained<UIView>,
        pill: Retained<UIView>,
        icon: Retained<UIImageView>,
        badge: Retained<UIImageView>,
        dash: Retained<CAShapeLayer>,
        name: Retained<UILabel>,
    }

    struct Panel {
        view: Retained<PanelView>,
        cards: Vec<CardView>,
        hint: Retained<UILabel>,
        footer: Retained<UILabel>,
    }

    thread_local! {
        static PANEL: RefCell<Vec<Panel>> = const { RefCell::new(Vec::new()) };
        static LAYOUT: std::cell::Cell<Option<Layout>> = const { std::cell::Cell::new(None) };
    }

    fn card_at(p: CGPoint) -> Option<(usize, bool)> {
        LAYOUT.with(|l| l.get()).and_then(|l| l.hit(p.x, p.y))
    }

    define_class!(
        #[unsafe(super(UIView, UIResponder, NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "NXControllerPanel"]
        struct PanelView;

        impl PanelView {
            #[unsafe(method_id(hitTest:withEvent:))]
            fn hit_test(&self, point: CGPoint, _event: Option<&UIEvent>) -> Option<Retained<UIView>> {
                card_at(point).map(|_| Retained::into_super(self.retain()))
            }

            #[unsafe(method(touchesBegan:withEvent:))]
            fn touches_began(&self, _touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {}

            #[unsafe(method(touchesMoved:withEvent:))]
            fn touches_moved(&self, _touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {}

            #[unsafe(method(touchesCancelled:withEvent:))]
            fn touches_cancelled(&self, _touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {}

            #[unsafe(method(touchesEnded:withEvent:))]
            fn touches_ended(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
                for t in touches.iter() {
                    if let Some(hit) = card_at(t.locationInView(Some(self))) {
                        super::tap(hit);
                    }
                }
            }
        }
    );

    fn rgb((r, g, b): (f64, f64, f64)) -> Retained<UIColor> {
        UIColor::colorWithRed_green_blue_alpha(r, g, b, 1.0)
    }

    fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool, frame: CGRect) -> Retained<UILabel> {
        let l = UILabel::new(mtm);
        l.setText(Some(&NSString::from_str(text)));
        let font = if bold { UIFont::boldSystemFontOfSize(size) } else { UIFont::systemFontOfSize(size) };
        unsafe { l.setFont(Some(&font)) };
        unsafe { l.setTextColor(Some(&UIColor::whiteColor())) };
        l.setTextAlignment(NSTextAlignment::Center);
        l.setAdjustsFontSizeToFitWidth(true);
        l.setMinimumScaleFactor(0.7);
        l.setFrame(frame);
        l
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
    }

    fn rounded(mtm: MainThreadMarker, frame: CGRect, radius: f64) -> Retained<UIView> {
        let v = UIView::initWithFrame(UIView::alloc(mtm), frame);
        v.layer().setCornerRadius(radius);
        v.setUserInteractionEnabled(false);
        v
    }

    fn image(name: &str, size: f64) -> Option<Retained<UIImage>> {
        let config = UIImageSymbolConfiguration::configurationWithPointSize_weight(size, UIImageSymbolWeight::Semibold);
        UIImage::systemImageNamed_withConfiguration(&NSString::from_str(name), Some(&config))
    }

    const TV_SHARE: f64 = 0.5;

    fn sharpen(view: &UIView, scale: f64) {
        view.setContentScaleFactor(scale);
        if let Some(sublayers) = unsafe { view.layer().sublayers() } {
            for l in sublayers.iter().filter(|l| l.downcast_ref::<CAShapeLayer>().is_some()) {
                l.setContentsScale(scale);
            }
        }
        for v in view.subviews().iter() {
            sharpen(&v, scale);
        }
    }

    fn rotation(a: f64) -> CGAffineTransform {
        CGAffineTransform { a: a.cos(), b: a.sin(), c: -a.sin(), d: a.cos(), tx: 0.0, ty: 0.0 }
    }

    pub fn open(mtm: MainThreadMarker, count: usize) {
        let layout = Layout::new(count);
        for (root, tv) in super::super::ios::panel_hosts() {
            let bounds = root.bounds();
            let (width, height) = (layout.width, layout.height);
            let scale = if tv {
                (bounds.size.width * TV_SHARE / width).min(bounds.size.height * TV_SHARE / height)
            } else {
                1f64.min((bounds.size.width - 32.0) / width).min((bounds.size.height - 32.0) / height)
            };
            let view: Retained<PanelView> =
                unsafe { msg_send![PanelView::alloc(mtm), initWithFrame: rect((bounds.size.width - width) / 2.0, (bounds.size.height - height) / 2.0, width, height)] };
            view.setTransform(CGAffineTransform { a: scale, b: 0.0, c: 0.0, d: scale, tx: 0.0, ty: 0.0 });
            view.setBackgroundColor(Some(&UIColor::colorWithWhite_alpha(0.07, 0.92)));
            view.setAutoresizingMask(UIViewAutoresizing::FlexibleLeftMargin | UIViewAutoresizing::FlexibleRightMargin | UIViewAutoresizing::FlexibleTopMargin | UIViewAutoresizing::FlexibleBottomMargin);
            let layer = view.layer();
            layer.setCornerRadius(20.0);
            layer.setBorderWidth(1.0);
            layer.setBorderColor(Some(&*unsafe { UIColor::colorWithWhite_alpha(1.0, 0.18).CGColor() }));
            view.addSubview(&label(mtm, "Controllers", 24.0, true, rect(20.0, 16.0, width - 40.0, 32.0)));
            let hint = label(mtm, "", 13.0, false, rect(20.0, 52.0, width - 40.0, 20.0));
            unsafe { hint.setTextColor(Some(&UIColor::colorWithWhite_alpha(1.0, 0.6))) };
            view.addSubview(&hint);
            let cards = (0..count)
                .map(|i| {
                    let (x, y) = layout.card(i);
                    let bg = rounded(mtm, rect(x, y, CARD_W, CARD_H), 16.0);
                    let pill = rounded(mtm, rect((CARD_W - 46.0) / 2.0, 12.0, 46.0, 22.0), 11.0);
                    pill.addSubview(&label(mtm, &format!("P{}", i + 1), 13.0, true, rect(0.0, 0.0, 46.0, 22.0)));
                    let icon = UIImageView::initWithImage(UIImageView::alloc(mtm), None);
                    icon.setFrame(rect(0.0, 44.0, CARD_W, 64.0));
                    icon.setContentMode(UIViewContentMode::Center);
                    let name = label(mtm, "", 12.0, false, rect(8.0, 116.0, CARD_W - 16.0, 18.0));
                    let badge = UIImageView::initWithImage(UIImageView::alloc(mtm), image("plus.circle.fill", 15.0).as_deref());
                    badge.setFrame(rect(BADGE_X, BADGE_Y, BADGE, BADGE));
                    badge.setContentMode(UIViewContentMode::Center);
                    unsafe { badge.setTintColor(Some(&UIColor::colorWithWhite_alpha(1.0, 0.55))) };
                    badge.setHidden(true);
                    bg.addSubview(&pill);
                    bg.addSubview(&icon);
                    bg.addSubview(&badge);
                    bg.addSubview(&name);
                    let dash = super::dashed(&*unsafe { UIColor::colorWithWhite_alpha(1.0, 0.3).CGColor() });
                    bg.layer().addSublayer(&dash);
                    view.addSubview(&bg);
                    CardView { bg, pill, icon, badge, dash, name }
                })
                .collect();
            let footer = label(mtm, "", 14.0, false, rect(20.0, height - 36.0, width - 40.0, 20.0));
            view.addSubview(&footer);
            sharpen(&view, scale * root.contentScaleFactor());
            root.addSubview(&view);
            PANEL.with(|p| p.borrow_mut().push(Panel { view, cards, hint, footer }));
        }
        LAYOUT.with(|l| l.set(Some(layout)));
    }

    pub fn update(cards: Vec<Card>, hint: String, footer: String) {
        PANEL.with(|panels| {
            for p in panels.borrow().iter() {
            for (i, (v, c)) in p.cards.iter().zip(&cards).enumerate() {
                let color = rgb(COLORS[i]);
                let on = c.kind.is_some();
                let a = |levels| adds_alpha(on, c.adds, levels);
                v.bg.setBackgroundColor(Some(&UIColor::colorWithWhite_alpha(1.0, a((0.12, 0.03, 0.05)))));
                let layer = v.bg.layer();
                let border = if c.selected { UIColor::whiteColor() } else if on { color.clone() } else { UIColor::colorWithWhite_alpha(1.0, 0.12) };
                layer.setBorderColor(Some(&*unsafe { border.CGColor() }));
                layer.setBorderWidth(if c.adds { 0.0 } else if c.selected { 3.0 } else { 1.5 });
                v.dash.setHidden(!c.adds);
                v.badge.setHidden(!(c.adds || c.removes));
                v.badge.setImage(image(super::badge_symbol(c), 15.0).as_deref());
                unsafe { v.badge.setTintColor(Some(&UIColor::colorWithWhite_alpha(1.0, if c.removes { 0.75 } else { 0.55 }))) };
                let pill = if on { color } else { UIColor::colorWithWhite_alpha(1.0, a((1.0, 0.12, 0.2))) };
                v.pill.setBackgroundColor(Some(&pill));
                v.icon.setImage(image(super::symbol(c), ICON).as_deref());
                unsafe { v.icon.setTintColor(Some(&UIColor::colorWithWhite_alpha(1.0, a((1.0, 0.4, 0.3))))) };
                v.name.setText(Some(&NSString::from_str(&c.name)));
                unsafe { v.name.setTextColor(Some(&UIColor::colorWithWhite_alpha(1.0, a((0.9, 0.5, 0.4))))) };
            }
            p.hint.setText(Some(&NSString::from_str(&hint)));
            p.footer.setText(Some(&NSString::from_str(&footer)));
            }
        });
    }

    pub fn wiggle(angles: Vec<f64>) {
        PANEL.with(|panels| {
            for p in panels.borrow().iter() {
                for (v, &a) in p.cards.iter().zip(&angles) {
                    v.icon.setTransform(rotation(a));
                }
            }
        });
    }

    pub fn save_png(_path: &str) {}

    pub fn close() {
        LAYOUT.with(|l| l.set(None));
        PANEL.with(|panels| {
            for p in panels.borrow_mut().drain(..) {
                p.view.removeFromSuperview();
            }
        });
    }
}

const CONFIRM: u64 = 1 | 1 << 10;
const CANCEL: u64 = 1 << 1;
const MOVE_LEFT: u64 = crate::input::L | crate::input::ZL;
const MOVE_RIGHT: u64 = crate::input::R | crate::input::ZR;
const WIGGLE: Duration = Duration::from_millis(450);
const SETTLE: Duration = Duration::from_millis(250);

fn movable(k: Option<Kind>) -> bool {
    matches!(k, Some(Kind::Controller | Kind::Keyboard | Kind::Touch | Kind::Iphone | Kind::Browser))
}

fn swappable(kinds: &[Option<Kind>], a: usize, b: usize) -> bool {
    if a == b || !(movable(kinds[a]) || movable(kinds[b])) {
        return false;
    }
    let mut after: Vec<_> = kinds.iter().map(|&k| k.filter(|_| movable(k))).collect();
    after.swap(a, b);
    let native = after.iter().position(|k| matches!(k, Some(Kind::Touch | Kind::Keyboard)));
    let pad = after.iter().rposition(|k| *k == Some(Kind::Controller));
    if native.zip(pad).is_some_and(|(n, p)| n < p) {
        return false;
    }
    after.iter().skip(1).skip_while(|k| k.is_some()).all(|k| k.is_none())
}

fn angle(since: Instant) -> f64 {
    let t = since.elapsed().as_secs_f64() / WIGGLE.as_secs_f64();
    if t >= 1.0 { 0.0 } else { 0.2 * (t * std::f64::consts::TAU * 3.0).sin() * (1.0 - t) }
}

pub fn controller_screen(min: usize, max: usize) -> Option<usize> {
    let connected = || crate::input::SLOT_NAMES.lock().unwrap().iter().filter(|n| n.is_some()).count();
    if !ui::available() || std::env::var_os("NX_AUTOINPUT").is_some() {
        return Some(connected());
    }
    let max = max.clamp(1, PLAYERS);
    TAPS.lock().unwrap().clear();
    on_main(move |mtm| ui::open(mtm, max));
    let held = || crate::services::hid::BUTTONS.iter().map(|b| b.load(Relaxed)).collect::<Vec<_>>();
    let mut prev = held();
    let mut saved = false;
    let mut selected: Option<usize> = None;
    let mut wiggles: [Option<Instant>; PLAYERS] = [None; PLAYERS];
    let mut moved = Instant::now() - SETTLE;
    let mut shown = None;
    let mut tilted = false;
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let names = crate::input::SLOT_NAMES.lock().unwrap().clone();
        let kinds: Vec<_> = names.iter().map(|n| n.as_ref().map(|(k, _)| *k)).collect();
        let n = connected();
        let now = held();
        let pressed = |i: usize, mask: u64| now[i] & mask != 0 && prev[i] & mask == 0;
        let native = kinds.iter().any(|k| matches!(k, Some(Kind::Touch | Kind::Keyboard)));
        let add = crate::input::native_slot().filter(|&j| j < max && kinds[j].is_none() && !native && n < min);
        let extra = crate::input::extra_native();
        let removes = |i: usize| extra && matches!(kinds[i], Some(Kind::Touch | Kind::Keyboard));
        let mut swaps = Vec::new();
        for i in 0..max {
            if now[i] & !prev[i] != 0 {
                wiggles[i] = Some(Instant::now());
            }
            let filled = (1..max).find(|&j| kinds[j].is_none()).unwrap_or(max);
            if i < filled && filled > 1 && moved.elapsed() >= SETTLE {
                if pressed(i, MOVE_LEFT) {
                    swaps.push((i, (i + filled - 1) % filled));
                } else if pressed(i, MOVE_RIGHT) {
                    swaps.push((i, (i + 1) % filled));
                }
            }
        }
        for (i, badge) in std::mem::take(&mut *TAPS.lock().unwrap()) {
            wiggles[i] = Some(Instant::now());
            if badge && removes(i) {
                crate::input::remove_native();
                selected = None;
                continue;
            }
            if Some(i) == add {
                crate::input::add_native();
                selected = None;
                continue;
            }
            match selected.take() {
                Some(s) if s != i => swaps.push((s, i)),
                Some(_) => {}
                None => selected = Some(i),
            }
        }
        if let Some(&(a, b)) = swaps.iter().find(|&&(a, b)| swappable(&kinds, a, b)) {
            crate::input::swap(a, b);
            moved = Instant::now();
            wiggles[a] = Some(moved);
            wiggles[b] = Some(moved);
            selected = None;
        }
        let cards: Vec<Card> = (0..max)
            .map(|i| Card {
                kind: kinds[i],
                name: names[i].as_ref().map_or(if add == Some(i) { ADD } else { "Empty" }.into(), |(_, name)| name.clone()),
                selected: selected == Some(i),
                adds: add == Some(i),
                removes: removes(i),
            })
            .collect();
        let keyboard = kinds[0] == Some(Kind::Keyboard);
        let (go, back, shift) = if keyboard { ("Return", "Backspace", "Q / E") } else { ("A or +", "B", "L / R") };
        let touch = if cfg!(target_os = "ios") { "tap" } else { "click" };
        let hint = format!("{shift} to move · {touch} two players to swap them");
        let footer = if n < min {
            format!("{n} of {min} players connected · {go} to continue anyway · {back} to go back")
        } else {
            format!("{go} to continue · {back} to go back")
        };
        let state = (cards, hint, footer);
        if shown.as_ref() != Some(&state) {
            shown = Some(state.clone());
            on_main(move |_| ui::update(state.0, state.1, state.2));
        }
        let angles: Vec<f64> = (0..max).map(|i| wiggles[i].map_or(0.0, angle)).collect();
        let tilting = angles.iter().any(|&a| a != 0.0);
        if std::mem::replace(&mut tilted, tilting) || tilting {
            on_main(move |_| ui::wiggle(angles));
        }
        if let Some(path) = std::env::var("NX_OVERLAY_PNG").ok().filter(|_| !std::mem::replace(&mut saved, true)) {
            on_main(move |_| ui::save_png(&path));
        }
        let (confirm, cancel) = ((0..now.len()).any(|i| pressed(i, CONFIRM)), (0..now.len()).any(|i| pressed(i, CANCEL)));
        prev = now;
        if confirm || cancel {
            on_main(|_| ui::close());
            return confirm.then_some(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamepads_stay_ahead_of_keyboard_and_touch() {
        let (pad, key, touch, phone) = (Some(Kind::Controller), Some(Kind::Keyboard), Some(Kind::Touch), Some(Kind::Iphone));
        assert!(!swappable(&[pad, key, None, None], 0, 1));
        assert!(!swappable(&[pad, touch, None, None], 0, 1));
        assert!(swappable(&[key, phone, None, None], 0, 1));
        assert!(swappable(&[pad, key, phone, None], 1, 2));
        assert!(swappable(&[pad, pad, touch, None], 0, 1));
        assert!(!swappable(&[pad, None, None, None], 0, 2));
    }
}
