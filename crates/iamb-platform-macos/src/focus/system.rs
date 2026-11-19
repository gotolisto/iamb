//! The system calls of the focus module: the frontmost application, the window server's
//! list, the Accessibility reads and the secure-entry flag. What their answers mean is
//! decided in the plain-Rust modules beside this one.
//!
//! # Threads
//!
//! Everything here may be called from any thread, with one exception that the function
//! itself takes care of: [`secure_input`] hands its Carbon call to the main thread.
//! `NSRunningApplication` is documented as thread safe; what the workspace reports as
//! frontmost is refreshed as the main run loop turns, which is one more reason why the
//! worker's main thread must run it.
//!
//! # What an Accessibility read can cost
//!
//! Every read is a message to the application, which a hung application does not answer.
//! Each element reference therefore gets a messaging timeout before it is used (the system
//! keeps the timeout per reference, not per element). A timeout of zero would mean "the
//! default", so the smallest one given is `MIN_MESSAGE_TIMEOUT`.

use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use iamb_platform::{AppIdentity, FieldSecurity, FocusToken, UnknownReason};
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFRunLoop, CFString, CFType,
};
use objc2_core_graphics::{
    CGWindowID, CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowLayer,
    kCGWindowNumber, kCGWindowOwnerPID,
};

use super::field::{self, Attribute, Text};
use super::identity::{self, WindowEntry};
use super::message_timeout;
use crate::listener::{main_thread, sys};

/// The frontmost application of the workspace: the one that receives key events. `None`
/// when there is none, or it has ended.
pub(crate) fn frontmost() -> Option<AppIdentity> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let bundle_id = app.bundleIdentifier().map(|id| id.to_string());
        let executable = app
            .executableURL()
            .and_then(|url| url.lastPathComponent())
            .map(|name| name.to_string());
        identity::identity(
            bundle_id.as_deref(),
            executable.as_deref(),
            app.processIdentifier(),
            std::process::id(),
        )
    })
}

/// The process id of the frontmost application, and nothing else about it: one property read
/// of the workspace, for the injector's focus guard, which asks before every step.
pub(crate) fn frontmost_pid() -> Option<i32> {
    autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
    })
}

/// The frontmost application and the token of its front window (window 0 when it shows no
/// ordinary window).
pub(crate) fn front() -> Option<(AppIdentity, FocusToken)> {
    let app = frontmost()?;
    let token = FocusToken::from_raw(app.pid, front_window(app.pid));
    Some((app, token))
}

/// One window of the window server's list. Only the owner, the layer and the number are
/// read; the entry's other fields (the window's name among them) are not touched.
fn entry(window: &CFDictionary<CFString, CFType>) -> Option<WindowEntry> {
    // SAFETY: constants the framework initialises when it is loaded; they never change.
    let (owner, layer, number) = unsafe { (kCGWindowOwnerPID, kCGWindowLayer, kCGWindowNumber) };
    let read = |key: &CFString| window.get(key)?.downcast::<CFNumber>().ok()?.as_i64();
    Some(WindowEntry {
        owner: read(owner)?,
        layer: read(layer)?,
        number: read(number)?,
    })
}

/// The window server's list for `option`, front to back. Asks no application anything.
fn windows(option: CGWindowListOption, relative_to: CGWindowID) -> Vec<WindowEntry> {
    let Some(list) = CGWindowListCopyWindowInfo(option, relative_to) else {
        return Vec::new();
    };
    // SAFETY: the function is documented to return an array of dictionaries with string
    // keys. The values are taken as untyped objects and their type is checked in `entry`.
    let list: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(list) };
    list.iter().filter_map(|window| entry(&window)).collect()
}

/// The window number of the front ordinary window of process `pid`; 0 when it shows none.
pub(crate) fn front_window(pid: u32) -> u64 {
    let on_screen =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    identity::front_window(windows(on_screen, kCGNullWindowID), pid)
}

/// Whether the token's window exists (on screen or not) and belongs to the token's process.
pub(crate) fn window_exists(token: FocusToken) -> bool {
    let Ok(number) = CGWindowID::try_from(token.window()) else {
        return false;
    };
    identity::lists_window(
        windows(CGWindowListOption::OptionIncludingWindow, number),
        token.pid(),
        token.window(),
    )
}

/// Whether secure keyboard entry is on in the login session, asked on the main thread.
/// `None` when the main thread did not get to it within `limit`.
pub(crate) fn secure_input(limit: Duration) -> Option<bool> {
    main_thread::call(limit, |_mtm: MainThreadMarker| {
        // SAFETY: the function takes no arguments and reads a flag of the login session; the
        // marker proves this is the main thread, which its header asks for.
        unsafe { sys::IsSecureEventInputEnabled() != 0 }
    })
}

