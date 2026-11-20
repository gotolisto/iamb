//! The three windows (architecture §10, ADR-0017), and the two panels' common behaviour.
//!
//! | Label | Created | On close |
//! | --- | --- | --- |
//! | `library` | On demand, by the tray's "Open library" | Destroyed |
//! | `form` | At start, hidden (pre-warmed for the 150 ms target) | Hidden |
//! | `palette` | At start, hidden (pre-warmed for the 100 ms target) | Hidden |
//!
//! All three load the same page; the front end picks its view by the window's label.
//!
//! # The panels (the fill-in form, task 3.5; the search palette reuses it, task 3.6)
//!
//! Both are undecorated, always-on-top windows without a taskbar button, created hidden and
//! never destroyed. [`show_panel`] puts one in the upper third of the display the user is
//! working on (the display under the pointer, until the platform can name the display of the
//! focused window: `docs/ux/spec.md` §11.2 entry 13), horizontally centred, as high as its
//! content up to 70% of the display ([`panel_frame`]), then shows it and gives it the keyboard,
//! at once and without animation. [`hide_panel`] hides it. When a panel's window gains or loses
//! the keyboard, the panel's owner hears of it (`panel_events`), so that it can tell the
//! service (`PanelFocus`, architecture §4.5).
//!
//! How each system lets a panel have the keyboard (spec §4.4, §7):
//!
//! - **Windows**: the panel takes the foreground. The service granted the UI host the right
//!   to (`allow_foreground`) when it sent the request; the target app's title bar turns
//!   inactive until the panel hides and the focus is handed back (`restore_focus`).
//! - **macOS**: the panel is meant to be a non-activating panel (an `NSPanel` with the
//!   non-activating style mask), so the target app stays the active one. Tauri has no such
//!   window, and converting its `NSWindow` needs `unsafe` Objective-C calls, which this crate
//!   forbids (`unsafe_code = "forbid"` in the workspace lints); the conversion belongs in
//!   `iamb-platform-macos` (the function it needs is described in `apps/desktop/README.md`,
//!   "The macOS panel"). Until it exists the panel is
//!   an ordinary window of the accessory app: showing it activates Iamb (the target app's menu
//!   bar gives way to none), and the hand-back re-activates the target app. The flow, the
//!   `PanelFocus` messages and the hand-back are the same either way.
//! - **Linux** (development only): an ordinary window; nothing is handed back.
//!
//! # Opening the library cold
//!
//! Most of a cold open is the web view's engine starting a content process for the new view,
//! not loading the page: on Linux under Xvfb, about 600 ms of the 650 to 830 ms the open took
//! passed before the page's navigation began (task 3.2, `docs/test-strategy.md`). So on Linux
//! the library window is created *related* to the pre-warmed palette's web view and shares its
//! running content process; an open then takes about 60 to 140 ms. The form shares it too
//! (one content process instead of two while idle). The three windows load the
//! same page of the same app, so sharing the process changes nothing about what each can
//! reach. A crash of that process ends all three views together; the UI host keeps running
//! and the service is not affected. Whether Windows (`WebView2`) and macOS (`WKWebView`) need an
//! equivalent is decided by the measurement on their CI runners.

use std::sync::Arc;

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::log::Logger;
use crate::timing::OpenTimer;

/// The library window's label.
pub const LIBRARY: &str = "library";
/// The fill-in form's label.
pub const FORM: &str = "form";
/// The search palette's label.
pub const PALETTE: &str = "palette";

/// The fill-in form's width, in logical pixels (design tokens, `size.form-panel`).
pub const FORM_WIDTH: f64 = 440.0;
/// The palette's width (`size.palette`).
pub const PALETTE_WIDTH: f64 = 620.0;
/// A panel's height before its content has reported one.
const PANEL_DEFAULT_HEIGHT: f64 = 360.0;
/// The share of the display's height a panel may take (spec §4.4).
const PANEL_MAX_SHARE: f64 = 0.7;
/// The lowest a panel gets, whatever its content says.
const PANEL_MIN_HEIGHT: f64 = 120.0;

fn page() -> WebviewUrl {
    WebviewUrl::App("index.html".into())
}

