//! The settings and diagnostics commands (task 3.9): `GetSettings`, `PutSetting`, the app
//! exclusions, the diagnostics ring and the dry run through the bridge (`crate::settings`).
//! `apps/desktop/src/library/settings/api.ts` is the typed TypeScript side.
//!
//! None of them logs its arguments or its answer: a setting can hold an app id or a format, and
//! the dry run's text is what the user typed (architecture §8 rule 3).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, State};

use super::Ui;
use crate::contract::CommandError;
use crate::settings::{self, DiagnosticView, ExclusionView, TRAY_KEY, TestView};

type Res<T> = Result<T, CommandError>;

async fn blocking<T: Send + 'static>(
    ui: &Ui,
    f: impl FnOnce(&crate::bridge::Bridge) -> Res<T> + Send + 'static,
) -> Res<T> {
    let bridge = ui.bridge.clone();
    tauri::async_runtime::spawn_blocking(move || f(&bridge))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// Every setting.
#[tauri::command]
pub async fn settings_get(ui: State<'_, Arc<Ui>>) -> Res<BTreeMap<String, String>> {
    blocking(&ui, settings::settings).await
}

/// Sets a setting, or removes it when `value` is absent. The tray icon follows
/// `ui.tray_icon` at once.
#[tauri::command]
pub async fn settings_put(ui: State<'_, Arc<Ui>>, key: String, value: Option<String>) -> Res<()> {
    let tray_key = key == TRAY_KEY;
    let shown = value.as_deref() != Some("off");
    blocking(&ui, move |b| {
        settings::put_setting(b, &key, value.as_deref())
    })
    .await?;
    if tray_key {
        ui.show_tray(shown);
    }
    Ok(())
}

/// The apps where Iamb does not expand.
#[tauri::command]
pub async fn settings_list_exclusions(ui: State<'_, Arc<Ui>>) -> Res<Vec<ExclusionView>> {
    blocking(&ui, settings::exclusions).await
}

/// Adds or removes an exclusion; answers the row written or the rows removed.
#[tauri::command]
pub async fn settings_put_exclusion(
    ui: State<'_, Arc<Ui>>,
    app_id: String,
    display_name: String,
    excluded: bool,
) -> Res<Vec<ExclusionView>> {
    blocking(&ui, move |b| {
        settings::put_exclusion(b, &app_id, &display_name, excluded)
    })
    .await
}

/// The diagnostics ring, newest first.
#[tauri::command]
pub async fn diagnostics_recent(ui: State<'_, Arc<Ui>>) -> Res<Vec<DiagnosticView>> {
    blocking(&ui, settings::diagnostics).await
}

/// What typing `typed` in an app would do (nothing is typed, nothing is stored).
#[tauri::command]
pub async fn diagnostics_test(ui: State<'_, Arc<Ui>>, typed: String) -> Res<TestView> {
    blocking(&ui, move |b| settings::test_abbreviation(b, &typed)).await
}

/// Whether the stored settings were applied since the service last connected.
static APPLIED: AtomicBool = AtomicBool::new(false);

/// Called with each status the bridge reports: when the service connects, the tray icon is
/// shown or hidden as the stored `ui.tray_icon` says.
pub fn connection_changed(app: &AppHandle, connected: bool) {
    if !connected {
        APPLIED.store(false, Ordering::SeqCst);
        return;
    }
    let Some(ui) = app.try_state::<Arc<Ui>>() else {
        return;
    };
    if APPLIED.swap(true, Ordering::SeqCst) {
        return;
    }
    let ui: Arc<Ui> = Arc::clone(&ui);
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(map) = settings::settings(&ui.bridge) {
            ui.show_tray(map.get(TRAY_KEY).map(String::as_str) != Some("off"));
        }
    });
}
