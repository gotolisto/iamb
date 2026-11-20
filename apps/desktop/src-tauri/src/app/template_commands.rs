//! The template editor's commands (task 3.4): `ParseTemplate` and `RenderPreview` through the
//! bridge (`crate::template`). `apps/desktop/src/library/template/api.ts` is the typed
//! TypeScript side.
//!
//! Neither command logs: the source and the test values are library content and user input
//! (architecture §8 rule 3).

use std::collections::BTreeMap;
use std::sync::Arc;

use tauri::State;

use super::Ui;
use crate::contract::CommandError;
use crate::template::{self, ParsedView, PreviewView};

type Res<T> = Result<T, CommandError>;

/// The tokens of `source` and its first error, for the chips.
#[tauri::command]
pub async fn template_parse(ui: State<'_, Arc<Ui>>, source: String) -> Res<ParsedView> {
    let bridge = ui.bridge.clone();
    tauri::async_runtime::spawn_blocking(move || template::parse(&bridge, source))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// What `source` would type with `values` (absent: none), for the preview.
#[tauri::command]
pub async fn template_preview(
    ui: State<'_, Arc<Ui>>,
    source: String,
    values: Option<BTreeMap<String, String>>,
) -> Res<PreviewView> {
    let bridge = ui.bridge.clone();
    let values = values.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || template::preview(&bridge, source, values))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}
