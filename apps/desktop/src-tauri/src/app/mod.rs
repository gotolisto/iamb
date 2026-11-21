//! The Tauri application: start-up, the managed state, the run loop.
//!
//! Start-up (architecture §3, §10): no window is shown. On macOS the app takes the accessory
//! activation policy (no Dock icon, no menu bar of its own); the form and the palette are
//! created hidden; the tray or menu-bar icon is built; the bridge starts connecting to the
//! service. Closing the last visible window does not end the app: only "Quit Iamb" (or the
//! session ending) does.

mod commands;
mod form_panel;
mod library_commands;
mod measure;
mod measure_form;
mod measure_palette;
mod notice_window;
mod onboarding_commands;
mod palette_panel;
mod settings_commands;
mod template_commands;
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
    /// `--budget-ms MS`: with `--measure-open` or `--measure-form`, exit with status 1 when an
    /// open took longer.
    pub budget_ms: Option<f64>,
    /// `--measure-form RUNS`: time this many fill-in forms from the request to the panel's
    /// first paint, print, exit (`measure_form`).
    pub measure_form_runs: Option<usize>,
    /// `--measure-palette RUNS`: time this many search palettes from the shortcut's arrival to the
    /// panel's first paint, print, exit (`measure_palette`).
    pub measure_palette_runs: Option<usize>,
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
    /// The fill-in form's flow (`crate::form`, `form_panel`).
    pub form: crate::form::FormController,
    /// The search palette's flow, rows and hand-over to the library (`crate::palette`,
    /// `palette_panel`).
    pub palette: palette_panel::PalettePanel,
    /// The notice for a refused pick or insert (`crate::notice`, `notice_window`).
    pub notices: crate::notice::Notices,
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

    /// Shows or hides the tray icon (`ui.tray_icon`, task 3.9), if there is one.
    pub fn show_tray(&self, visible: bool) {
        if let Some(tray) = self.tray.get() {
            tray.set_visible(visible);
        }
    }
}

/// Tells the notices (`crate::notice`) of each explicit request the form or the palette sends.
fn notify_request(app: &AppHandle) -> crate::form::OnRequest {
    let app = app.clone();
    Arc::new(move |asked, snippet| {
        if let Some(ui) = app.try_state::<Arc<Ui>>() {
            ui.notices.expect(asked, snippet);
        }
    })
}

/// Sends the bridge's reports to the tray and the front end.
struct Sink {
    app: AppHandle,
}

impl BridgeSink for Sink {
    fn status(&self, view: &StatusView) {
        if let Some(ui) = self.app.try_state::<Arc<Ui>>() {
            ui.show_status(view);
            let connected = view.connection == crate::contract::ConnectionState::Connected;
            if connected {
                palette_panel::connected(&ui);
            } else {
                ui.form.connection_lost();
                palette_panel::connection_lost(&ui);
            }
            if connected {
                tray::refresh_shortcut(&ui);
            }
            settings_commands::connection_changed(&self.app, connected);
            onboarding_commands::connection_changed(&self.app, connected);
        }
        let _ = self.app.emit(STATUS_EVENT, view);
    }

    fn event(&self, event: &UiEvent) {
        let _ = self.app.emit(UI_EVENT, event);
    }

    fn service_event(&self, event: &iamb_ipc::EventBody) {
        if let Some(ui) = self.app.try_state::<Arc<Ui>>() {
            ui.form.service_event(event);
            palette_panel::service_event(&ui, event);
            ui.notices.service_event(event);
            if matches!(event, iamb_ipc::EventBody::SettingsChanged { .. }) {
                tray::refresh_shortcut(&ui);
            }
        }
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
    let form = form_panel::start(&handle, &bridge, options.measure_form_runs.is_some(), log)?;
    let notices = notice_window::start(&handle, &bridge, log)?;
    let palette = palette_panel::start(
        &handle,
        &bridge,
        dirs.run(),
        options.measure_palette_runs.is_some(),
        log,
    )?;
    let ui = Arc::new(Ui {
        bridge,
        timer: OpenTimer::default(),
        form,
        palette,
        notices,
        log: Arc::clone(log),
        tray: OnceLock::new(),
    });
    match tray::Tray::create(&handle, palette_panel::platform_focus()) {
        Ok(t) => {
            let _ = ui.tray.set(t);
        }
        Err(e) => log.warn(format_args!(
            "no tray icon ({e}); the UI host stays resident without it"
        )),
    }
    ui.show_status(&ui.bridge.status());
    app.manage(Arc::clone(&ui));
    // The service may have answered before the state existed (task 3.7: first-start onboarding).
    onboarding_commands::connection_changed(
        &handle,
        ui.bridge.status().connection == crate::contract::ConnectionState::Connected,
    );
    if let Some(runs) = options.measure_runs {
        measure::spawn(handle, runs, options.budget_ms);
    } else if let Some(runs) = options.measure_form_runs {
        measure_form::spawn(handle, runs, options.budget_ms);
    } else if let Some(runs) = options.measure_palette_runs {
        measure_palette::spawn(handle, runs, options.budget_ms);
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
            commands::app_quit,
            library_commands::library_list_folders,
            library_commands::library_list_tags,
            library_commands::library_list_summaries,
            library_commands::library_get_snippet,
            library_commands::library_save_snippet,
            library_commands::library_delete_snippet,
            library_commands::library_set_enabled,
            library_commands::library_save_folder,
            library_commands::library_delete_folder,
            library_commands::library_save_tag,
            library_commands::library_delete_tag,
            library_commands::library_check_abbreviation,
            library_commands::library_health,
            library_commands::library_reset_usage,
            template_commands::template_parse,
            template_commands::template_preview,
            template_commands::template_add_attachment,
            template_commands::template_try_form,
            template_commands::template_open_link,
            settings_commands::settings_get,
            settings_commands::settings_put,
            settings_commands::settings_list_exclusions,
            settings_commands::settings_put_exclusion,
            settings_commands::diagnostics_recent,
            settings_commands::diagnostics_test,
            form_panel::form_ready,
            form_panel::form_current,
            form_panel::form_laid_out,
            form_panel::form_painted,
            form_panel::form_submit,
            form_panel::form_cancel,
            form_panel::form_abandon,
            palette_panel::palette_ready,
            palette_panel::palette_current,
            palette_panel::palette_painted,
            palette_panel::palette_search,
            palette_panel::palette_recent,
            palette_panel::palette_preview,
            palette_panel::palette_pick,
            palette_panel::palette_open,
            palette_panel::library_take_request,
            notice_window::notice_current,
            onboarding_commands::onboarding_permissions,
            onboarding_commands::onboarding_open_permission_settings,
            onboarding_commands::onboarding_test_field,
            onboarding_commands::onboarding_restart_helper,
            onboarding_commands::onboarding_finish,
            onboarding_commands::onboarding_login_item,
            onboarding_commands::onboarding_open_library,
            onboarding_commands::onboarding_close,
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
                ui.form.stop();
                ui.palette.controller.stop();
                ui.notices.stop();
                ui.bridge.stop();
            }
            run_log.info(format_args!("ui host exits"));
        }
        _ => {}
    });
    Ok(code)
}