/// The Accessibility element of application `pid`. Creating it sends the application
/// nothing.
pub(crate) fn application(pid: u32) -> Option<CFRetained<AXUIElement>> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    // SAFETY: the function takes a process id by value and returns a new element reference;
    // a process id that names no application gives an element whose reads fail.
    Some(unsafe { AXUIElement::new_application(pid) })
}

/// Gives every later message through `element` the timeout `left`.
pub(crate) fn set_timeout(element: &AXUIElement, left: Duration) {
    // SAFETY: `element` is a valid reference and the timeout a positive number of seconds.
    // A refusal leaves the default timeout; the caller's own deadline still holds.
    let _ = unsafe { element.set_messaging_timeout(message_timeout(left)) };
}

/// Reads one attribute of `element`: a message to its application. The error is the
/// `AXError` code.
///
/// [`Attribute`] names the focused element, the role and the subrole and nothing else, so
/// this function cannot be used to read what a field contains.
pub(crate) fn copy_attribute(
    element: &AXUIElement,
    attribute: Attribute,
) -> Result<CFRetained<CFType>, i32> {
    let name = CFString::from_static_str(attribute.name());
    let mut value: *const CFType = ptr::null();
    // SAFETY: `element` and `name` are valid for the call and `value` is a valid place for
    // the result.
    let error = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    if error != AXError::Success {
        return Err(error.0);
    }
    let value = NonNull::new(value.cast_mut()).ok_or(field::code::NO_VALUE)?;
    // SAFETY: on success the function stored an object the caller owns (the "Copy" rule).
    Ok(unsafe { CFRetained::from_raw(value) })
}

/// The element of application `app` that has the keyboard focus.
pub(crate) fn focused_element(app: &AXUIElement) -> Result<CFRetained<AXUIElement>, i32> {
    copy_attribute(app, Attribute::FocusedElement)?
        .downcast::<AXUIElement>()
        .map_err(|_| field::code::NO_VALUE)
}

/// The role or the subrole of `element`.
fn text(element: &AXUIElement, attribute: Attribute) -> Text {
    match copy_attribute(element, attribute) {
        Ok(value) => value
            .downcast::<CFString>()
            .map_or(Text::Absent, |name| Text::Is(name.to_string())),
        Err(code) => Text::failed(code),
    }
}

/// What is left until `deadline`; `None` when it has passed.
fn left(deadline: Instant) -> Option<Duration> {
    Some(deadline.saturating_duration_since(Instant::now())).filter(|left| !left.is_zero())
}

/// The state of the focused element of application `pid`, with every message to the
/// application timing out at `deadline`.
pub(crate) fn focused_field(pid: u32, deadline: Instant) -> FieldSecurity {
    const TIMEOUT: FieldSecurity = FieldSecurity::Unknown(UnknownReason::Timeout);
    let Some(app) = application(pid) else {
        return FieldSecurity::Unknown(UnknownReason::NotInspectable);
    };
    let Some(time) = left(deadline) else {
        return TIMEOUT;
    };
    set_timeout(&app, time);
    let element = match focused_element(&app) {
        Ok(element) => element,
        Err(code) => {
            return FieldSecurity::Unknown(field::unreadable(code, left(deadline).is_none()));
        }
    };
    let Some(time) = left(deadline) else {
        return TIMEOUT;
    };
    set_timeout(&element, time);
    let subrole = text(&element, Attribute::Subrole);
    if subrole.names_secure_field() {
        return FieldSecurity::Secure;
    }
    let role = match left(deadline) {
        Some(time) => {
            set_timeout(&element, time);
            text(&element, Attribute::Role)
        }
        None => Text::Failed(field::code::CANNOT_COMPLETE),
    };
    field::judge(&role, &subrole, left(deadline).is_none())
}

/// Whether process `pid` is a running application.
pub(crate) fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .is_some_and(|app| !app.isTerminated())
    })
}

/// Asks the system to make application `pid` the active one, with its main and key windows
/// in front. `false` when the request could not be sent.
pub(crate) fn activate(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .is_some_and(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
    })
}

/// Lets `duration` pass. On the main thread the run loop is run meanwhile: what the
/// workspace reports as frontmost changes only as the main run loop turns.
pub(crate) fn pause(duration: Duration) {
    let started = Instant::now();
    if MainThreadMarker::new().is_some() {
        let _ = CFRunLoop::run_in_mode(main_thread::default_mode(), duration.as_secs_f64(), false);
    }
    // Off the main thread, or when the run loop had nothing to wait for and returned early.
    if let Some(rest) = duration.checked_sub(started.elapsed()) {
        std::thread::sleep(rest);
    }
}
