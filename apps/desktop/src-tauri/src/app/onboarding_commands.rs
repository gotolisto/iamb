//! The onboarding window and its commands (task 3.7, `crate::onboarding`).
//!
//! One window, label `onboarding`, 560 × 600 logical pixels, created on demand and destroyed
//! when closed (closing it at any step leaves Iamb running). It opens:
//!
//! - at start, once the service answers, while `ui.onboarding_done` is not set;
//! - from the menu's "Set up Iamb…" (offered while setup is unfinished), at step 1;
//! - from the menu's "Fix permissions…" (offered once setup is done while a permission is
//!   missing: a thread asks the service every 30 s), at the permissions step alone
//!   (`?mode=fix`).
//!
//! Every command answers the onboarding window only. The test field is declared for the window
//! that calls (`onboarding_test_field`) and withdrawn when the step is left, when the end is
//! reached and when the window closes, whichever comes first.
//! `apps/desktop/src/onboarding/api.ts` is the typed TypeScript side.
//!
//! No command logs its arguments or its answers (architecture §8 rule 3): the front end keeps
//! what is typed in the test field to itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tauri::menu::{Menu, MenuId, MenuItem};
use tauri::{
    AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry,
};

use super::{Ui, windows};
use crate::contract::CommandError;
use crate::onboarding::{self, FinishView, Host, LoginView, NoSupervisor, PermissionsView};

type Res<T> = Result<T, CommandError>;

/// The onboarding window's label.
pub const ONBOARDING: &str = "onboarding";

/// The window's size, in logical pixels (specification §4.6).
const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 600.0;

/// How often the service is asked about permissions while setup is done (specification §11.2,
/// entry 9, "every 30 s while something is missing"), and how often it is asked at all.
const PERMISSION_POLL: Duration = Duration::from_secs(30);

/// The platform layer the commands use: what names the focused window, the login item, and
/// (until the supervisor has a control pipe) nothing to restart the worker with.
fn platform_host() -> Host {
    #[cfg(windows)]
    {
        let p = Arc::new(iamb_platform_windows::Platform::new());
        Host {
            focus: Some(p.clone()),
            login: Some(p),
            supervisor: Arc::new(NoSupervisor),
            pid: std::process::id(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let p = Arc::new(iamb_platform_macos::Platform::new());
        Host {
            focus: Some(p.clone()),
            login: Some(p),
            supervisor: Arc::new(NoSupervisor),
            pid: std::process::id(),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Host {
            focus: None,
            login: None,
            supervisor: Arc::new(NoSupervisor),
            pid: std::process::id(),
        }
    }
}

fn host() -> &'static Host {
    static HOST: OnceLock<Host> = OnceLock::new();
    HOST.get_or_init(platform_host)
}

fn only_onboarding(window: &WebviewWindow) -> Res<()> {
    if window.label() == ONBOARDING {
        Ok(())
    } else {
        Err(CommandError::new(
            "not_permitted",
            "only the onboarding window may do this",
        ))
    }
}

async fn blocking<T: Send + 'static>(
    ui: &Ui,
    f: impl FnOnce(&crate::bridge::Bridge) -> Res<T> + Send + 'static,
) -> Res<T> {
    let bridge = ui.bridge.clone();
    tauri::async_runtime::spawn_blocking(move || f(&bridge))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

// ---------------------------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------------------------

/// Shows the onboarding window, creating it if needed. `mode` is `setup` (from step 1) or
/// `fix` (the permissions step alone); it only matters when the window is created.
///
/// Call it off the main thread (see `windows::open_library`).
///
/// # Errors
///
/// When the window cannot be created or shown.
pub fn open(app: &AppHandle, mode: &str) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(ONBOARDING) {
        let _ = window.unminimize();
        window.show()?;
        return window.set_focus();
    }
    let page = if mode == "fix" {
        "index.html?mode=fix"
    } else {
        "index.html"
    };
    let window = WebviewWindowBuilder::new(app, ONBOARDING, WebviewUrl::App(page.into()))
        .general_autofill_enabled(false)
        .title("Welcome to Iamb")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .center()
        .build()?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            withdraw_test_field(&handle);
        }
    });
    Ok(())
}

