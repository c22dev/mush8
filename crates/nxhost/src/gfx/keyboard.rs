// Mush8 - keyboard.rs
// Constantin Clerc (c22dev) - 0.1
use objc2::MainThreadMarker;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;

pub static TYPING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default)]
pub struct Prompt {
    pub header: String,
    pub sub: String,
    pub guide: String,
    pub ok: String,
    pub initial: String,
    pub password: bool,
    pub numeric: bool,
}

pub fn typing() -> bool {
    TYPING.load(Relaxed)
}

pub fn prompt(p: Prompt) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    TYPING.store(true, Relaxed);
    dispatch2::DispatchQueue::main().exec_async(move || {
        let mtm = MainThreadMarker::new().expect("main queue runs on the main thread");
        if !show(mtm, &p, tx.clone()) {
            let _ = tx.send(None);
        }
    });
    let r = rx.recv().ok().flatten();
    std::thread::sleep(std::time::Duration::from_millis(150));
    TYPING.store(false, Relaxed);
    r
}

fn title(p: &Prompt) -> String {
    [&p.header, &p.sub].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_else(|| "Enter text".into())
}

fn message(p: &Prompt) -> String {
    if p.header.is_empty() { String::new() } else { p.sub.clone() }
}

#[cfg(target_os = "macos")]
fn show(mtm: MainThreadMarker, p: &Prompt, tx: mpsc::Sender<Option<String>>) -> bool {
    use block2::RcBlock;
    use objc2::MainThreadOnly;
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSModalResponse, NSSecureTextField, NSTextField, NSWindow};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let w = super::overlay::game_window();
    if w == 0 {
        return false;
    }
    let game: &NSWindow = unsafe { &*(w as *const NSWindow) };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&title(p)));
    alert.setInformativeText(&NSString::from_str(&message(p)));
    alert.addButtonWithTitle(&NSString::from_str(&p.ok));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 24.0));
    let field: objc2::rc::Retained<NSTextField> = if p.password {
        objc2::rc::Retained::into_super(NSSecureTextField::initWithFrame(NSSecureTextField::alloc(mtm), frame))
    } else {
        NSTextField::initWithFrame(NSTextField::alloc(mtm), frame)
    };
    field.setStringValue(&NSString::from_str(&p.initial));
    let guide = if p.guide.is_empty() && p.numeric { "Numbers only" } else { &p.guide };
    if !guide.is_empty() {
        field.setPlaceholderString(Some(&NSString::from_str(guide)));
    }
    alert.setAccessoryView(Some(&field));
    alert.window().setInitialFirstResponder(Some(&field));
    let read = field.clone();
    let done = RcBlock::new(move |code: NSModalResponse| {
        let _ = tx.send((code == NSAlertFirstButtonReturn).then(|| read.stringValue().to_string()));
    });
    alert.beginSheetModalForWindow_completionHandler(game, Some(&done));
    true
}

#[cfg(target_os = "ios")]
fn show(mtm: MainThreadMarker, p: &Prompt, tx: mpsc::Sender<Option<String>>) -> bool {
    use block2::RcBlock;
    use objc2_foundation::NSString;
    use objc2_ui_kit::{UIAlertAction, UIAlertActionStyle, UIAlertController, UIAlertControllerStyle, UIKeyboardType, UITextField, UITextInputTraits};
    use std::ptr::NonNull;
    let Some(host) = super::ios::presenter() else { return false };
    let msg = message(p);
    let alert = UIAlertController::alertControllerWithTitle_message_preferredStyle(
        Some(&NSString::from_str(&title(p))),
        (!msg.is_empty()).then(|| NSString::from_str(&msg)).as_deref(),
        UIAlertControllerStyle::Alert,
        mtm,
    );
    let (initial, guide, password, numeric) = (p.initial.clone(), p.guide.clone(), p.password, p.numeric);
    let configure = RcBlock::new(move |f: NonNull<UITextField>| {
        let f = unsafe { f.as_ref() };
        f.setText(Some(&NSString::from_str(&initial)));
        if !guide.is_empty() {
            f.setPlaceholder(Some(&NSString::from_str(&guide)));
        }
        f.setSecureTextEntry(password);
        if numeric {
            f.setKeyboardType(UIKeyboardType::NumberPad);
        }
    });
    alert.addTextFieldWithConfigurationHandler(Some(&configure));
    let weak = objc2::rc::Weak::from_retained(&alert);
    let ok_tx = tx.clone();
    let ok = RcBlock::new(move |_: NonNull<UIAlertAction>| {
        let text = weak.load().and_then(|a| a.textFields()).and_then(|f| f.firstObject()).and_then(|f| f.text()).map(|t| t.to_string()).unwrap_or_default();
        let _ = ok_tx.send(Some(text));
    });
    let cancel = RcBlock::new(move |_: NonNull<UIAlertAction>| {
        let _ = tx.send(None);
    });
    alert.addAction(&UIAlertAction::actionWithTitle_style_handler(Some(&NSString::from_str("Cancel")), UIAlertActionStyle::Cancel, Some(&cancel), mtm));
    let ok = UIAlertAction::actionWithTitle_style_handler(Some(&NSString::from_str(&p.ok)), UIAlertActionStyle::Default, Some(&ok), mtm);
    alert.addAction(&ok);
    alert.setPreferredAction(Some(&ok));
    host.presentViewController_animated_completion(&alert, true, None);
    true
}
