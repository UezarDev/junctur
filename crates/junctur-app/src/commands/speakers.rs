//! Speakers and remembered voices.

use tauri::State;

use super::{blocking, App};
use crate::player::TurnsView;
use crate::voices::VoiceView;

/// Renames speaker `id` in the edit list.
#[tauri::command]
pub async fn rename_speaker(app: State<'_, App>, id: u32, name: String) -> Result<(), String> {
    let e = app.editable()?;
    e.rename_speaker(e.file(), id, &name)
}

/// Sets speaker `id`'s caption colour (`#RRGGBB`).
#[tauri::command]
pub async fn recolor_speaker(app: State<'_, App>, id: u32, color: String) -> Result<(), String> {
    let e = app.editable()?;
    e.recolor_speaker(e.file(), id, &color)
}

/// The voices Junctur remembers.
#[tauri::command]
pub async fn voices() -> Result<Vec<VoiceView>, String> {
    crate::voices::list()
}

/// Remembers `name`'s voice from the runs of transcript words in `runs` (`[first, last]` each)
/// and labels the speakers again; returns a line for the user.
#[tauri::command]
pub async fn enroll_voice(
    app: State<'_, App>,
    runs: Vec<(usize, usize)>,
    name: String,
) -> Result<String, String> {
    let e = app.editable()?;
    blocking(move || e.enroll_voice(e.file(), &runs, &name)).await
}

/// Hears the open recording's speakers again from the audio, cut at the seconds pinned to people,
/// on the first-half lane ([`Analysis::exclusive`]) so never beside another speakers stage; it may
/// run beside a transcription, since `jobs.json` changes are made under one lock.
#[tauri::command]
pub async fn reanalyze_speakers(app: State<'_, App>) -> Result<(), String> {
    let e = app.editable()?;
    let analysis = app.analysis.clone();
    blocking(move || analysis.exclusive(|| e.reanalyze_speakers(e.file()))).await
}

/// Forgets voice `name`, relabelling the open recording.
#[tauri::command]
pub async fn forget_voice(app: State<'_, App>, name: String) -> Result<(), String> {
    // No player (libmpv missing): the voice can still be forgotten. With one, not while the open
    // recording is analysed: the run took the voices as they were and would label it with this one.
    if app.core().is_err() {
        return crate::voices::forget(None, &name).map(|_| ());
    }
    let e = app.editable()?;
    e.forget_voice(e.file(), &name)
}

/// The open recording's speaker turns, for the Speakers lane.
#[tauri::command]
pub async fn turns(app: State<'_, App>) -> Result<TurnsView, String> {
    Ok(app.core()?.turns())
}