/// Creates the hidden palette and form windows.
///
/// # Errors
///
/// When the palette cannot be created, or (except on Linux, where the form is created in a
/// later main-thread task that logs its failure) the form.
pub fn prewarm(app: &AppHandle, log: &Arc<Logger>) -> tauri::Result<()> {
    let palette = panel(app, PALETTE, PALETTE_WIDTH, 420.0).build()?;
    panel_events(&palette);
    #[cfg(target_os = "linux")]
    {
        let _ = palette.with_webview(|view| {
            RELATED.with(|related| *related.borrow_mut() = Some(view.inner()));
        });
        // Queued after the task above, so the palette's web view is known by then.
        let handle = app.clone();
        let log = Arc::clone(log);
        app.run_on_main_thread(move || {
            match related(panel(&handle, FORM, FORM_WIDTH, PANEL_DEFAULT_HEIGHT)).build() {
                Ok(form) => panel_events(&form),
                Err(e) => log.error(format_args!("cannot create the form window: {e}")),
            }
        })?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = log;
        let form = panel(app, FORM, FORM_WIDTH, PANEL_DEFAULT_HEIGHT).build()?;
        panel_events(&form);
    }
    Ok(())
}

/// A hidden, undecorated panel window.
fn panel<'a>(
    app: &'a AppHandle,
    label: &str,
    width: f64,
    height: f64,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    WebviewWindowBuilder::new(app, label, page())
        .title("Iamb")
        .visible(false)
        .focused(false)
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        // On the Space or virtual desktop the user is on, also over a full-screen app.
        .visible_on_all_workspaces(true)
        .inner_size(width, height)
        .center()
}

/// `builder`, related to the palette's web view when it is known (module docs). Main thread
/// only.
#[cfg(target_os = "linux")]
fn related(
    builder: WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
) -> WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    match RELATED.with(|related| related.borrow().clone()) {
        Some(view) => builder.with_related_view(view),
        None => builder,
    }
}

#[cfg(target_os = "linux")]
thread_local! {
    /// The palette's web view, kept on the main thread (GTK objects stay on it): the library
    /// window is created related to it, to share its content process (module docs).
    static RELATED: std::cell::RefCell<Option<webkit2gtk::WebView>> =
        const { std::cell::RefCell::new(None) };
}

/// A panel's window events: closing hides it (the panels must stay created), and a change of
/// the keyboard goes to the panel's owner (module docs). Runs on the main thread; the owner's
/// handler only queues a message.
fn panel_events(window: &WebviewWindow) {
    let handle = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = handle.hide();
        }
        WindowEvent::Focused(has) => {
            if handle.label() == FORM
                && let Some(ui) = handle.app_handle().try_state::<Arc<super::Ui>>()
            {
                ui.form.keyboard(*has);
            }
        }
        _ => {}
    });
}

/// Where a panel goes on a display: its top-left corner and size, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelFrame {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// The frame of a panel `width` logical pixels wide whose content is `content` logical pixels
/// high (or unknown), on a display whose work area (without the taskbar or menu bar) starts at
/// `origin` and is `area` physical pixels large at `scale` physical pixels per logical one:
/// horizontally centred, its top one sixth of the way down (so it sits in the upper third),
/// as high as its content between 120 and 70% of the area's height (spec §4.4).
#[must_use]
pub fn panel_frame(
    origin: (i32, i32),
    area: (u32, u32),
    scale: f64,
    width: f64,
    content: Option<f64>,
) -> PanelFrame {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let area_w = f64::from(area.0) / scale;
    let area_h = f64::from(area.1) / scale;
    let max_h = (area_h * PANEL_MAX_SHARE).max(PANEL_MIN_HEIGHT);
    let height = content
        .filter(|h| h.is_finite() && *h > 0.0)
        .unwrap_or(PANEL_DEFAULT_HEIGHT)
        .clamp(PANEL_MIN_HEIGHT, max_h);
    let width = width.min(area_w.max(1.0));
    let left = (area_w - width).max(0.0) / 2.0;
    let top = area_h / 6.0;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "pixel coordinates of a display: small, finite and not negative where unsigned"
    )]
    PanelFrame {
        x: origin.0 + (left * scale).round() as i32,
        y: origin.1 + (top * scale).round() as i32,
        width: (width * scale).round() as u32,
        height: (height * scale).round() as u32,
    }
}

/// Places the panel `label` (`width` logical pixels wide, its content `content` high when
/// known) on the display under the pointer, shows it and gives it the keyboard (module docs).
///
/// # Errors
///
/// When the window is gone or refuses.
pub fn show_panel(
    app: &AppHandle,
    label: &str,
    width: f64,
    content: Option<f64>,
) -> tauri::Result<()> {
    let window = app
        .get_webview_window(label)
        .ok_or(tauri::Error::WindowNotFound)?;
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let area = monitor.work_area();
        let frame = panel_frame(
            (area.position.x, area.position.y),
            (area.size.width, area.size.height),
            monitor.scale_factor(),
            width,
            content,
        );
        window.set_size(PhysicalSize::new(frame.width, frame.height))?;
        window.set_position(PhysicalPosition::new(frame.x, frame.y))?;
    }
    window.show()?;
    window.set_focus()
}

