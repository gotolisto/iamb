//! The tray icon (Windows) or menu-bar icon (macOS) and its menu (UX specification §4.1;
//! `crate::menu` holds what it shows, independent of Tauri).
//!
//! ```text
//! Iamb is on                       (disabled: the first line, crate::menu::status)
//! 76 snippets                      (disabled: its second line, when there is one)
//! ─────────────
//! Start Iamb again / Set up Iamb… / Fix permissions…
//!                                  (only while they apply, `onboarding_commands::MenuEntries`)
//! Search snippets…        ⌃⇧Space  (the search shortcut, when one is set)
//! Fill in the last form again…     (ReopenLastForm, task 3.5)
//! New snippet…                     (the library window with a new draft)
//! Open library
//! ✓ Pause Iamb                     (disabled while the service is not connected)
//! Never expand in Mail             (the app in front when the menu opened; hidden for Iamb)
//! Why didn't it expand?
//! Settings…               ⌘, / Ctrl+,
//! ─────────────
//! Quit Iamb / Exit Iamb
//! ```
//!
//! The icon has three states (On, Paused, Needs you), drawn from the app's icon at start
//! (`crate::menu::dim`, `crate::menu::add_dot`).
//!
//! The UI host stays resident whether or not the icon can be shown (ADR-0017): a system without
//! a tray (a Linux desktop without a status-notifier host) only loses the menu.
//!
//! "Search snippets…", "Fill in the last form again…" and "Never expand in *App*" are for the
//! window that was in front before the menu opened, which on Windows is no longer in front when
//! they run (the menu takes the foreground): the icon's pointer events note it first
//! (`crate::palette`, "The tray menu takes the foreground"), and the exclusion item is named
//! then, when the pointer reaches the icon. "Quit Iamb" asks the library window first, when it is
//! open, so that unsaved edits are guarded as its close guards them (spec §4.3); the window then
//! ends Iamb (`commands::app_quit`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use iamb_platform::FocusInspector;
use tauri::image::Image;
use tauri::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem,
};
use tauri::tray::{TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use super::onboarding_commands::{self, MenuEntries};
use super::{Ui, palette_panel, windows};
use crate::bridge::Bridge;
use crate::contract::{ConnectionState, StatusView};
use crate::menu::{self, ExclusionTarget, IconState};
use crate::palette::{LibraryRequest, LibraryView};
use crate::settings;

const STATUS: &str = "status";
const DETAIL: &str = "status_detail";
const OPEN_LIBRARY: &str = "open_library";
const SEARCH: &str = "search";
const REOPEN_FORM: &str = "reopen_form";
const NEW_SNIPPET: &str = "new_snippet";
const PAUSE: &str = "pause";
const EXCLUDE: &str = "exclude_app";
const WHY: &str = "why";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

/// The event that tells the library window "Quit Iamb" was chosen (`src/library/LibraryView.tsx`).
pub const QUIT_REQUESTED: &str = "iamb:quit-requested";
const SETUP: &str = onboarding_commands::SETUP_ID;
const FIX_PERMISSIONS: &str = onboarding_commands::FIX_ID;
const START_AGAIN: &str = onboarding_commands::START_ID;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Puts `item` into `menu` right after the item whose id is `anchor` (at the end when the anchor
/// is not there).
pub fn insert_after(menu: &Menu<Wry>, anchor: &MenuId, item: &dyn IsMenuItem<Wry>) {
    let at = menu
        .items()
        .ok()
        .and_then(|items| items.iter().position(|i| i.id() == anchor))
        .map_or(usize::MAX, |i| i + 1);
    let _ = if at == usize::MAX {
        menu.append(item)
    } else {
        menu.insert(item, at)
    };
}

/// An item that is in the menu only while it applies.
struct Optional {
    item: MenuItem<Wry>,
    anchor: MenuId,
    shown: AtomicBool,
}

impl Optional {
    fn new(item: MenuItem<Wry>, anchor: &str) -> Self {
        Self {
            item,
            anchor: MenuId::new(anchor),
            shown: AtomicBool::new(false),
        }
    }

    fn follow(&self, menu: &Menu<Wry>, wanted: bool) {
        if self.shown.swap(wanted, Ordering::SeqCst) == wanted {
            return;
        }
        if wanted {
            insert_after(menu, &self.anchor, &self.item);
        } else {
            let _ = menu.remove(&self.item);
        }
    }
}

/// The three icons, drawn from the app's icon.
struct Icons {
    on: Image<'static>,
    paused: Image<'static>,
    needs_you: Image<'static>,
}

impl Icons {
    fn from(base: &Image<'_>) -> Self {
        let (w, h) = (base.width(), base.height());
        let mut paused = base.rgba().to_vec();
        menu::dim(&mut paused);
        let mut needs_you = base.rgba().to_vec();
        menu::add_dot(&mut needs_you, w, h);
        Self {
            on: Image::new_owned(base.rgba().to_vec(), w, h),
            paused: Image::new_owned(paused, w, h),
            needs_you: Image::new_owned(needs_you, w, h),
        }
    }

    fn get(&self, state: IconState) -> Image<'static> {
        match state {
            IconState::On => self.on.clone(),
            IconState::Paused => self.paused.clone(),
            IconState::NeedsYou => self.needs_you.clone(),
        }
    }
}

/// The menu items that follow the bridge's state, and the icon (kept alive with them).
pub struct Tray {
    menu: Menu<Wry>,
    status: MenuItem<Wry>,
    detail: Optional,
    search: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    exclusion: Optional,
    target: Mutex<Option<ExclusionTarget>>,
    icon: TrayIcon<Wry>,
    icons: Option<Icons>,
    shown_icon: Mutex<Option<IconState>>,
    /// The last status and the number of missing permissions.
    last: Mutex<(StatusView, u8)>,
    /// Names the app in front for the exclusion item; none on Linux (development only).
    focus: Option<Arc<dyn FocusInspector>>,
    /// "Set up Iamb…" and "Fix permissions…", added and removed as they apply (task 3.7).
    pub setup: MenuEntries,
}

impl Tray {
    /// Builds the icon and its menu.
    ///
    /// # Errors
    ///
    /// When the system has no tray or the menu cannot be built.
    pub fn create(app: &AppHandle, focus: Option<Arc<dyn FocusInspector>>) -> tauri::Result<Self> {
        let none = None::<&str>;
        let status = MenuItem::with_id(app, STATUS, "Connecting…", false, none)?;
        let detail = MenuItem::with_id(app, DETAIL, "", false, none)?;
        let separator = PredefinedMenuItem::separator(app)?;
        let search = MenuItem::with_id(app, SEARCH, "Search snippets…", true, none)?;
        let reopen =
            MenuItem::with_id(app, REOPEN_FORM, "Fill in the last form again…", true, none)?;
        let new = MenuItem::with_id(app, NEW_SNIPPET, "New snippet…", true, none)?;
        let open = MenuItem::with_id(app, OPEN_LIBRARY, "Open library", true, none)?;
        let pause = CheckMenuItem::with_id(app, PAUSE, "Pause Iamb", false, false, none)?;
        let exclusion = MenuItem::with_id(app, EXCLUDE, "Never expand in this app", true, none)?;
        let why = MenuItem::with_id(app, WHY, "Why didn’t it expand?", true, none)?;
        let settings = MenuItem::with_id(app, SETTINGS, "Settings…", true, Some("CmdOrCtrl+,"))?;
        let quit = MenuItem::with_id(app, QUIT, menu::quit_label(cfg!(windows)), true, none)?;
        let menu = Menu::with_items(
            app,
            &[
                &status,
                &separator,
                &search,
                &reopen,
                &new,
                &open,
                &pause,
                &why,
                &settings,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
        // The separator's id is the system's; the entries go right after it.
        let setup = MenuEntries::new(app, menu.clone(), separator.id().clone())?;
        let mut builder = TrayIconBuilder::with_id("iamb")
            .tooltip("Iamb")
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_tray_icon_event(|icon, event| on_icon_event(icon.app_handle(), &event))
            .on_menu_event(|app, event| on_menu_event(app, &event));
        let icons = app.default_window_icon().map(Icons::from);
        if let Some(icons) = &icons {
            builder = builder.icon(icons.get(IconState::On));
        }
        let icon = builder.build(app)?;
        Ok(Self {
            menu,
            status,
            detail: Optional::new(detail, STATUS),
            search,
            pause,
            exclusion: Optional::new(exclusion, PAUSE),
            target: Mutex::new(None),
            icon,
            icons,
            shown_icon: Mutex::new(Some(IconState::On)),
            last: Mutex::new((
                StatusView {
                    connection: ConnectionState::Connecting,
                    service: None,
                },
                0,
            )),
            focus,
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
        let missing = {
            let mut last = lock(&self.last);
            last.0 = view.clone();
            last.1
        };
        self.render(view, missing);
    }

    /// Shows that `missing` permissions are missing (0: none, or setup is not done yet).
    pub fn set_missing(&self, missing: u8) {
        let view = {
            let mut last = lock(&self.last);
            if last.1 == missing {
                return;
            }
            last.1 = missing;
            last.0.clone()
        };
        self.render(&view, missing);
    }

    fn render(&self, view: &StatusView, missing: u8) {
        let status = menu::status(view, missing);
        let _ = self.status.set_text(status.title);
        if let Some(detail) = &status.detail {
            let _ = self.detail.item.set_text(detail);
        }
        self.detail.follow(&self.menu, status.detail.is_some());
        self.setup
            .set_start(status.needs == Some(menu::NeedsItem::StartAgain));
        let connected = view.connection == ConnectionState::Connected;
        let _ = self.pause.set_enabled(connected);
        let _ = self.pause.set_checked(view.paused());
        if !connected {
            self.exclusion.follow(&self.menu, false);
        }
        let mut shown = lock(&self.shown_icon);
        if *shown != Some(status.icon)
            && let Some(icons) = &self.icons
        {
            let _ = self.icon.set_icon(Some(icons.get(status.icon)));
            *shown = Some(status.icon);
        }
    }

    /// Shows the search shortcut beside "Search snippets…" (`None`: no shortcut is set).
    pub fn show_shortcut(&self, chord: Option<&str>) {
        let accelerator = chord.and_then(menu::accelerator);
        if self.search.set_accelerator(accelerator.as_deref()).is_err() {
            // A chord the menu cannot write is left out rather than shown wrong.
            let _ = self.search.set_accelerator(None::<&str>);
        }
    }

    /// Names the exclusion item for the app in front now, or hides it (Iamb itself, an app that
    /// cannot be named, no service). Blocks on one request to the service: call it off the main
    /// thread.
    pub fn refresh_exclusion(&self, bridge: &Bridge) {
        let front = self
            .focus
            .as_deref()
            .and_then(|f| f.focused_app().ok())
            .filter(|a| a.pid != std::process::id());
        let target = front.and_then(|app| {
            let excluded = settings::exclusions(bridge).ok()?;
            menu::exclusion_item(
                Some((app.app.0.as_str(), app.own)),
                excluded.iter().map(|e| e.app_id.as_str()),
            )
        });
        self.show_exclusion(target);
    }

    fn show_exclusion(&self, target: Option<ExclusionTarget>) {
        if let Some(t) = &target {
            let _ = self.exclusion.item.set_text(t.label());
        }
        self.exclusion.follow(&self.menu, target.is_some());
        *lock(&self.target) = target;
    }

    /// "Never expand in *App*" / "Expand in *App* again" was chosen: adds or removes the
    /// exclusion. Blocks on the service: call it off the main thread.
    ///
    /// # Errors
    ///
    /// The service's error code.
    pub fn toggle_exclusion(&self, bridge: &Bridge) -> Result<(), String> {
        let Some(target) = lock(&self.target).clone() else {
            return Ok(());
        };
        settings::put_exclusion(bridge, &target.app_id, &target.name, !target.excluded)
            .map_err(|e| e.code)?;
        self.show_exclusion(Some(ExclusionTarget {
            excluded: !target.excluded,
            ..target
        }));
        Ok(())
    }
}

/// Reads the search shortcut and shows it in the menu (at connection, and when the settings
/// change). Never blocks.
pub fn refresh_shortcut(ui: &Arc<Ui>) {
    let ui = Arc::clone(ui);
    tauri::async_runtime::spawn_blocking(move || {
        if let (Some(tray), Ok(values)) = (ui.tray.get(), settings::settings(&ui.bridge)) {
            tray.show_shortcut(values.get("palette_hotkey").map(String::as_str));
        }
    });
}

/// The pointer over the icon, before its menu opens: on Windows the menu takes the
/// foreground, so the window the user was in is noted now (`crate::palette`, "The tray menu
/// takes the foreground"), and the exclusion item is named for its app.
fn on_icon_event(app: &AppHandle, event: &TrayIconEvent) {
    let entered = match event {
        TrayIconEvent::Enter { .. } => true,
        TrayIconEvent::Move { .. } => false,
        _ => return,
    };
    let Some(ui) = app.try_state::<Arc<Ui>>() else {
        return;
    };
    ui.palette.controller.tray_pointer(entered);
    if entered && ui.bridge.status().connection == ConnectionState::Connected {
        let ui: Arc<Ui> = Arc::clone(&ui);
        tauri::async_runtime::spawn_blocking(move || {
            if let Some(tray) = ui.tray.get() {
                tray.refresh_exclusion(&ui.bridge);
            }
        });
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
        NEW_SNIPPET => palette_panel::to_library(
            app,
            LibraryRequest::New {
                label: String::new(),
            },
        ),
        WHY => palette_panel::to_library(
            app,
            LibraryRequest::View {
                view: LibraryView::Diagnostics,
            },
        ),
        SETTINGS => palette_panel::to_library(
            app,
            LibraryRequest::View {
                view: LibraryView::Settings,
            },
        ),
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
        EXCLUDE => {
            tauri::async_runtime::spawn_blocking(move || {
                if let Some(tray) = ui.tray.get()
                    && let Err(code) = tray.toggle_exclusion(&ui.bridge)
                {
                    ui.log
                        .warn(format_args!("put_exclusion from the tray failed: {code}"));
                }
            });
        }
        SETUP => onboarding_commands::open_later(app, "setup"),
        FIX_PERMISSIONS => onboarding_commands::open_later(app, "fix"),
        START_AGAIN => {
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = onboarding_commands::restart_service() {
                    ui.log.warn(format_args!(
                        "cannot start the service again: {}",
                        e.message
                    ));
                }
            });
        }
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
