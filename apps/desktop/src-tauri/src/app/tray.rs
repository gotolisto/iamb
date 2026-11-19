//! The tray icon (Windows) or menu-bar icon (macOS) and its menu.
//!
//! ```text
//! Service: running                 (disabled; the bridge's state)
//! ─────────────
//! Open library
//! Search
//! ✓ Pause expansion                (disabled while the service is not connected)
//! ─────────────
//! Quit Iamb
//! ```
//!
//! The UI host stays resident whether or not the icon can be shown (ADR-0017): a system without
//! a tray (a Linux desktop without a status-notifier host) only loses the menu.

use std::sync::Arc;

use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};

use super::{Ui, windows};
use crate::contract::{ConnectionState, StatusView};

const STATUS: &str = "status";
const OPEN_LIBRARY: &str = "open_library";
const SEARCH: &str = "search";
const PAUSE: &str = "pause";
const QUIT: &str = "quit";

/// The menu items that follow the bridge's state, and the icon (kept alive with them).
pub struct Tray {
    status: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    _icon: TrayIcon<Wry>,
}

/// The status line of the menu.
#[must_use]
pub fn status_label(view: &StatusView) -> &'static str {
    match (view.connection, view.service.as_ref()) {
        (ConnectionState::Connected, Some(s)) if s.paused => "Service: paused",
        (ConnectionState::Connected, Some(s)) if !s.listener => "Service: not listening",
        (ConnectionState::Connected, _) => "Service: running",
        (ConnectionState::Connecting, _) => "Service: connecting…",
        (ConnectionState::Disconnected, _) => "Service: not running, retrying",
    }
}

impl Tray {
    /// Builds the icon and its menu.
    ///
    /// # Errors
    ///
    /// When the system has no tray or the menu cannot be built.
    pub fn create(app: &AppHandle) -> tauri::Result<Self> {
        let status = MenuItem::with_id(app, STATUS, "Service: connecting…", false, None::<&str>)?;
        let open = MenuItem::with_id(app, OPEN_LIBRARY, "Open library", true, None::<&str>)?;
        let search = MenuItem::with_id(app, SEARCH, "Search", true, None::<&str>)?;
        let pause =
            CheckMenuItem::with_id(app, PAUSE, "Pause expansion", false, false, None::<&str>)?;
        let quit = MenuItem::with_id(app, QUIT, "Quit Iamb", true, None::<&str>)?;
        let menu = Menu::with_items(
            app,
            &[
                &status,
                &PredefinedMenuItem::separator(app)?,
                &open,
                &search,
                &pause,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
        let mut builder = TrayIconBuilder::with_id("iamb")
            .tooltip("Iamb")
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_menu_event(|app, event| on_menu_event(app, &event));
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
        let icon = builder.build(app)?;
        Ok(Self {
            status,
            pause,
            _icon: icon,
        })
    }

    /// Shows the bridge's state.
    pub fn show(&self, view: &StatusView) {
        let _ = self.status.set_text(status_label(view));
        let _ = self
            .pause
            .set_enabled(view.connection == ConnectionState::Connected);
        let _ = self.pause.set_checked(view.paused());
    }
}

fn on_menu_event(app: &AppHandle, event: &MenuEvent) {
    let Some(ui) = app.try_state::<Arc<Ui>>() else {
        return;
    };
    let ui: Arc<Ui> = Arc::clone(&ui);
    match event.id().as_ref() {
        OPEN_LIBRARY => {
            let app = app.clone();
            // Off the menu handler (see `windows::open_library`).
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = windows::open_library(&app, &ui.timer) {
                    ui.log
                        .warn(format_args!("cannot open the library window: {e}"));
                }
            });
        }
        SEARCH => {
            if let Err(e) = windows::show_palette(app) {
                ui.log.warn(format_args!("cannot show the palette: {e}"));
            }
        }
        PAUSE => {
            tauri::async_runtime::spawn_blocking(move || {
                let wanted = !ui.bridge.status().paused();
                if let Err(e) = ui.bridge.set_paused(wanted) {
                    ui.log
                        .warn(format_args!("set_paused from the tray failed: {}", e.code));
                    // Put the check mark back to what the service says.
                    ui.show_status(&ui.bridge.status());
                }
            });
        }
        QUIT => app.exit(0),
        _ => {}
    }
}
