//! "Ask, scrub, accept" (7c, a prototype): the open recording's requests ([`crate::ask`]), a new
//! one asked over the selection, one asked again with its recipe edited, the answers to their ghosts,
//! and the preview that plays them before they are accepted.

use std::path::Path;

use tauri::{AppHandle, Manager};

use super::{blocking, App};
use crate::ask::{self, Answer, AskView};
use junctur_core::llm::Size;

/// What the Ask tab and lane show of `path`.
#[tauri::command]
pub async fn ask_view(path: String) -> Result<AskView, String> {
    blocking(move || ask::view(Path::new(&path), None)).await
}

/// Reads `request` (by the model of size `model`, else the keywords) and runs it over `range` of
/// `path` (the whole recording when none): a new step in the history, its ghosts waiting.
#[tauri::command]
pub async fn ask_request(
    path: String,
    request: String,
    range: Option<(f64, f64)>,
    model: Option<Size>,
) -> Result<AskView, String> {
    blocking(move || ask::request(Path::new(&path), None, &request, range, model)).await
}

/// Step `id` of `path` asked again with `recipe` as given.
#[tauri::command]
pub async fn ask_rerun(
    path: String,
    id: u64,
    recipe: serde_json::Value,
) -> Result<AskView, String> {
    blocking(move || ask::rerun(Path::new(&path), None, id, recipe)).await
}

/// Accepts, rejects or undoes step `id`'s ghosts (those at `which`, or all) on the open recording;
/// accepting and undoing edit the list as one undo step.
#[tauri::command]
pub async fn ask_answer(
    handle: AppHandle,
    answer: Answer,
    id: u64,
    which: Option<Vec<usize>>,
    applied: Option<u64>,
) -> Result<AskView, String> {
    blocking(move || {
        let app = handle.state::<App>();
        let e = app.editable()?;
        ask::answer(&e, e.file(), None, answer, id, which, applied)
    })
    .await
}

/// Plays the open recording as every waiting ghost would make it, or as its list is again.
#[tauri::command]
pub async fn ask_preview(handle: AppHandle, on: bool) -> Result<(), String> {
    blocking(move || {
        let app = handle.state::<App>();
        let core = app.core()?;
        let file = core.file();
        ask::set_preview(&core, file.as_deref(), None, on)
    })
    .await
}
