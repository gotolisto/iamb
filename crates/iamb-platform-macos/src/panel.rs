//! The non-activating panel of the fill-in form and the search palette (ADR-0017; spec §4.4,
//! §7), behind the `panel` feature, which only the UI host enables.
//!
//! A panel of the UI host must take the keyboard without making Iamb the active application:
//! the app the user is typing into keeps its menu bar and its active state, and the focus can
//! be handed back to it exactly. `AppKit` offers that only to an `NSPanel` with
//! `NSWindowStyleMask::NonactivatingPanel`. Tauri creates every window as an `NSWindow` of its
//! own subclass, so [`make_non_activating_panel`] changes the class of that window, once, to
//! `IambNonActivatingPanel`, a subclass of `NSPanel` registered at run time, and then sets the
//! panel's style and behaviour.
//!
//! # Why changing the class is sound
//!
//! `object_setClass` replaces the object's class pointer and nothing else: the object keeps its
//! memory, allocated and initialised by the class it was created with (the *original* class).
//! The conversion is refused unless all of this holds, each checked at run time:
//!
//! - `NSPanel` has the instance size of `NSWindow` (it adds no instance variables), and
//!   `IambNonActivatingPanel` adds none either, so every method of the new class reads and
//!   writes only the `NSWindow` part of the object, which is laid out exactly as it was;
//! - the original class is an `NSWindow` subclass at least as large as `NSWindow`, and not
//!   already an `NSPanel`; the instance variables it added stay where they are, untouched by
//!   the new class, and its own code still finds them at their fixed offsets;
//! - the original class has no `dealloc` of its own (the new class's `dealloc` is `NSWindow`'s
//!   through `NSPanel`, so no clean-up of the original class can be skipped);
//! - every converted window has the same original class, which is remembered, so the one
//!   method the panel takes from it (below) always runs on memory that class laid out.
//!
//! The original class's other overrides no longer apply. For Tauri's window those are the
//! zoom button's state when the style changes (a panel has no title bar), dragging a window
//! that is movable by its background (the panels are not), and whether the window may become
//! main (a panel never does) or key. The last matters: Tauri's `set_focusable(false)`, which
//! the UI host uses while a panel paints out of sight (`prepaint_panel`), works through that
//! override. So `canBecomeKeyWindow` asks the original class's implementation, and the panel
//! takes the keyboard exactly when the window could before.
//!
//! The style mask is changed after the class, so that `NSPanel`'s own `setStyleMask:` sees the
//! non-activating flag, and the UI host converts its panels right after creating them, before
//! they are ever shown.

use std::ptr;
use std::sync::OnceLock;

use iamb_platform::PlatformError;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSPanel, NSResponder, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The class every converted window had before (module docs). Set by the first conversion,
/// never changed.
static ORIGINAL: OnceLock<&'static AnyClass> = OnceLock::new();

