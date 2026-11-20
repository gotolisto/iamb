//! The tray icon (Windows) or menu-bar icon (macOS) and its menu.
//!
//! ```text
//! Service: running                 (disabled; the bridge's state)
//! ─────────────
//! Open library
//! Search snippets…
//! Fill in the last form again…     (ReopenLastForm, task 3.5)
//! ✓ Pause expansion                (disabled while the service is not connected)
//! ─────────────
//! Quit Iamb
//! ```
//!
//! The UI host stays resident whether or not the icon can be shown (ADR-0017): a system without
//! a tray (a Linux desktop without a status-notifier host) only loses the menu.
//!
//! "Search snippets…" and "Fill in the last form again…" are for the window that was in front
//! before the menu opened, which on Windows is no longer in front when they run (the menu takes
//! the foreground): the icon's pointer events note it first (`crate::palette`, "The tray menu
//! takes the foreground"). "Quit Iamb" asks the library window first, when it is open, so that
//! unsaved edits are guarded as its close guards them (spec §4.3); the window then ends Iamb
//! (`commands::app_quit`).

use std::sync::Arc;

use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use super::onboarding_commands::{self, MenuEntries};
use super::{Ui, windows};
use crate::contract::{ConnectionState, StatusView};

const STATUS: &str = "status";
const OPEN_LIBRARY: &str = "open_library";
const SEARCH: &str = "search";
const REOPEN_FORM: &str = "reopen_form";
const PAUSE: &str = "pause";
const QUIT: &str = "quit";

/// The event that tells the library window "Quit Iamb" was chosen (`src/library/LibraryView.tsx`).
pub const QUIT_REQUESTED: &str = "iamb:quit-requested";
const SETUP: &str = onboarding_commands::SETUP_ID;
const FIX_PERMISSIONS: &str = onboarding_commands::FIX_ID;

/// The menu items that follow the bridge's state, and the icon (kept alive with them).
pub struct Tray {
    status: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    icon: TrayIcon<Wry>,
    /// "Set up Iamb…" and "Fix permissions…", added and removed as they apply (task 3.7).
    pub setup: MenuEntries,
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
        let search = MenuItem::with_id(app, SEARCH, "Search snippets…", true, None::<&str>)?;
        let reopen = MenuItem::with_id(
            app,
            REOPEN_FORM,
            "Fill in the last form again…",
            true,
            None::<&str>,
        )?;
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
                &reopen,
                &pause,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
        let setup = MenuEntries::new(app, menu.clone())?;
        let mut builder = TrayIconBuilder::with_id("iamb")
            .tooltip("Iamb")
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_tray_icon_event(|icon, event| on_icon_event(icon.app_handle(), &event))
            .on_menu_event(|app, event| on_menu_event(app, &event));
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
        let icon = builder.build(app)?;
        Ok(Self {
            status,
            pause,
            icon,
            setup,
        })
    }

    /// Shows or hides the icon (Settings → General). The menu is still reachable from the
    /// library window's status line while it is hidden.
    pub fn set_visible(&self, visible: bool) {
        let _ = self.icon.set_visible(visible);
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

/// The pointer over the icon, before its menu opens: on Windows the menu takes the
/// foreground, so the window the user was in is noted now (`crate::palette`, "The tray menu
/// takes the foreground").
fn on_icon_event(app: &AppHandle, event: &TrayIconEvent) {
    let entered = match event {
        TrayIconEvent::Enter { .. } => true,
        TrayIconEvent::Move { .. } => false,
        _ => return,
    };
    if let Some(ui) = app.try_state::<Arc<Ui>>() {
        ui.palette.controller.tray_pointer(entered);
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
            // The palette for the window that was in front when the menu opened
            // (`PaletteController::open_from_tray`); the flow does the rest.
            ui.palette.controller.open_from_tray();
        }
        REOPEN_FORM => {
            // `ReopenLastForm` runs against the window that has the focus when it arrives
            // (architecture §4.5), so the focus goes back to the window that was in front when
            // the menu opened first; the service answers with a new `FormRequested`, or records
            // `no_last_form` when nothing was submitted in the last 30 minutes.
            let target = ui.palette.controller.tray_target();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = ui.form.reopen_last_for(target) {
                    ui.log.warn(format_args!(
                        "reopen_last_form from the tray failed: {}",
                        e.code
                    ));
                }
            });
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
        SETUP => onboarding_commands::open_later(app, "setup"),
        FIX_PERMISSIONS => onboarding_commands::open_later(app, "fix"),
        QUIT => match app.get_webview_window(windows::LIBRARY) {
            // The library window asks about unsaved edits first, as its close does (spec §4.3),
            // and then ends Iamb (`commands::app_quit`).
            Some(library) => {
                if library.emit(QUIT_REQUESTED, ()).is_err() {
                    app.exit(0);
                }
            }
            None => app.exit(0),
        },
        _ => {}
    }
}