/// Opens the window from a menu handler or a callback: on the blocking pool.
pub fn open_later(app: &AppHandle, mode: &'static str) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = open(&app, mode)
            && let Some(ui) = app.try_state::<Arc<Ui>>()
        {
            ui.log
                .warn(format_args!("cannot open the onboarding window: {e}"));
        }
    });
}

/// Withdraws the test field when the window goes (the service may be gone too: then there is
/// nothing to withdraw).
fn withdraw_test_field(app: &AppHandle) {
    let Some(ui) = app.try_state::<Arc<Ui>>() else {
        return;
    };
    let bridge = ui.bridge.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = onboarding::clear_test_field(&bridge);
    });
}

// ---------------------------------------------------------------------------------------------
// Start-up and the menu
// ---------------------------------------------------------------------------------------------

/// Whether the window was already offered at this start.
static OFFERED: AtomicBool = AtomicBool::new(false);
/// Whether the permission poll is running.
static POLLING: AtomicBool = AtomicBool::new(false);

/// Called with each status the bridge reports: when the service first answers, onboarding opens
/// by itself if it was never finished, and the menu's two entries follow what the service says.
pub fn connection_changed(app: &AppHandle, connected: bool) {
    if !connected {
        return;
    }
    let Some(ui) = app.try_state::<Arc<Ui>>() else {
        return;
    };
    let ui: Arc<Ui> = Arc::clone(&ui);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(done) = onboarding::is_done(&ui.bridge) else {
            return;
        };
        show_menu_entries(&ui, Some(onboarding::menu_offers_setup(done)), None);
        if onboarding::should_open_at_start(done, OFFERED.swap(true, Ordering::SeqCst))
            && let Err(e) = open(&app, "setup")
        {
            ui.log
                .warn(format_args!("cannot open the onboarding window: {e}"));
        }
        start_poll(&app);
    });
}

/// The menu id of "Set up Iamb…".
pub const SETUP_ID: &str = "setup";
/// The menu id of "Fix permissions…".
pub const FIX_ID: &str = "fix_permissions";

/// The menu's "Set up Iamb…" (offered while setup is unfinished) and "Fix permissions…" (offered
/// while a permission is missing). The items exist all the time; they are in the menu only while
/// they apply, so a finished, healthy setup shows neither.
pub struct MenuEntries {
    menu: Menu<Wry>,
    /// The item they follow: the separator under the status lines, before "Search snippets…".
    anchor: MenuId,
    setup: MenuItem<Wry>,
    fix: MenuItem<Wry>,
    setup_shown: AtomicBool,
    fix_shown: AtomicBool,
}

impl MenuEntries {
    /// The two items, not yet in `menu`; they go right after the item `anchor`.
    ///
    /// # Errors
    ///
    /// When the items cannot be built.
    pub fn new(app: &AppHandle, menu: Menu<Wry>, anchor: MenuId) -> tauri::Result<Self> {
        Ok(Self {
            menu,
            anchor,
            setup: MenuItem::with_id(app, SETUP_ID, "Set up Iamb…", true, None::<&str>)?,
            fix: MenuItem::with_id(app, FIX_ID, "Fix permissions…", true, None::<&str>)?,
            setup_shown: AtomicBool::new(false),
            fix_shown: AtomicBool::new(false),
        })
    }

    fn follow(&self, shown: &AtomicBool, item: &MenuItem<Wry>, wanted: bool) {
        if shown.swap(wanted, Ordering::SeqCst) == wanted {
            return;
        }
        if wanted {
            super::tray::insert_after(&self.menu, &self.anchor, item);
        } else {
            let _ = self.menu.remove(item);
        }
    }

    /// Offers "Set up Iamb…" or stops offering it.
    pub fn set_setup(&self, wanted: bool) {
        self.follow(&self.setup_shown, &self.setup, wanted);
    }

    /// Offers "Fix permissions…" or stops offering it.
    pub fn set_fix(&self, wanted: bool) {
        self.follow(&self.fix_shown, &self.fix, wanted);
    }
}

/// Sets the menu's "Set up Iamb…" and "Fix permissions…" entries (`None` leaves one as it is);
/// `missing` permissions, while "Fix permissions…" is offered, make the icon say "Needs you".
fn show_menu_entries(ui: &Ui, setup: Option<bool>, fix: Option<(bool, u8)>) {
    if let Some(tray) = ui.tray.get() {
        if let Some(setup) = setup {
            tray.setup.set_setup(setup);
        }
        if let Some((fix, missing)) = fix {
            tray.setup.set_fix(fix);
            tray.set_missing(if fix { missing } else { 0 });
        }
    }
}