/// How long a panel stays shown, out of sight, for its first paint ([`prepaint_panel`]).
const PREPAINT_HOLD: std::time::Duration = std::time::Duration::from_millis(500);

/// Shows the panel `label` once, out of sight and unable to take the keyboard, long enough to
/// paint, then hides it. A web view that was never shown pays for setting up its drawing on
/// its first paint: the first form measured about 400 ms from the request to its paint under
/// Xvfb, the later ones under 25 ms (`docs/test-strategy.md` §8.5). Called once, when the
/// panel's page reports it loaded (`form_ready`), on the panel owner's thread (so that no
/// request shows the panel meanwhile). Blocks for [`PREPAINT_HOLD`].
pub fn prepaint_panel(app: &AppHandle, label: &str) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let _ = window.set_focusable(false);
    let _ = window.set_position(PhysicalPosition::new(-32_000, -32_000));
    let _ = window.show();
    std::thread::sleep(PREPAINT_HOLD);
    let _ = window.hide();
    let _ = window.set_focusable(true);
}

/// Hides the panel `label`, if it exists.
pub fn hide_panel(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
    }
}

/// Shows the library window, creating it if needed. A creation is timed by `timer` until the
/// window's first paint (see `timing`).
///
/// Call it off the main thread: creating a web view inside a menu or event handler can
/// deadlock on Windows. On Linux the window is built on the main thread, where the palette's
/// web view lives, and this waits for it.
///
/// # Errors
///
/// When the window cannot be created or shown.
pub fn open_library(app: &AppHandle, timer: &OpenTimer) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LIBRARY) {
        let _ = window.unminimize();
        window.show()?;
        return window.set_focus();
    }
    timer.start();
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let _ = tx.send(related(library_builder(&handle)).build().map(|_| ()));
        })?;
        rx.recv()
            .map_err(|_| tauri::Error::FailedToReceiveMessage)?
    }
    #[cfg(not(target_os = "linux"))]
    {
        library_builder(app).build()?;
        Ok(())
    }
}

fn library_builder(app: &AppHandle) -> WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    WebviewWindowBuilder::new(app, LIBRARY, page())
        .title("Iamb")
        .inner_size(960.0, 640.0)
        .min_inner_size(640.0, 400.0)
        .center()
}

/// Shows the palette placeholder (task 3.6 adds the focus token and the hand-back).
///
/// # Errors
///
/// When the palette window is gone or cannot be shown.
pub fn show_palette(app: &AppHandle) -> tauri::Result<()> {
    show_panel(app, PALETTE, PALETTE_WIDTH, None)
}

/// Hides a panel, or closes (and so destroys) the library window.
///
/// # Errors
///
/// When the window refuses.
pub fn dismiss(window: &WebviewWindow) -> tauri::Result<()> {
    if window.label() == LIBRARY {
        window.close()
    } else {
        window.hide()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_sits_centred_in_the_upper_third() {
        // A 1440 x 900 work area at 2x, its origin below a 25-point menu bar.
        let f = panel_frame((0, 50), (2880, 1800), 2.0, FORM_WIDTH, Some(300.0));
        assert_eq!(f.width, 880);
        assert_eq!(f.height, 600);
        assert_eq!(f.x, (2880 - 880) / 2);
        assert_eq!(f.y, 50 + 300);
        // The top stays in the upper third.
        assert!(f.y < 50 + 1800 / 3);
    }

    #[test]
    fn a_panel_is_as_high_as_its_content_up_to_seventy_percent() {
        let f = panel_frame((100, 0), (1000, 1000), 1.0, FORM_WIDTH, Some(5000.0));
        assert_eq!(f.height, 700);
        assert_eq!(f.x, 100 + 280);
        let f = panel_frame((0, 0), (1000, 1000), 1.0, FORM_WIDTH, Some(10.0));
        assert_eq!(f.height, 120, "never lower than the minimum");
        let f = panel_frame((0, 0), (1000, 1000), 1.0, FORM_WIDTH, None);
        assert_eq!(f.height, 360, "the default before the content reported");
        let f = panel_frame((0, 0), (1000, 1000), f64::NAN, FORM_WIDTH, Some(f64::NAN));
        assert_eq!((f.width, f.height), (440, 360), "nonsense falls back");
    }

    #[test]
    fn a_panel_never_overflows_a_narrow_display() {
        let f = panel_frame((-1280, 0), (400, 800), 1.0, PALETTE_WIDTH, Some(200.0));
        assert_eq!(f.width, 400);
        assert_eq!(f.x, -1280);
    }
}
