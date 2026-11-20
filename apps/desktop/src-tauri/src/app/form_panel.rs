//! The fill-in form panel in the app (task 3.5): the [`PanelHost`] over the pre-warmed `form`
//! window, the platform layer that gives the focus back, and the form's Tauri commands.
//! The flow itself is `crate::form`; `apps/desktop/src/form/` is the front end.
//!
//! | Command | Does |
//! | --- | --- |
//! | `form_ready` | The panel's page has loaded and listens: it paints once while hidden |
//! | `form_current` | The open form, for a panel whose page loaded after the request |
//! | `form_laid_out` | The front end rendered the form; its height places the panel |
//! | `form_painted` | The panel painted after it was shown (the 150 ms measurement) |
//! | `form_submit` | Insert: hide, hand the focus back, then `FormSubmit` |
//! | `form_cancel` | Cancel: hide, hand the focus back, then `FormCancel` |
//!
//! Each answers only the `form` window: no other window may read or submit a form's values.
//! None logs its arguments or its answer.

use std::collections::BTreeMap;
use std::sync::Arc;

use iamb_platform::FocusInspector;
use tauri::{AppHandle, Emitter, EventTarget, State, WebviewWindow};

use super::{Ui, windows};
use crate::bridge::Bridge;
use crate::contract::CommandError;
use crate::form::{
    FORM_EVENT, FormConfig, FormController, FormEvent, FormOpen, FormService, InstantService,
    PanelHost,
};
use crate::log::Logger;

type Res<T> = Result<T, CommandError>;

/// The `form` window as the flow's [`PanelHost`].
struct Panel {
    app: AppHandle,
}

impl PanelHost for Panel {
    fn send(&self, event: &FormEvent) {
        // To the form panel only: the library and the palette never see a form's values.
        let _ = self.app.emit_to(
            EventTarget::webview_window(windows::FORM),
            FORM_EVENT,
            event,
        );
    }

    fn show(&self, height: Option<f64>) -> Result<(), String> {
        windows::show_panel(&self.app, windows::FORM, windows::FORM_WIDTH, height)
            .map_err(|e| e.to_string())
    }

    fn hide(&self) {
        windows::hide_panel(&self.app, windows::FORM);
    }

    fn prepaint(&self) {
        windows::prepaint_panel(&self.app, windows::FORM);
    }
}

/// The platform layer the UI host gives the focus back with (`restore_focus`) and follows the
/// front application with; none on Linux, which is for development only.
#[cfg_attr(
    any(windows, target_os = "macos"),
    expect(
        clippy::unnecessary_wraps,
        reason = "the same signature returns `None` on Linux"
    )
)]
fn platform_focus() -> Option<Arc<dyn FocusInspector>> {
    #[cfg(windows)]
    {
        Some(Arc::new(iamb_platform_windows::Platform::new()))
    }
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(iamb_platform_macos::Platform::new()))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Starts the form's flow over `bridge`; for the measurement (`measuring`), over a service that
/// answers at once and with no connection to follow (`crate::form::InstantService`).
///
/// # Errors
///
/// When its thread cannot be started.
pub fn start(
    app: &AppHandle,
    bridge: &Bridge,
    measuring: bool,
    log: &Arc<Logger>,
) -> std::io::Result<FormController> {
    let host = Arc::new(Panel { app: app.clone() });
    let service: Arc<dyn FormService> = if measuring {
        Arc::new(InstantService)
    } else {
        Arc::new(bridge.clone())
    };
    let mut cfg = FormConfig::new(service, host, platform_focus(), Arc::clone(log));
    cfg.follows_connection = !measuring;
    FormController::start(cfg)
}

fn only_the_form(window: &WebviewWindow) -> Res<()> {
    if window.label() == windows::FORM {
        Ok(())
    } else {
        Err(CommandError::new(
            "not_permitted",
            "only the fill-in form panel may do this",
        ))
    }
}

async fn blocking<T: Send + 'static>(
    ui: &Arc<Ui>,
    f: impl FnOnce(&FormController) -> Res<T> + Send + 'static,
) -> Res<T> {
    let ui = Arc::clone(ui);
    tauri::async_runtime::spawn_blocking(move || f(&ui.form))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// The panel's page has loaded and listens for forms: the panel paints once while hidden
/// (`windows::prepaint_panel`), so that the first form does not pay for the first paint.
#[tauri::command]
pub async fn form_ready(ui: State<'_, Arc<Ui>>, window: WebviewWindow) -> Res<()> {
    only_the_form(&window)?;
    ui.form.prepaint();
    Ok(())
}

/// The open form, if any.
#[tauri::command]
pub async fn form_current(ui: State<'_, Arc<Ui>>, window: WebviewWindow) -> Res<Option<FormOpen>> {
    only_the_form(&window)?;
    Ok(ui.form.current())
}

/// The front end rendered `session`'s form; its content is `height` CSS pixels high.
#[tauri::command]
pub async fn form_laid_out(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    session: u64,
    height: f64,
) -> Res<()> {
    only_the_form(&window)?;
    ui.form.laid_out(session, height);
    Ok(())
}

/// The panel of `session` painted at `painted_at_ms` (epoch milliseconds) after it was shown.
#[tauri::command]
pub async fn form_painted(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    session: u64,
    painted_at_ms: f64,
) -> Res<()> {
    only_the_form(&window)?;
    if let Some(t) = ui.form.painted(session, painted_at_ms) {
        ui.log.info(format_args!(
            "form {session}: first paint {:.1} ms after the request arrived (shown after {:.1} \
             ms, reported after {:.1} ms)",
            t.paint_ms, t.shown_ms, t.report_ms
        ));
    }
    Ok(())
}

/// Insert `session` with `values` (field name → value).
#[tauri::command]
pub async fn form_submit(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    session: u64,
    values: BTreeMap<String, String>,
) -> Res<()> {
    only_the_form(&window)?;
    blocking(&ui, move |form| form.submit(session, values)).await
}

/// Cancel `session`.
#[tauri::command]
pub async fn form_cancel(ui: State<'_, Arc<Ui>>, window: WebviewWindow, session: u64) -> Res<()> {
    only_the_form(&window)?;
    blocking(&ui, move |form| form.cancel(session)).await
}
