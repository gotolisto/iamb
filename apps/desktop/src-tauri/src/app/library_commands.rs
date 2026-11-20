//! The library window's Tauri commands (task 3.3). Each runs one function of `crate::library`
//! on Tauri's blocking pool, like the commands in `commands.rs`; `apps/desktop/src/library/api.ts`
//! is the typed TypeScript side.
//!
//! None of them logs its arguments or its answer: they hold library content.

use std::sync::Arc;

use tauri::State;

use super::Ui;
use crate::contract::CommandError;
use crate::library::{
    self, CheckView, FolderDraft, FoldersView, SavedView, SnippetDraft, SnippetFull, SummaryPage,
    TagDraft, TagView,
};

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

/// The personal profile's folders.
#[tauri::command]
pub async fn library_list_folders(ui: State<'_, Arc<Ui>>) -> Res<FoldersView> {
    blocking(&ui, library::folders).await
}

/// The tags with their counts.
#[tauri::command]
pub async fn library_list_tags(ui: State<'_, Arc<Ui>>) -> Res<Vec<TagView>> {
    blocking(&ui, library::tags).await
}

/// One page of snippet summaries.
#[tauri::command]
pub async fn library_list_summaries(
    ui: State<'_, Arc<Ui>>,
    offset: u32,
    limit: u32,
) -> Res<SummaryPage> {
    blocking(&ui, move |b| library::summaries(b, offset, limit)).await
}

/// One snippet with its tags and options.
#[tauri::command]
pub async fn library_get_snippet(ui: State<'_, Arc<Ui>>, id: String) -> Res<SnippetFull> {
    blocking(&ui, move |b| library::snippet(b, &id)).await
}

/// Creates, replaces or restores (Undo) a snippet.
#[tauri::command]
pub async fn library_save_snippet(ui: State<'_, Arc<Ui>>, draft: SnippetDraft) -> Res<SavedView> {
    blocking(&ui, move |b| library::save_snippet(b, &draft)).await
}

/// Deletes a snippet.
#[tauri::command]
pub async fn library_delete_snippet(ui: State<'_, Arc<Ui>>, id: String) -> Res<()> {
    blocking(&ui, move |b| library::delete_snippet_by_id(b, &id)).await
}

/// Turns a snippet on or off.
#[tauri::command]
pub async fn library_set_enabled(
    ui: State<'_, Arc<Ui>>,
    id: String,
    enabled: bool,
) -> Res<SavedView> {
    blocking(&ui, move |b| library::set_enabled(b, &id, enabled)).await
}

/// Creates or changes a folder.
#[tauri::command]
pub async fn library_save_folder(ui: State<'_, Arc<Ui>>, draft: FolderDraft) -> Res<SavedView> {
    blocking(&ui, move |b| library::save_folder(b, &draft)).await
}

/// Deletes a folder with its snippets.
#[tauri::command]
pub async fn library_delete_folder(ui: State<'_, Arc<Ui>>, id: String) -> Res<()> {
    blocking(&ui, move |b| library::delete_folder_by_id(b, &id)).await
}

/// Creates, renames or recolours a tag.
#[tauri::command]
pub async fn library_save_tag(ui: State<'_, Arc<Ui>>, draft: TagDraft) -> Res<SavedView> {
    blocking(&ui, move |b| library::save_tag(b, &draft)).await
}

/// Deletes a tag everywhere.
#[tauri::command]
pub async fn library_delete_tag(ui: State<'_, Arc<Ui>>, id: String) -> Res<()> {
    blocking(&ui, move |b| library::delete_tag_by_id(b, &id)).await
}

/// What saving an abbreviation would run into.
#[tauri::command]
pub async fn library_check_abbreviation(
    ui: State<'_, Arc<Ui>>,
    abbreviation: String,
    trigger: String,
    snippet: Option<String>,
) -> Res<CheckView> {
    blocking(&ui, move |b| {
        library::check(b, &abbreviation, &trigger, snippet.as_deref())
    })
    .await
}
