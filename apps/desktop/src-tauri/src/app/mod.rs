//! The Tauri application: start-up, the managed state, the run loop.
//!
//! Start-up (architecture §3, §10): no window is shown. On macOS the app takes the accessory
//! activation policy (no Dock icon, no menu bar of its own); the form and the palette are
//! created hidden; the tray or menu-bar icon is built; the bridge starts connecting to the
//! service. Closing the last visible window does not end the app: only "Quit Iamb" (or the
//! session ending) does.

mod commands;
mod measure;
pub mod tray;
pub mod windows;

use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Emitter, Manager, RunEvent};

use crate::bridge::{Bridge, BridgeConfig, BridgeSink};
use crate::contract::{STATUS_EVENT, StatusView, UI_EVENT, UiEvent};
use crate::log::Logger;
use crate::paths::DataDirs;
use crate::timing::OpenTimer;

/// What the command line asked for.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `--measure-open RUNS`: time this many opens of the library window, print, exit.
    pub measure_runs: Option<usize>,
    /// `--budget-ms MS`: with `--measure-open`, exit with status 1 when an open took longer.
    pub budget_ms: Option<f64>,
    /// `--open-library`: open the library window at start (development, and desktops without
    /// a tray).
    pub open_library: bool,
}

/// The state every command and handler shares.
pub struct Ui {
    /// The bridge to the service.
    pub bridge: Bridge,
    /// The cold-open timer of the library window.
    pub timer: OpenTimer,
    /// The UI host's log.
    pub log: Arc<Logger>,
    tray: OnceLock<tray::Tray>,
}

impl Ui {
    /// Shows `view` in the tray menu, if there is one.
    pub fn show_status(&self, view: &StatusView) {
        if let Some(tray) = self.tray.get() {
            tray.show(view);
        }
    }
}

/// Sends the bridge's reports to the tray and the front end.
struct Sink {
    app: AppHandle,
}

impl BridgeSink for Sink {
    fn status(&self, view: &StatusView) {
        if let Some(ui) = self.app.try_state::<Arc<Ui>>() {
            ui.show_status(view);
        }
        let _ = self.app.emit(STATUS_EVENT, view);
    }

    fn event(&self, event: &UiEvent) {
        let _ = self.app.emit(UI_EVENT, event);
    }
}

fn setup(
    app: &mut tauri::App,
    dirs: &DataDirs,
    log: &Arc<Logger>,
    options: &Options,
) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let handle = app.handle().clone();
    if let Err(e) = windows::prewarm(&handle, log) {
        log.error(format_args!(
            "cannot create the form and palette windows: {e}"
        ));
    }
    let cfg = BridgeConfig::new(
        iamb_ipc::native_endpoint(),
        dirs.service_endpoint(),
        Arc::clone(log),
    );
    let bridge = Bridge::start(
        cfg,
        Arc::new(Sink {
            app: handle.clone(),
        }),
    )?;
    let ui = Arc::new(Ui {
        bridge,
        timer: OpenTimer::default(),
        log: Arc::clone(log),
        tray: OnceLock::new(),
    });
    match tray::Tray::create(&handle) {
        Ok(t) => {
            let _ = ui.tray.set(t);
        }
        Err(e) => log.warn(format_args!(
            "no tray icon ({e}); the UI host stays resident without it"
        )),
    }
    ui.show_status(&ui.bridge.status());
    app.manage(Arc::clone(&ui));
    if let Some(runs) = options.measure_runs {
        measure::spawn(handle, runs, options.budget_ms);
    } else if options.open_library {
        let ui = Arc::clone(&ui);
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = windows::open_library(&handle, &ui.timer) {
                ui.log
                    .warn(format_args!("cannot open the library window: {e}"));
            }
        });
    }
    log.info(format_args!("ui host ready"));
    Ok(())
}

/// Runs the UI host until it is asked to quit; returns the exit status.
///
/// # Errors
///
/// When the application cannot be built (no display, a broken configuration).
pub fn run(dirs: DataDirs, options: &Options, log: &Arc<Logger>) -> tauri::Result<i32> {
    let setup_log = Arc::clone(log);
    let setup_options = options.clone();
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::status,
            commands::refresh_status,
            commands::set_paused,
            commands::list_snippets,
            commands::get_snippet,
            commands::report_first_paint,
            commands::dismiss,
        ])
        .setup(move |app| setup(app, &dirs, &setup_log, &setup_options).map_err(Into::into))
        .build(tauri::generate_context!())?;
    let run_log = Arc::clone(log);
    let code = app.run_return(move |app, event| match event {
        // The windows closing is not a reason to quit (module docs); an explicit exit has a
        // code.
        RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(ui) = app.try_state::<Arc<Ui>>() {
                ui.bridge.stop();
            }
            run_log.info(format_args!("ui host exits"));
        }
        _ => {}
    });
    Ok(code)
}
