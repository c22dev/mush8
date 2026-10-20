// Mush8 - ios.rs
// Constantin Clerc (c22dev) - 0.1
use super::{LAYER, Shared, gpu};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject, NSObjectProtocol};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
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

pub(super) fn root_view() -> Option<Retained<UIView>> {
    UI.with(|ui| ui.borrow().as_ref().and_then(|ui| ui.window.rootViewController()).and_then(|vc| vc.view()))
}

pub(super) static CONTROLLER_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_touch_controls(visible: bool) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        if !CONTROLLER_MODE.load(Relaxed) {
            super::touch::set_visible(visible)
        }
    });
}

struct Ui {
    window: Retained<UIWindow>,
    label: Retained<UILabel>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
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
            let b=self.bounds().size;
            if b.width>=b.height { crate::display::update_size(b.width,b.height); }
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
        fn will_connect(&self, scene: &UIScene, _session: &UISceneSession, _options: &UISceneConnectionOptions) {
            if let Some(scene) = scene.downcast_ref::<UIWindowScene>() {
                connect(scene, self.mtm());
            }
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn did_enter_background(&self, _scene: &UIScene) {
            set_active(false);
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn will_enter_foreground(&self, _scene: &UIScene) {
            set_active(true);
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
    let b=scene.screen().nativeBounds().size;
    super::super::display::update_size(b.width.max(b.height),b.width.min(b.height));
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
    let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
    let frame = UICoordinateSpace::bounds(&*window);
    let (view, layer) = metal_view(scene, &window, mtm);
    let _ = LAYER.set(Shared(layer));

    let label = text_label(mtm, frame, "Starting…");
    super::touch::install(&view, mtm);
    view.addSubview(&label);

    let vc: Retained<ViewController> = unsafe { msg_send![ViewController::alloc(mtm), init] };
    vc.setView(Some(&view));
    window.setRootViewController(Some(&vc));
    window.makeKeyAndVisible();
    UI.with(|ui| *ui.borrow_mut() = Some(Ui { window, label }));

    unsafe extern "C" {
        fn mush8_show_launcher(view_controller: *mut std::ffi::c_void, play: extern "C" fn(), controller: extern "C" fn());
    }
    let vc = Retained::as_ptr(&vc) as usize;
    dispatch2::DispatchQueue::main().exec_async(move || unsafe { mush8_show_launcher(vc as *mut _, play, controller) });
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
