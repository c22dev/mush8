// Mush8 - ios.rs
// Constantin Clerc (c22dev) - 0.1
use super::{LAYER, Shared, gpu};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject, NSObjectProtocol};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_core_foundation::CGRect;
use objc2_foundation::NSString;
use objc2_metal::MTLPixelFormat;
use objc2_quartz_core::{CALayer, CAMetalLayer};
use objc2_ui_kit::*;
use std::cell::RefCell;
use std::sync::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

type Boot = Box<dyn FnOnce() -> anyhow::Result<()> + Send>;

static BOOT: Mutex<Option<Boot>> = Mutex::new(None);
pub(super) static ACTIVE: AtomicBool = AtomicBool::new(true);
static FOREGROUND: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

fn set_active(v: bool) {
    ACTIVE.store(v, Relaxed);
    let _g = FOREGROUND.0.lock().unwrap();
    FOREGROUND.1.notify_all();
}

pub fn wait_foreground() {
    if ACTIVE.load(Relaxed) {
        return;
    }
    crate::kernel::blocking(|| {
        let mut g = FOREGROUND.0.lock().unwrap();
        while !ACTIVE.load(Relaxed) {
            g = FOREGROUND.1.wait(g).unwrap();
        }
    });
}

pub(super) fn panel_hosts() -> Vec<(Retained<UIView>, bool)> {
    UI.with(|ui| {
        let ui = ui.borrow();
        let Some(ui) = ui.as_ref() else { return Vec::new() };
        let mut hosts = vec![(ui.root.clone(), false)];
        hosts.extend(ui.metal.superview().filter(|v| !std::ptr::eq(&**v, &*ui.root)).map(|v| (v, true)));
        hosts
    })
}

pub(super) fn presenter() -> Option<Retained<UIViewController>> {
    let mut vc = UI.with(|ui| ui.borrow().as_ref().and_then(|ui| ui.window.rootViewController()))?;
    while let Some(next) = vc.presentedViewController() {
        vc = next;
    }
    Some(vc)
}

pub(super) static CONTROLLER_MODE: AtomicBool = AtomicBool::new(false);
pub(super) static TV: AtomicBool = AtomicBool::new(false);
static WAITING: AtomicBool = AtomicBool::new(false);
const TV_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
static WANT_TOUCH: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn tv() -> bool {
    TV.load(Relaxed)
}

pub fn set_touch_controls(visible: bool) {
    WANT_TOUCH.store(1 + visible as u8, Relaxed);
    dispatch2::DispatchQueue::main().exec_async(refresh_touch);
}

pub fn set_touch_idle(idle: bool) {
    dispatch2::DispatchQueue::main().exec_async(move || super::touch::set_idle(idle));
}

fn refresh_touch() {
    let want = WANT_TOUCH.load(Relaxed);
    if !CONTROLLER_MODE.load(Relaxed) && want != 0 {
        super::touch::set_visible(want == 2 || TV.load(Relaxed))
    }
}

struct Ui {
    window: Retained<UIWindow>,
    root: Retained<UIView>,
    metal: Retained<MetalView>,
    label: Retained<UILabel>,
    tv: Option<Retained<UIWindow>>,
    accessory: Option<Retained<NSObject>>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static PENDING_TV: RefCell<Option<Retained<UIWindowScene>>> = const { RefCell::new(None) };
}

fn is_tv(session: &UISceneSession) -> bool {
    session.role().isEqualToString(unsafe { UIWindowSceneSessionRoleExternalDisplayNonInteractive })
}

define_class!(
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NXMetalView"]
    struct MetalView;

    impl MetalView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            CAMetalLayer::class()
        }

        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            let layer: Retained<CALayer> = self.layer();
            layer.setContentsScale(self.contentScaleFactor());
        }
    }
);

define_class!(
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NXViewController"]
    struct ViewController;

    impl ViewController {
        #[unsafe(method(prefersStatusBarHidden))]
        fn prefers_status_bar_hidden(&self) -> bool {
            true
        }

        #[unsafe(method(prefersHomeIndicatorAutoHidden))]
        fn prefers_home_indicator_hidden(&self) -> bool {
            true
        }

        #[unsafe(method(preferredScreenEdgesDeferringSystemGestures))]
        fn deferred_edges(&self) -> UIRectEdge {
            UIRectEdge::All
        }
    }
);

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NXSceneDelegate"]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(&self, scene: &UIScene, session: &UISceneSession, _options: &UISceneConnectionOptions) {
            let Some(scene) = scene.downcast_ref::<UIWindowScene>() else { return };
            eprintln!("[scene] {} connected", session.role());
            if is_tv(session) {
                attach_tv(scene, self.mtm());
            } else {
                connect(scene, self.mtm());
            }
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn did_disconnect(&self, scene: &UIScene) {
            if is_tv(&scene.session()) {
                detach_tv();
            }
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn did_enter_background(&self, scene: &UIScene) {
            if !is_tv(&scene.session()) {
                set_active(false);
            }
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn will_enter_foreground(&self, scene: &UIScene) {
            if !is_tv(&scene.session()) {
                set_active(true);
            }
        }
    }
);

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NXAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl UIApplicationDelegate for AppDelegate {}
);

