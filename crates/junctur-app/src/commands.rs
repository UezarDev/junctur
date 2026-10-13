//! The commands the frontend invokes. Thin: each forwards to the player or the project.
//!
//! The player's commands are `async` so Tauri runs them on its async runtime rather than the main
//! thread: an mpv command blocks until mpv takes it, and a dragged seek bar sends many.

use std::path::Path;
use std::sync::{Arc, Mutex};

use junctur_core::edit::Captions;
use serde::Serialize;
use tauri::State;

use crate::cuts::EditState;
use crate::editor::{Op, Step, TranscriptView};
use crate::lock::lock;
use crate::player::{Core, Player};
use crate::project::{self, ProjectStatus};
use crate::state::PlayerState;
use crate::voices::VoiceView;

/// The player, or why it could not start (libmpv missing, no GL): the window still opens and
/// says so instead of the app failing to launch.
pub struct App {
    pub player: Mutex<Result<Player, String>>,
}

impl App {
    fn with<T>(&self, f: impl FnOnce(&Player) -> Result<T, String>) -> Result<T, String> {
        let guard = lock(&self.player);
        match &*guard {
            Ok(p) => f(p),
            Err(e) => Err(e.clone()),
        }
    }

    /// The player's shared part, for a command that should not hold the app's player lock while
    /// it works (edits save a file; the transcript is parsed on first use).
    fn core(&self) -> Result<Arc<Core>, String> {
        self.with(|p| Ok(p.core()))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub project: Option<ProjectStatus>,
    /// The file plays even when its project cannot be opened (e.g. an unwritable cache).
    pub project_error: Option<String>,
}

/// A file named on the command line (`junctur-app <file>`, or "Open with"), for the page to open
/// once it is ready.
#[tauri::command]
pub fn startup_file() -> Option<String> {
    std::env::args().nth(1)
}

#[tauri::command]
pub async fn player_state(app: State<'_, App>) -> Result<PlayerState, String> {
    app.with(|p| Ok(p.state()))
}

/// Opens the project first, so the player knows the edit list before it loads (and can start in
/// the kept-ranges form); a project that cannot open still plays the whole file.
#[tauri::command]
pub async fn open_file(app: State<'_, App>, path: String) -> Result<Opened, String> {
    let status = project::status(Path::new(&path), None);
    let edit_path = status.as_ref().ok().map(|s| s.edit_path.clone());
    app.with(|p| p.open(&path, edit_path))?;
    Ok(match status {
        Ok(s) => Opened {
            project: Some(s),
            project_error: None,
        },
        Err(e) => Opened {
            project: None,
            project_error: Some(e),
        },
    })
}

/// Plays only the kept ranges (`on`) or the whole file.
#[tauri::command]
pub async fn set_skip(app: State<'_, App>, on: bool) -> Result<(), String> {
    app.with(|p| p.set_skip(on))
}

#[tauri::command]
pub async fn edit_state(app: State<'_, App>) -> Result<EditState, String> {
    app.with(|p| Ok(p.edit_state()))
}

#[tauri::command]
pub async fn toggle_pause(app: State<'_, App>) -> Result<(), String> {
    app.with(|p| p.toggle_pause())
}

/// Returns the source time the seek lands on (a cut lands on the next kept range while skipping).
#[tauri::command]
pub async fn seek(app: State<'_, App>, seconds: f64, exact: bool) -> Result<f64, String> {
    app.with(|p| p.seek(seconds, exact))
}

#[tauri::command]
pub async fn set_audio_track(app: State<'_, App>, id: i64) -> Result<(), String> {
    app.with(|p| p.set_audio_track(id))
}

/// The open recording's transcript words, or `None` when it has none yet. Read outside the app's
/// player lock: a 30 h transcript takes a moment to parse the first time.
#[tauri::command]
pub async fn transcript(app: State<'_, App>) -> Result<Option<TranscriptView>, String> {
    Ok(app.core()?.transcript())
}

/// Cuts (`keep` false) or keeps transcript words `first..=last`, snapped into the pauses around.
#[tauri::command]
pub async fn edit_words(
    app: State<'_, App>,
    first: usize,
    last: usize,
    keep: bool,
) -> Result<(), String> {
    app.core()?.edit(Step::Do(Op::Words { first, last, keep }))
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
    app.core()?.edit(Step::Do(Op::Frames { start, end, keep }))
}

/// Renames speaker `id` in the edit list.
#[tauri::command]
pub async fn rename_speaker(app: State<'_, App>, id: u32, name: String) -> Result<(), String> {
    app.core()?.rename_speaker(id, &name)
}

/// Sets speaker `id`'s caption colour (`#RRGGBB`).
#[tauri::command]
pub async fn recolor_speaker(app: State<'_, App>, id: u32, color: String) -> Result<(), String> {
    app.core()?.recolor_speaker(id, &color)
}

/// Sets the caption settings: on/off and how cues are cut.
#[tauri::command]
pub async fn set_captions(app: State<'_, App>, captions: Captions) -> Result<(), String> {
    app.core()?.set_captions(captions)
}

/// What an export wrote: the file's captions (one file since 5e).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub cues: usize,
    /// Speaker files beside it from an earlier export (5b wrote one per speaker).
    pub stale: Vec<String>,
}

