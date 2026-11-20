//! The notice window in the app (`crate::notice`): the [`NoticeHost`] over the `notice` window
//! (`windows::show_notice`) and its one command.
//!
//! | Command | Does |
//! | --- | --- |
//! | `notice_current` | The notice on screen, for the notice window's page |
//!
//! It answers the `notice` window only.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, EventTarget, State, WebviewWindow};

use super::{Ui, windows};
use crate::bridge::Bridge;
use crate::contract::CommandError;
use crate::log::Logger;
use crate::notice::{NOTICE_EVENT, Notice, NoticeHost, Notices};

/// The `notice` window as the notices' [`NoticeHost`].
struct Window {
    app: AppHandle,
}

impl NoticeHost for Window {
    fn show(&self, notice: &Notice) -> Result<(), String> {
        // Called on the notices' own thread, never the main one (`windows::show_notice`).
        windows::show_notice(&self.app).map_err(|e| e.to_string())?;
        // A page that loaded just now fetches the notice itself (`notice_current`).
        let _ = self.app.emit_to(
            EventTarget::webview_window(windows::NOTICE),
            NOTICE_EVENT,
            notice.id,
        );
        Ok(())
    }

    fn hide(&self) {
        windows::hide_panel(&self.app, windows::NOTICE);
    }
}

/// Starts the notices over `bridge`.
///
/// # Errors
///
/// When their thread cannot be started.
pub fn start(app: &AppHandle, bridge: &Bridge, log: &Arc<Logger>) -> std::io::Result<Notices> {
    Notices::start(
        Arc::new(bridge.clone()),
        Arc::new(Window { app: app.clone() }),
        Arc::clone(log),
    )
}

/// The notice on screen, if any.
#[tauri::command]
pub async fn notice_current(
    ui: State<'_, Arc<Ui>>,
    window: WebviewWindow,
) -> Result<Option<Notice>, CommandError> {
    if window.label() != windows::NOTICE {
        return Err(CommandError::new(
            "not_permitted",
            "only the notice window may do this",
        ));
    }
    Ok(ui.notices.current())
}
