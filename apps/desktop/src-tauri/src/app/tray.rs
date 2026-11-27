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
//! then, under the same rules, with a time limit, and a click acts on exactly the app its label
//! named or on nothing (`crate::menu::ExclusionCapture`). "Quit Iamb" asks the library window first, when it is
//! open, so that unsaved edits are guarded as its close guards them (spec §4.3); the window then
//! ends Iamb (`commands::app_quit`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

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
use crate::menu::{self, ExclusionItem, IconState, SharedCapture};
use crate::palette::{LibraryRequest, LibraryView, TRAY_CAPTURE_TTL};
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
    /// What the exclusion item shows and when it was taken (`crate::menu::SharedCapture`).
    capture: SharedCapture,
    icon: TrayIcon<Wry>,
    icons: Option<Icons>,
    shown_icon: Mutex<Option<IconState>>,
    /// The last status and the number of missing permissions.
    last: Mutex<(StatusView, u8)>,
    /// Names the app in front for the exclusion item; none on Linux (development only).
    focus: Option<Arc<dyn FocusInspector>>,
    /// To reach the main thread, where every change to the menu and the icon is made
    /// ([`Tray::on_main`]).
    app: AppHandle,
    /// The main thread (the tray is built there, in `setup`).
    main_thread: std::thread::ThreadId,
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
            capture: SharedCapture::default(),
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
            app: app.clone(),
            main_thread: std::thread::current().id(),
            setup,
        })
    }

    /// Runs `f` on the main thread: at once when called there, otherwise queued, in order
    /// (the beta review's re-verification, N1). The menu's and the icon's calls wait for the
    /// main thread, so no lock the main thread takes may be held across one; doing every change
    /// there, from what the tray holds when it runs, keeps them in order without such a lock.
    fn on_main(&self, f: fn(&Self)) {
        if std::thread::current().id() == self.main_thread {
            f(self);
            return;
        }
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || {
            if let Some(ui) = app.try_state::<Arc<Ui>>()
                && let Some(tray) = ui.tray.get()
            {
                f(tray);
            }
        });
    }

    /// Shows or hides the icon (Settings → General). The menu is still reachable from the
    /// library window's status line while it is hidden.
    pub fn set_visible(&self, visible: bool) {
        let _ = self.icon.set_visible(visible);
    }

    /// Shows the bridge's state.
    pub fn show(&self, view: &StatusView) {
        lock(&self.last).0 = view.clone();
        if view.connection != ConnectionState::Connected {
            self.capture.clear(self);
        }
        self.on_main(Self::render);
    }

    /// Shows that `missing` permissions are missing (0: none, or setup is not done yet).
    pub fn set_missing(&self, missing: u8) {
        {
            let mut last = lock(&self.last);
            if last.1 == missing {
                return;
            }
            last.1 = missing;
        }
        self.on_main(Self::render);
    }

    /// Shows the last status (main thread, [`Tray::on_main`]).
    fn render(&self) {
        let (view, missing) = lock(&self.last).clone();
        let status = menu::status(&view, missing);
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
        let changed = {
            let mut shown = lock(&self.shown_icon);
            let changed = *shown != Some(status.icon);
            *shown = Some(status.icon);
            changed
        };
        if changed && let Some(icons) = &self.icons {
            let _ = self.icon.set_icon(Some(icons.get(status.icon)));
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

    /// The pointer reached the icon (`entered`) or moved over it: a ticket when the exclusion
    /// item's application is to be captured now (`crate::menu::ExclusionCapture::begin`).
    /// Never blocks for long: one lock, which nothing holds across a menu call.
    pub fn begin_exclusion_capture(&self, entered: bool) -> Option<u64> {
        self.capture
            .begin(entered, Instant::now(), TRAY_CAPTURE_TTL)
    }

    /// Names the exclusion item for the app in front now, or hides it (Iamb itself, an app that
    /// cannot be named, the shell's taskbar or notification area, no service), for the capture
    /// `ticket`. Blocks on one request to the service: call it off the main thread.
    pub fn refresh_exclusion(&self, bridge: &Bridge, ticket: u64) {
        let front = self
            .focus
            .as_deref()
            .and_then(|f| f.focus_now().ok())
            .filter(|change| change.app.pid != std::process::id())
            .filter(|change| !shell_tray_window(change.token.window()));
        let target = front.and_then(|change| {
            let excluded = settings::exclusions(bridge).ok()?;
            menu::exclusion_item(
                Some((change.app.app.0.as_str(), change.app.own)),
                excluded.iter().map(|e| e.app_id.as_str()),
            )
        });
        self.capture.finish(ticket, target, Instant::now(), self);
    }

    /// Sets the item's text and whether it is shown from the capture as it is now (main
    /// thread, [`Tray::on_main`]).
    fn show_exclusion(&self) {
        let target = self.capture.shown();
        if let Some(t) = &target {
            let _ = self.exclusion.item.set_text(t.label());
        }
        self.exclusion.follow(&self.menu, target.is_some());
    }

    /// "Never expand in *App*" / "Expand in *App* again" was chosen: adds or removes the
    /// exclusion of exactly the app the item named, or does nothing
    /// (`crate::menu::ExclusionCapture::for_click`). Blocks on the service: call it off the
    /// main thread.
    ///
    /// # Errors
    ///
    /// The refusal's code, or the service's error code.
    pub fn toggle_exclusion(&self, bridge: &Bridge) -> Result<(), String> {
        let target = self
            .capture
            .for_click(Instant::now(), TRAY_CAPTURE_TTL, self)
            .map_err(|refusal| refusal.code().to_owned())?;
        settings::put_exclusion(bridge, &target.app_id, &target.name, !target.excluded)
            .map_err(|e| e.code)?;
        self.capture.toggled(&target.app_id, self);
        Ok(())
    }
}

impl ExclusionItem for Tray {
    fn text(&self) -> Option<String> {
        self.exclusion.item.text().ok()
    }

    fn refresh(&self) {
        self.on_main(Self::show_exclusion);
    }
}

/// True when `window` is the Windows shell's taskbar or notification-area window (the item is
/// then hidden: the shell is in front, not the app the user was in).
fn shell_tray_window(window: u64) -> bool {
    #[cfg(windows)]
    {
        iamb_platform_windows::window_class(window).is_some_and(|c| menu::is_shell_tray_class(&c))
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        false
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
    if ui.bridge.status().connection == ConnectionState::Connected
        && let Some(ticket) = ui
            .tray
            .get()
            .and_then(|t| t.begin_exclusion_capture(entered))
    {
        let ui: Arc<Ui> = Arc::clone(&ui);
        tauri::async_runtime::spawn_blocking(move || {
            if let Some(tray) = ui.tray.get() {
                tray.refresh_exclusion(&ui.bridge, ticket);
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
