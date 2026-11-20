//! The Tauri commands: the whole surface the front end can call (architecture §6.2: "The React
//! UI never speaks IPC itself"). `apps/desktop/src/ipc.ts` is the typed TypeScript side.
//!
//! Commands that reach the service run the bridge's blocking call on Tauri's blocking pool, so
//! neither the main thread nor the async runtime waits on a socket.

use std::sync::Arc;

use tauri::{State, WebviewWindow};

use super::{Ui, windows};
use crate::bridge::Bridge;
use crate::contract::{CommandError, SnippetDetail, SnippetSummary, StatusView, parse_id};
use crate::timing::PaintMarks;

type Res<T> = Result<T, CommandError>;

/// The windows that may call the library's, Settings' and "Why didn't it expand?"'s commands:
/// the library window, and the onboarding window, which reads the folders and saves its first
/// snippet and the shortcut. The form, the palette and the notice may not (the Phase 3 UI
/// review's finding 8): what they show must not be able to change the library or the settings.
pub const LIBRARY_CALLERS: [&str; 2] = [windows::LIBRARY, super::onboarding_commands::ONBOARDING];

/// `Ok` when the calling `window` is one of `allowed`; `not_permitted` otherwise.
///
/// # Errors
///
/// `not_permitted` for any other window.
pub fn caller(window: &WebviewWindow, allowed: &[&str]) -> Res<()> {
    may_call(window.label(), allowed)
}

/// [`caller`] by the window's label.
///
/// # Errors
///
/// `not_permitted` when `label` is not one of `allowed`.
pub fn may_call(label: &str, allowed: &[&str]) -> Res<()> {
    if allowed.contains(&label) {
        Ok(())
    } else {
        Err(CommandError::new(
            "not_permitted",
            "this window may not do this",
        ))
    }
}

async fn blocking<T: Send + 'static>(
    bridge: &Bridge,
    f: impl FnOnce(&Bridge) -> Res<T> + Send + 'static,
) -> Res<T> {
    let bridge = bridge.clone();
    tauri::async_runtime::spawn_blocking(move || f(&bridge))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// The connection and the service's last status.
#[tauri::command]
pub async fn status(ui: State<'_, Arc<Ui>>) -> Res<StatusView> {
    Ok(ui.bridge.status())
}

/// Asks the service for its status now.
#[tauri::command]
pub async fn refresh_status(ui: State<'_, Arc<Ui>>) -> Res<StatusView> {
    blocking(&ui.bridge, Bridge::refresh_status).await
}

/// Turns global pause on or off.
#[tauri::command]
pub async fn set_paused(ui: State<'_, Arc<Ui>>, paused: bool) -> Res<StatusView> {
    blocking(&ui.bridge, move |b| b.set_paused(paused)).await
}

/// The snippets of a folder, or of all folders (for the library, task 3.3).
#[tauri::command]
pub async fn list_snippets(
    ui: State<'_, Arc<Ui>>,
    folder: Option<String>,
) -> Res<Vec<SnippetSummary>> {
    let folder = folder.as_deref().map(parse_id).transpose()?;
    blocking(&ui.bridge, move |b| b.list_snippets(folder)).await
}

/// One snippet with its template (for the editor, task 3.4).
#[tauri::command]
pub async fn get_snippet(ui: State<'_, Arc<Ui>>, id: String) -> Res<SnippetDetail> {
    let id = parse_id(&id)?;
    blocking(&ui.bridge, move |b| b.get_snippet(id)).await
}

/// The calling window painted for the first time; the marks are epoch milliseconds. Closes
/// the cold-open measurement of the library window (`timing`).
#[tauri::command]
pub async fn report_first_paint(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
    navigation_at_ms: f64,
    script_at_ms: f64,
    painted_at_ms: f64,
) -> Res<()> {
    let marks = PaintMarks {
        navigation_at_ms,
        script_at_ms,
        painted_at_ms,
    };
    if window.label() == windows::LIBRARY
        && let Some(m) = ui.timer.painted(marks)
    {
        ui.log.info(format_args!(
            "library window: first paint {:.1} ms after the open action (navigation {:.1} ms, \
             script {:.1} ms, reported after {:.1} ms)",
            m.paint_ms, m.navigation_ms, m.script_ms, m.report_ms
        ));
    }
    Ok(())
}

/// Hides the calling panel, or closes the library window.
#[tauri::command]
pub async fn dismiss(window: WebviewWindow) -> Res<()> {
    windows::dismiss(&window).map_err(|e| CommandError::new("internal", e.to_string()))
}

/// Ends Iamb: the tray's "Quit Iamb" asked the library window first (`tray`), which asked
/// about unsaved edits and calls this. The library window only.
#[tauri::command]
pub async fn app_quit(app: tauri::AppHandle, window: WebviewWindow) -> Res<()> {
    caller(&window, &[windows::LIBRARY])?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_library_and_onboarding_windows_reach_the_library_and_settings() {
        assert!(may_call(windows::LIBRARY, &LIBRARY_CALLERS).is_ok());
        assert!(may_call("onboarding", &LIBRARY_CALLERS).is_ok());
        for panel in [windows::FORM, windows::PALETTE, windows::NOTICE, "other"] {
            assert_eq!(
                may_call(panel, &LIBRARY_CALLERS).map_err(|e| e.code),
                Err("not_permitted".to_owned()),
                "{panel}"
            );
        }
        assert!(may_call("onboarding", &[windows::LIBRARY]).is_err());
    }
}