/// Writes the captions as one SRT to `path`, people talking at once stacked in one caption.
#[tauri::command]
pub async fn export_captions(app: State<'_, App>, path: String) -> Result<Exported, String> {
    let f = app.core()?.export_captions(Path::new(&path))?;
    Ok(Exported {
        cues: f.cues,
        stale: f.stale.iter().map(|p| p.display().to_string()).collect(),
    })
}

/// The voices Junctur remembers (5b).
#[tauri::command]
pub async fn voices() -> Result<Vec<VoiceView>, String> {
    crate::voices::list()
}

/// Remembers `name`'s voice from transcript words `first..=last` and labels the speakers again;
/// returns a line for the user.
#[tauri::command]
pub async fn enroll_voice(
    app: State<'_, App>,
    first: usize,
    last: usize,
    name: String,
) -> Result<String, String> {
    app.core()?.enroll_voice(first, last, &name)
}

/// Hears the open recording's speakers again from the audio, cut at the seconds pinned to people
/// (5c, "Analyse speakers again"), and puts them into the list in play. Takes about as long as the
/// speakers stage, so it runs on a blocking thread, not the async runtime's workers.
#[tauri::command]
pub async fn reanalyze_speakers(app: State<'_, App>) -> Result<(), String> {
    let core = app.core()?;
    tauri::async_runtime::spawn_blocking(move || core.reanalyze_speakers())
        .await
        .map_err(|e| e.to_string())?
}

/// Forgets voice `name`, relabelling the open recording.
#[tauri::command]
pub async fn forget_voice(app: State<'_, App>, name: String) -> Result<(), String> {
    match app.core() {
        Ok(core) => core.forget_voice(&name),
        // No player (libmpv missing): the voice can still be forgotten.
        Err(_) => crate::voices::forget(None, &name).map(|_| ()),
    }
}

#[tauri::command]
pub async fn undo(app: State<'_, App>) -> Result<(), String> {
    app.core()?.edit(Step::Undo)
}

#[tauri::command]
pub async fn redo(app: State<'_, App>) -> Result<(), String> {
    app.core()?.edit(Step::Redo)
}

/// The share of the window's width the transcript panel covers, kept free of video.
#[tauri::command]
pub async fn set_video_margin(app: State<'_, App>, right: f64) -> Result<(), String> {
    app.core()?.set_video_margin(right)
}

/// A line from the frontend for the app's log (timings test mode reports).
#[tauri::command]
pub fn ui_log(message: String) {
    crate::testmode::ui_log(message);
}
