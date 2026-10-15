//! Edits to the kept ranges, undo and redo, and the transcript they are made on.

use tauri::State;

use super::{blocking, App};
use crate::cuts::EditState;
use crate::editor::{Op, Step, TranscriptView};

#[tauri::command]
pub async fn edit_state(app: State<'_, App>) -> Result<EditState, String> {
    app.with(|p| Ok(p.edit_state()))
}

/// The open recording's transcript words, or `None` when it has none yet. Read outside the app's
/// player lock: a 30 h transcript takes a moment to parse the first time.
#[tauri::command]
pub async fn transcript(app: State<'_, App>) -> Result<Option<TranscriptView>, String> {
    let core = app.core()?;
    blocking(move || Ok(core.transcript())).await
}

/// Cuts (`keep` false) or keeps every run of transcript words in `runs` (`[first, last]` each),
/// snapped into the pauses around them, as one undo step.
#[tauri::command]
pub async fn edit_words(
    app: State<'_, App>,
    runs: Vec<(usize, usize)>,
    keep: bool,
) -> Result<(), String> {
    let e = app.editable()?;
    e.edit_words(e.file(), &runs, keep)
}

/// Cuts or keeps source frames `[start, end)` exactly (timeline selections, nudges, restoring a
/// whole cut stretch).
#[tauri::command]
pub async fn edit_frames(
    app: State<'_, App>,
    start: u64,
    end: u64,
    keep: bool,
) -> Result<(), String> {
    let e = app.editable()?;
    e.edit(e.file(), Step::Do(Op::Frames { start, end, keep }))
}

/// Moves a cut's edge (the lanes' drag, review's and the range toolbar's steppers): the frame
/// edit `[start, end)` (`keep`) that moves edge `from` to its other end, and the words beside the
/// edge with it, as one undo step; an edit that moves no edge of the list is the frame edit alone.
#[tauri::command]
pub async fn move_edge(
    app: State<'_, App>,
    start: u64,
    end: u64,
    keep: bool,
    from: u64,
) -> Result<(), String> {
    let e = app.editable()?;
    e.move_edge(e.file(), (start, end, keep), from)
}

#[tauri::command]
pub async fn undo(app: State<'_, App>) -> Result<(), String> {
    let e = app.editable()?;
    e.edit(e.file(), Step::Undo)
}

#[tauri::command]
pub async fn redo(app: State<'_, App>) -> Result<(), String> {
    let e = app.editable()?;
    e.edit(e.file(), Step::Redo)
}
