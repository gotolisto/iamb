//! The template editor's commands (task 3.4): `ParseTemplate` and `RenderPreview` through the
//! bridge (`crate::template`). `apps/desktop/src/library/template/api.ts` is the typed
//! TypeScript side.
//!
//! Neither command logs: the source and the test values are library content and user input
//! (architecture §8 rule 3).

use std::collections::BTreeMap;
use std::sync::Arc;

use iamb_core::model::Id;
use tauri::State;

use super::Ui;
use crate::contract::CommandError;
use crate::template::{self, AttachmentView, ParsedView, PreviewView};

type Res<T> = Result<T, CommandError>;

/// The tokens of `source` and its first error, for the chips.
#[tauri::command]
pub async fn template_parse(ui: State<'_, Arc<Ui>>, source: String) -> Res<ParsedView> {
    let bridge = ui.bridge.clone();
    tauri::async_runtime::spawn_blocking(move || template::parse(&bridge, source))
        .await
        .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// What `source` would type with `values` (absent: none), for the preview; `rich` (absent:
/// no) renders it as a formatted snippet.
#[tauri::command]
pub async fn template_preview(
    ui: State<'_, Arc<Ui>>,
    source: String,
    values: Option<BTreeMap<String, String>>,
    rich: Option<bool>,
) -> Res<PreviewView> {
    let bridge = ui.bridge.clone();
    let values = values.unwrap_or_default();
    let rich = rich.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        template::preview_as(&bridge, source, values, rich)
    })
    .await
    .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}

/// Keeps an image for a formatted snippet (task 3.8). The body is the raw bytes; the headers
/// `iamb-folder` (the snippet's folder id) and `iamb-file-name` (URI-encoded) say where and
/// what. Answers the attachment and the Markdown that shows it. Does not log the bytes or the
/// name.
#[tauri::command]
pub async fn template_add_attachment(
    ui: State<'_, Arc<Ui>>,
    request: tauri::ipc::Request<'_>,
) -> Res<AttachmentView> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(CommandError::new(
            "invalid",
            "the image must be sent as bytes",
        ));
    };
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let folder: Id = header("iamb-folder")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| CommandError::new("invalid", "the folder is missing"))?;
    let file_name = header("iamb-file-name")
        .map(|v| template::decode_uri_component(&v))
        .unwrap_or_default();
    let bytes = bytes.clone();
    let bridge = ui.bridge.clone();
    tauri::async_runtime::spawn_blocking(move || {
        template::add_attachment(&bridge, folder, &file_name, &bytes)
    })
    .await
    .map_err(|_| CommandError::new("internal", "the request did not finish"))?
}