define_class!(
    // SAFETY: `NSPanel` may be subclassed. The class has no instance variables and no `Drop`,
    // so it adds no `dealloc` and its instances have `NSPanel`'s layout. Both methods have
    // the signatures `AppKit` declares (`- (BOOL)canBecomeKeyWindow`,
    // `- (BOOL)canBecomeMainWindow`). Windows are main-thread only.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IambNonActivatingPanel"]
    struct Panel;

    impl Panel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            match ORIGINAL.get() {
                // SAFETY: every instance of this class was an instance of `original` until
                // `convert` changed its class (the class is never allocated directly), so its
                // memory was allocated and initialised by `original`, and `original`'s
                // implementation finds its instance variables where it expects them (module
                // docs). `canBecomeKeyWindow` takes no argument and returns a `BOOL`.
                Some(original) => unsafe { msg_send![super(self, *original), canBecomeKeyWindow] },
                None => true,
            }
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

/// Turns `window` (a Tauri window, through `raw-window-handle`) into a non-activating panel:
/// showing it makes it the key window, so it gets the keys, without activating Iamb, so the
/// app the user was typing into stays the active one (ADR-0017; module docs).
///
/// The window becomes an `NSPanel` with the non-activating style, that becomes key when shown
/// (`becomesKeyOnlyIfNeeded` off), floats over normal windows, stays when Iamb is not active
/// (`hidesOnDeactivate` off) and joins every Space, also as an auxiliary window over a
/// full-screen app. Call it on the main thread, before the window is first shown. Calling it
/// again for the same window changes nothing that the first call did not.
///
/// # Errors
///
/// [`PlatformError::Invalid`] off the main thread, for a handle that is not an `AppKit` view in
/// a window, and when the window's class does not allow the conversion (module docs): then
/// nothing was changed, and the window stays an ordinary one.
pub fn make_non_activating_panel(window: &impl HasWindowHandle) -> Result<(), PlatformError> {
    if MainThreadMarker::new().is_none() {
        return Err(invalid("make_non_activating_panel: not on the main thread"));
    }
    let handle = window
        .window_handle()
        .map_err(|_| invalid("make_non_activating_panel: the window has no handle"))?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return Err(invalid("make_non_activating_panel: not an AppKit window"));
    };
    // SAFETY: an `AppKit` window handle holds a valid, non-null `NSView` for as long as the
    // handle is borrowed (`raw-window-handle`'s contract for `WindowHandle`), and `handle`
    // lives to the end of this function. Only code on the main thread could close the window,
    // and this function, on the main thread (checked above), does not. Views are main-thread
    // only.
    let view: &NSView = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
    let ns_window = view
        .window()
        .ok_or_else(|| invalid("make_non_activating_panel: the view is in no window"))?;
    convert(&ns_window)?;
    let panel = ns_window
        .downcast_ref::<NSPanel>()
        .ok_or_else(|| invalid("make_non_activating_panel: the window is no panel"))?;
    panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setFloatingPanel(true);
    panel.setHidesOnDeactivate(false);
    panel.setCollectionBehavior(
        panel.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    Ok(())
}

const fn invalid(context: &'static str) -> PlatformError {
    PlatformError::Invalid { context }
}

/// Changes `window`'s class to [`Panel`], when the checks of the module docs pass; nothing
/// when it already is one. Main thread (the window is main-thread only).
fn convert(window: &Retained<NSWindow>) -> Result<(), PlatformError> {
    let panel = Panel::class();
    let object: &AnyObject = window;
    let current = object.class();
    if ptr::eq(current, panel) {
        return Ok(());
    }
    if window.isKindOfClass(NSPanel::class()) {
        return Err(invalid(
            "make_non_activating_panel: the window is a panel of another class",
        ));
    }
    let size = NSWindow::class().instance_size();
    if NSPanel::class().instance_size() != size
        || panel.instance_size() != size
        || current.instance_size() < size
    {
        return Err(invalid(
            "make_non_activating_panel: the panel class does not share the window's layout",
        ));
    }
    if implementation(current, sel!(dealloc)) != implementation(NSWindow::class(), sel!(dealloc)) {
        return Err(invalid(
            "make_non_activating_panel: the window's class has its own dealloc",
        ));
    }
    if !ptr::eq(*ORIGINAL.get_or_init(|| current), current) {
        return Err(invalid(
            "make_non_activating_panel: the window's class differs from the first window's",
        ));
    }
    // SAFETY: `window` is a live window, used on the main thread only. The new class is a
    // subclass of `NSPanel`, which has `NSWindow`'s layout, and adds no instance variables;
    // the current class is an `NSWindow` subclass at least that large, without a `dealloc`
    // of its own, and is `ORIGINAL`, whose `canBecomeKeyWindow` the new class calls (all
    // checked above). So every method of the new class touches only memory laid out as it
    // expects, the current class's own instance variables stay valid for its code, and no
    // clean-up is skipped (module docs).
    unsafe {
        objc2::ffi::object_setClass(Retained::as_ptr(window).cast_mut().cast(), panel);
    }
    Ok(())
}

/// The address of `class`'s implementation of `sel`, inherited or its own.
fn implementation(class: &AnyClass, sel: Sel) -> Option<usize> {
    class
        .instance_method(sel)
        .map(|method| method.implementation() as usize)
}
