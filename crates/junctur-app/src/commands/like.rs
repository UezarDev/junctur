//! "Find all like this" (7b, a prototype): the open recording's search ([`crate::like`]), its
//! settings, the search itself, and what is done with the matches.

use std::path::Path;

use tauri::{AppHandle, Manager};

use super::{blocking, App};
use crate::like::{self, Act, Answer, LikeView, Settings};

/// What the Like this tab and lane show of `path`.
#[tauri::command]
pub async fn like_view(path: String) -> Result<LikeView, String> {
    blocking(move || like::view(Path::new(&path), None)).await
}

/// Saves the page's examples, track, range, slider and padding for `path`.
#[tauri::command]
pub async fn like_set(path: String, settings: Settings) -> Result<LikeView, String> {
    blocking(move || like::set(Path::new(&path), None, settings)).await
}

/// Searches `path` with its saved examples; the first search on a track reads its whole sound.
#[tauri::command]
pub async fn like_search(path: String) -> Result<LikeView, String> {
    blocking(move || like::search(Path::new(&path), None)).await
}

/// Proposes keeping around or cutting every shown match, or sends them to the Moments list.
#[tauri::command]
pub async fn like_act(path: String, act: Act) -> Result<LikeView, String> {
    blocking(move || like::act(Path::new(&path), None, act)).await
}

/// Accepts or discards the proposal, undoes an accepted one, or takes a sent batch back, on the
/// open recording; accepting and undoing edit the list as one undo step.
#[tauri::command]
pub async fn like_answer(
    handle: AppHandle,
    answer: Answer,
    id: Option<u64>,
) -> Result<LikeView, String> {
    blocking(move || {
        let app = handle.state::<App>();
        let e = app.editable()?;
        like::answer(&e, e.file(), None, answer, id)
    })
    .await
}
