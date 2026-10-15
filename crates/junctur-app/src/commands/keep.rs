//! Smart keep: Laya's suggestions for the open recording, its prompt, the judging run and the
//! user's answers ([`crate::smartkeep`]).

use std::path::Path;

use junctur_core::keep;
use tauri::{AppHandle, Emitter, Manager, State};

use super::{blocking, App};
use crate::smartkeep::{self, Action, KeepView, Run, EVENT};

/// What the Properties section and the Suggestions lane show of `path`.
#[tauri::command]
pub async fn keep_view(app: State<'_, App>, path: String) -> Result<KeepView, String> {
    let run = app.judging.of(Path::new(&path));
    blocking(move || {
        let pairs = keep::pairs_path();
        smartkeep::view(Path::new(&path), None, run, pairs.as_deref())
    })
    .await
}

/// Saves `prompt` as `path`'s own (and as the last one used), without judging.
#[tauri::command]
pub async fn keep_prompt(path: String, prompt: String) -> Result<(), String> {
    blocking(move || smartkeep::set_prompt(Path::new(&path), None, &prompt)).await
}

/// Saves `prompt` as `path`'s and has Laya judge it in the background; the run's progress comes as
/// [`EVENT`]. Refused while the recording is being analysed (the run judges at its end).
#[tauri::command]
pub async fn keep_judge(
    handle: AppHandle,
    app: State<'_, App>,
    path: String,
    prompt: String,
) -> Result<Run, String> {
    let file = Path::new(&path).to_path_buf();
    let editing = app
        .analysis
        .edit(&file)
        .ok_or("analysing this recording: Laya judges at the end of its run")?;
    let (f, p) = (file.clone(), prompt.clone());
    blocking(move || smartkeep::set_prompt(&f, None, &p)).await?;
    let notify = move |r: &Run| {
        let _ = handle.emit(EVENT, r);
    };
    app.judging
        .start(&file, None, prompt, Some(editing), notify)
}

/// Accepts every suggestion (`acceptAll`), or accepts, dismisses or undoes the part from `start` to
/// `end`, on the open recording; accepting edits the list as one undo step.
#[tauri::command]
pub async fn keep_answer(
    handle: AppHandle,
    action: Action,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<(), String> {
    blocking(move || {
        let app = handle.state::<App>();
        let e = app.editable()?;
        let pairs = keep::pairs_path();
        smartkeep::answer(&e, e.file(), None, action, start.zip(end), pairs.as_deref())
    })
    .await
}