/// Every 30 s, while setup is done and the service answers: is a permission missing? The menu
/// offers "Fix permissions…" while one is (a permission can be taken away at any time).
fn start_poll(app: &AppHandle) {
    if POLLING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("onboarding-poll".to_owned())
        .spawn(move || {
            loop {
                std::thread::sleep(PERMISSION_POLL);
                let Some(ui) = app.try_state::<Arc<Ui>>() else {
                    continue;
                };
                let fix = match (
                    onboarding::is_done(&ui.bridge),
                    onboarding::permissions(&ui.bridge),
                ) {
                    (Ok(done), Ok(p)) => (onboarding::menu_offers_fix(done, &p), p.missing_count()),
                    _ => (false, 0),
                };
                show_menu_entries(&ui, None, Some(fix));
            }
        });
    if spawned.is_err() {
        POLLING.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

/// The worker's permissions.
#[tauri::command]
pub async fn onboarding_permissions(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
) -> Res<PermissionsView> {
    only_onboarding(&window)?;
    blocking(&ui, onboarding::permissions).await
}

/// Opens the system settings page of `kind` (`input_monitoring` or `accessibility`).
#[tauri::command]
pub async fn onboarding_open_permission_settings(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    kind: String,
) -> Res<()> {
    only_onboarding(&window)?;
    blocking(&ui, move |b| onboarding::open_permission_settings(b, &kind)).await
}

/// Declares this window as the test field (`on`), or withdraws it.
#[tauri::command]
pub async fn onboarding_test_field(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    on: bool,
) -> Res<()> {
    only_onboarding(&window)?;
    blocking(&ui, move |b| {
        if on {
            host().declare(b)
        } else {
            onboarding::clear_test_field(b)
        }
    })
    .await
}

/// Asks the supervisor to stop and start the worker ("Restart the helper", "Start it again").
#[tauri::command]
pub async fn onboarding_restart_helper(window: WebviewWindow) -> Res<()> {
    only_onboarding(&window)?;
    tauri::async_runtime::spawn_blocking(|| host().supervisor.restart_worker())
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// The last step was reached: withdraws the test field, stores the default search shortcut when
/// none is set and records that onboarding was done. The menu stops offering "Set up Iamb…".
#[tauri::command]
pub async fn onboarding_finish(ui: State<'_, Arc<Ui>>, window: WebviewWindow) -> Res<FinishView> {
    only_onboarding(&window)?;
    let view = blocking(&ui, onboarding::finish).await?;
    show_menu_entries(&ui, Some(false), None);
    Ok(view)
}

/// "Start Iamb when I log in": registers (`Some(true)`) or removes (`Some(false)`) the login
/// item, or only reads it (`None`).
#[tauri::command]
pub async fn onboarding_login_item(window: WebviewWindow, enabled: Option<bool>) -> Res<LoginView> {
    only_onboarding(&window)?;
    tauri::async_runtime::spawn_blocking(move || match enabled {
        Some(on) => host().set_login(on),
        None => host().login_status(),
    })
    .await
    .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// Opens the library window and, unless `close_this` is false, closes this one.
#[tauri::command]
pub async fn onboarding_open_library(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    close_this: bool,
) -> Res<()> {
    only_onboarding(&window)?;
    let app = window.app_handle().clone();
    let ui: Arc<Ui> = Arc::clone(&ui);
    tauri::async_runtime::spawn_blocking(move || {
        windows::open_library(&app, &ui.timer)
            .map_err(|e| CommandError::new("internal", e.to_string()))
    })
    .await
    .map_err(|_| CommandError::new("internal", "the request did not finish"))??;
    if close_this {
        window
            .close()
            .map_err(|e| CommandError::new("internal", e.to_string()))?;
    }
    Ok(())
}

/// Closes (and so destroys) the window.
#[tauri::command]
pub async fn onboarding_close(window: WebviewWindow) -> Res<()> {
    only_onboarding(&window)?;
    window
        .close()
        .map_err(|e| CommandError::new("internal", e.to_string()))
}
