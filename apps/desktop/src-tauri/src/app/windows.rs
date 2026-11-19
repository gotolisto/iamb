//! The three windows (architecture §10, ADR-0017).
//!
//! | Label | Created | On close |
//! | --- | --- | --- |
//! | `library` | On demand, by the tray's "Open library" | Destroyed |
//! | `form` | At start, hidden (pre-warmed for the 150 ms target) | Hidden |
//! | `palette` | At start, hidden (pre-warmed for the 100 ms target) | Hidden |
//!
//! All three load the same page; the front end picks its view by the window's label. The form
//! and the palette are plain undecorated windows for now: the non-activating panels on macOS
//! and the foreground grant on Windows come with their content (tasks 3.5 and 3.6), as does the
//! focus hand-back.
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

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::log::Logger;
use crate::timing::OpenTimer;

/// The library window's label.
pub const LIBRARY: &str = "library";
/// The fill-in form's label.
pub const FORM: &str = "form";
/// The search palette's label.
pub const PALETTE: &str = "palette";

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
    let palette = panel(app, PALETTE, 640.0, 420.0).build()?;
    keep_on_close(&palette);
    #[cfg(target_os = "linux")]
    {
        let _ = palette.with_webview(|view| {
            RELATED.with(|related| *related.borrow_mut() = Some(view.inner()));
        });
        // Queued after the task above, so the palette's web view is known by then.
        let handle = app.clone();
        let log = Arc::clone(log);
        app.run_on_main_thread(move || {
            match related(panel(&handle, FORM, 440.0, 360.0)).build() {
                Ok(form) => keep_on_close(&form),
                Err(e) => log.error(format_args!("cannot create the form window: {e}")),
            }
        })?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = log;
        let form = panel(app, FORM, 440.0, 360.0).build()?;
        keep_on_close(&form);
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

/// Hides `window` instead of closing it: the panels must stay created.
fn keep_on_close(window: &WebviewWindow) {
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = handle.hide();
        }
    });
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
    let window = app
        .get_webview_window(PALETTE)
        .ok_or(tauri::Error::WindowNotFound)?;
    window.center()?;
    window.show()?;
    window.set_focus()
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