fn metal_view(scene: &UIWindowScene, window: &UIWindow, mtm: MainThreadMarker) -> (Retained<MetalView>, Retained<CAMetalLayer>) {
    let frame = UICoordinateSpace::bounds(window);
    let view: Retained<MetalView> = unsafe { msg_send![MetalView::alloc(mtm), initWithFrame: frame] };
    view.setBackgroundColor(Some(&UIColor::blackColor()));
    view.setContentScaleFactor(scene.screen().nativeScale());
    let layer: Retained<CAMetalLayer> = unsafe { Retained::cast_unchecked(view.layer()) };
    layer.setDevice(Some(&gpu().device.0));
    layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    (view, layer)
}

fn text_label(mtm: MainThreadMarker, frame: CGRect, text: &str) -> Retained<UILabel> {
    let label = UILabel::new(mtm);
    label.setFrame(frame.inset(40.0));
    label.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
    unsafe { label.setTextColor(Some(&UIColor::whiteColor())) };
    label.setTextAlignment(NSTextAlignment::Center);
    label.setNumberOfLines(0);
    label.setText(Some(&NSString::from_str(text)));
    label
}

fn connect(scene: &UIWindowScene, mtm: MainThreadMarker) {
    if UI.with(|ui| ui.borrow().is_some()) {
        eprintln!("[scene] Mush8 is already open in another window: closing this one");
        UIApplication::sharedApplication(mtm).requestSceneSessionDestruction_options_errorHandler(&scene.session(), None, None);
        return;
    }
    let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
    let frame = UICoordinateSpace::bounds(&*window);
    let (metal, layer) = metal_view(scene, &window, mtm);
    let _ = LAYER.set(Shared(layer));
    metal.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);

    let root = UIView::initWithFrame(UIView::alloc(mtm), frame);
    root.setBackgroundColor(Some(&UIColor::blackColor()));
    root.addSubview(&metal);
    let label = text_label(mtm, frame, "Starting…");
    super::touch::install(&root, mtm);
    root.addSubview(&label);

    let vc: Retained<ViewController> = unsafe { msg_send![ViewController::alloc(mtm), init] };
    vc.setView(Some(&root));
    window.setRootViewController(Some(&vc));
    window.makeKeyAndVisible();
    let accessory = register_tv(&vc, mtm);
    UI.with(|ui| *ui.borrow_mut() = Some(Ui { window, root, metal, label, tv: None, accessory }));
    if let Some(tv) = PENDING_TV.take() {
        attach_tv(&tv, mtm);
    }

    unsafe extern "C" {
        fn mush8_show_launcher(view_controller: *mut std::ffi::c_void, play: extern "C" fn(), controller: extern "C" fn());
    }
    let vc = Retained::as_ptr(&vc) as usize;
    dispatch2::DispatchQueue::main().exec_async(move || unsafe { mush8_show_launcher(vc as *mut _, play, controller) });
}

fn register_tv(vc: &ViewController, mtm: MainThreadMarker) -> Option<Retained<NSObject>> {
    let class = AnyClass::get(c"UISceneAccessory")?;
    let role = unsafe { UIWindowSceneSessionRoleExternalDisplayNonInteractive };
    let config = UISceneConfiguration::initWithName_sessionRole(UISceneConfiguration::alloc(mtm), Some(&NSString::from_str("TV")), role);
    unsafe { config.setDelegateClass(Some(SceneDelegate::class())) };
    let accessory: Retained<NSObject> = unsafe { msg_send![class, externalNonInteractiveSceneAccessoryWithConfiguration: &*config] };
    let registration: Retained<NSObject> = unsafe { msg_send![vc, registerSceneAccessory: &*accessory] };
    eprintln!("[tv] registered for external displays");
    Some(registration)
}

fn attach_tv(scene: &UIWindowScene, mtm: MainThreadMarker) {
    if UI.with(|ui| ui.borrow().is_none()) {
        PENDING_TV.set(Some(scene.retain()));
        return;
    }
    let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
    let host = UIView::initWithFrame(UIView::alloc(mtm), UICoordinateSpace::bounds(&*window));
    host.setBackgroundColor(Some(&UIColor::blackColor()));
    let vc: Retained<ViewController> = unsafe { msg_send![ViewController::alloc(mtm), init] };
    vc.setView(Some(&host));
    window.setRootViewController(Some(&vc));
    window.setHidden(false);
    let size = UICoordinateSpace::bounds(&*window).size;
    eprintln!("[tv] external display connected ({}x{} @{}x): the game moves there, this phone becomes a controller", size.width, size.height, scene.screen().nativeScale());
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let Some(ui) = ui.as_mut() else { return };
        move_game(ui, &host, scene.screen().nativeScale());
        ui.tv = Some(window);
    });
    TV.store(true, Relaxed);
    refresh_touch();
    if WAITING.swap(false, Relaxed) {
        start();
    }
}

