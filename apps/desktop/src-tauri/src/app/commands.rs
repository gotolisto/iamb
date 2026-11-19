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