fn detach_tv() {
    PENDING_TV.set(None);
    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let Some(ui) = ui.as_mut() else { return };
        if ui.tv.take().is_none() {
            return;
        }
        eprintln!("[tv] external display gone: the game is back on the phone");
        let scale = ui.window.windowScene().map_or(1.0, |s| s.screen().nativeScale());
        let root = ui.root.clone();
        move_game(ui, &root, scale);
    });
    TV.store(false, Relaxed);
    refresh_touch();
}

fn move_game(ui: &Ui, host: &UIView, scale: f64) {
    ui.metal.removeFromSuperview();
    ui.metal.setFrame(host.bounds());
    ui.metal.setContentScaleFactor(scale);
    host.insertSubview_atIndex(&ui.metal, 0);
}

extern "C" fn controller() {
    if let Some(mtm) = MainThreadMarker::new() {
        UIApplication::sharedApplication(mtm).setIdleTimerDisabled(true);
    }
    CONTROLLER_MODE.store(true, Relaxed);
    super::touch::set_visible(true);
    let _ = std::thread::Builder::new().name("controller".into()).spawn(|| crate::remote::run_controller(set_status));
}

extern "C" fn play() {
    if let Some(mtm) = MainThreadMarker::new() {
        UIApplication::sharedApplication(mtm).setIdleTimerDisabled(true);
    }
    let available = UI.with(|ui| {
        ui.borrow().as_ref().is_some_and(|ui| ui.tv.is_none() && ui.accessory.as_ref().is_some_and(|r| unsafe { msg_send![&**r, isAvailable] }))
    });
    if available {
        eprintln!("[tv] an external display is coming: waiting for it before sizing the screen");
        WAITING.store(true, Relaxed);
        let fallback = dispatch2::DispatchTime::try_from(TV_WAIT).map(|t| {
            dispatch2::DispatchQueue::main().after(t, || {
                if WAITING.swap(false, Relaxed) {
                    start();
                }
            })
        });
        if matches!(fallback, Ok(Ok(()))) {
            return;
        }
        WAITING.store(false, Relaxed);
    }
    start();
}

fn start() {
    UI.with(|ui| {
        if let Some(ui) = ui.borrow().as_ref() {
            if let Some(tv) = &ui.tv {
                let size = UICoordinateSpace::bounds(&**tv).size;
                crate::aspect::set_screen(size.width, size.height, size.width);
                return;
            }
            let size = UICoordinateSpace::bounds(&*ui.window).size;
            let inset = ui.window.safeAreaInsets();
            let (long, short, edge) = if size.width >= size.height {
                (size.width, size.height, inset.left.max(inset.right))
            } else {
                (size.height, size.width, inset.top.max(inset.bottom))
            };
            crate::aspect::set_screen(long, short, long - 2.0 * edge);
        }
    });
    if let Some(boot) = BOOT.lock().unwrap().take() {
        std::thread::Builder::new()
            .name("boot".into())
            .spawn(move || match boot() {
                Ok(()) => set_status(""),
                Err(e) => {
                    eprintln!("[host] {e:#}");
                    set_status(&format!("Mush8 could not start:\n\n{e:#}"));
                }
            })
            .unwrap();
    }
}

trait Inset {
    fn inset(self, by: f64) -> Self;
}

impl Inset for CGRect {
    fn inset(mut self, by: f64) -> Self {
        self.origin.x += by;
        self.origin.y += by;
        self.size.width = (self.size.width - 2.0 * by).max(0.0);
        self.size.height = (self.size.height - 2.0 * by).max(0.0);
        self
    }
}

fn set_status(text: &str) {
    let text = text.to_string();
    dispatch2::DispatchQueue::main().exec_async(move || {
        UI.with(|ui| {
            if let Some(ui) = ui.borrow().as_ref() {
                ui.label.setHidden(text.is_empty());
                ui.label.setText(Some(&NSString::from_str(&text)));
            }
        })
    });
}

pub fn run_app(boot: impl FnOnce() -> anyhow::Result<()> + Send + 'static) -> ! {
    let mtm = MainThreadMarker::new().expect("run_app must be called on the main thread");
    *BOOT.lock().unwrap() = Some(Box::new(boot));
    let _ = (SceneDelegate::class(), ViewController::class(), MetalView::class());
    UIApplication::main(None, Some(&NSString::from_class(AppDelegate::class())), mtm)
}
