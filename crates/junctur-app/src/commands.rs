//! The commands the frontend invokes. Thin: each forwards to the player or the project.
//!
//! The player's commands are `async` so Tauri runs them on its async runtime rather than the main
//! thread: an mpv command blocks until mpv takes it, and a dragged seek bar sends many.

use std::path::Path;
use std::sync::{Arc, Mutex};

use junctur_core::edit::Captions;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::analysis::{self, Analysis, Notify, Progress, Reanalysed};
use crate::cuts::EditState;
use crate::editor::{Op, Step, TranscriptView};
use crate::lock::lock;
use crate::player::{Core, Player};
use crate::project::{self, ProjectStatus};
use crate::recent::{self, Recent};
use crate::state::PlayerState;
use crate::voices::VoiceView;

/// The player, or why it could not start (libmpv missing, no GL): the window still opens and
/// says so instead of the app failing to launch.
pub struct App {
    pub player: Mutex<Result<Player, String>>,
    pub analysis: Analysis,
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

    /// [`App::core`] for a command that changes the edit list: refused while the open recording
    /// is being analysed, whose `edit` stage rewrites the list (an edit made meanwhile would only
    /// be backed up and replaced).
    fn editable(&self) -> Result<Arc<Core>, String> {
        let core = self.core()?;
        if core
            .file()
            .is_some_and(|f| self.analysis.busy(Path::new(&f)))
        {
            return Err("analysing this recording: edits wait until it finishes".into());
        }
        Ok(core)
    }
}

/// Sends a run's progress to the page. When it ends, the player first takes what it wrote (the
/// edit list, a new transcript) if that recording is open: the page fetches the transcript again
/// on this event, and must not get the words read before the run.
pub fn notifier(handle: AppHandle) -> Notify {
    Arc::new(move |p: &Progress| {
        if !p.phase.busy() {
            if let Some(core) = handle.try_state::<App>().and_then(|a| a.core().ok()) {
                core.analysed(Path::new(&p.file));
            }
        }
        let _ = handle.emit(analysis::EVENT, p);
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    /// The recording, absolute: the key its analysis progress comes under.
    pub file: String,
    pub project: Option<ProjectStatus>,
    /// The file plays even when its project cannot be opened (e.g. an unwritable cache).
    pub project_error: Option<String>,
    /// Its analysis, when one is waiting, running, or ran in this session.
    pub analysis: Option<Progress>,
    /// Why its analysis is out of date with the settings now, for the page to offer a re-analysis
    /// (never started on its own); empty when it is up to date or still being analysed.
    pub outdated: Vec<String>,
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
/// the kept-ranges form); a project that cannot open still plays the whole file. A recording with
/// no edit list is analysed in the background while it plays (2d); an analysed one is not, but says
/// when the settings moved since.
#[tauri::command]
pub async fn open_file(
    handle: AppHandle,
    app: State<'_, App>,
    path: String,
) -> Result<Opened, String> {
    let file = Path::new(&path);
    let status = project::status(file, None);
    let edit_path = status.as_ref().ok().map(|s| s.edit_path.clone());
    app.with(|p| p.open(&path, edit_path))?;
    recent::add(file);
    let mut analysis = app.analysis.progress(file);
    let mut outdated = Vec::new();
    if let Ok(s) = &status {
        let unanalysed = s.edit.is_none() && s.edit_error.is_none();
        if unanalysed {
            analysis = Some(app.analysis.start(file, notifier(handle))?);
        } else if !app.analysis.busy(file) {
            outdated = analysis::outdated(file);
        }
    }
    let (project, project_error) = match status {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e)),
    };
    Ok(Opened {
        file: std::path::absolute(file)
            .unwrap_or_else(|_| file.to_path_buf())
            .display()
            .to_string(),
        project,
        project_error,
        analysis,
        outdated,
    })
}

/// The recordings opened before, newest first, for the start window.
#[tauri::command]
pub async fn recent() -> Vec<Recent> {
    recent::list()
}

/// Takes `path` off the recent list.
#[tauri::command]
pub async fn forget_recent(path: String) -> Result<(), String> {
    recent::remove(&path)
}

/// `path`'s analysis progress, when it has one in this session.
#[tauri::command]
pub async fn analysis(app: State<'_, App>, path: String) -> Result<Option<Progress>, String> {
    Ok(app.analysis.progress(Path::new(&path)))
}

/// Analyses `path` again with the settings now. Without `replace`, an edit list holding the user's
/// edits starts nothing and says so, for the page to ask; with it, the edits are kept as a backup
/// and replaced by a fresh cut.
#[tauri::command]
pub async fn reanalyse(
    handle: AppHandle,
    app: State<'_, App>,
    path: String,
    replace: bool,
) -> Result<Reanalysed, String> {
    app.analysis
        .reanalyse(Path::new(&path), replace, notifier(handle))
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
    app.editable()?
        .edit(Step::Do(Op::Words { first, last, keep }))
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
    app.editable()?
        .edit(Step::Do(Op::Frames { start, end, keep }))
}

/// Renames speaker `id` in the edit list.
#[tauri::command]
pub async fn rename_speaker(app: State<'_, App>, id: u32, name: String) -> Result<(), String> {
    app.editable()?.rename_speaker(id, &name)
}

/// Sets speaker `id`'s caption colour (`#RRGGBB`).
#[tauri::command]
pub async fn recolor_speaker(app: State<'_, App>, id: u32, color: String) -> Result<(), String> {
    app.editable()?.recolor_speaker(id, &color)
}

/// Sets the caption settings: on/off and how cues are cut.
#[tauri::command]
pub async fn set_captions(app: State<'_, App>, captions: Captions) -> Result<(), String> {
    app.editable()?.set_captions(captions)
}

/// What an export wrote: the combined file's captions and each speaker's file (5b).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub cues: usize,
    pub speakers: Vec<SpeakerFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerFile {
    pub path: String,
    pub name: String,
    pub cues: usize,
}

/// Writes the captions as an SRT to `path`, and one per speaker beside it.
#[tauri::command]
pub async fn export_captions(app: State<'_, App>, path: String) -> Result<Exported, String> {
    let f = app.core()?.export_captions(Path::new(&path))?;
    Ok(Exported {
        cues: f.cues,
        speakers: f
            .speakers
            .into_iter()
            .map(|(p, name, cues)| SpeakerFile {
                path: p.display().to_string(),
                name,
                cues,
            })
            .collect(),
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
    app.editable()?.enroll_voice(first, last, &name)
}

/// Forgets voice `name`, relabelling the open recording.
#[tauri::command]
pub async fn forget_voice(app: State<'_, App>, name: String) -> Result<(), String> {
    match app.editable() {
        Ok(core) => core.forget_voice(&name),
        // No player (libmpv missing): the voice can still be forgotten.
        Err(_) => crate::voices::forget(None, &name).map(|_| ()),
    }
}

#[tauri::command]
pub async fn undo(app: State<'_, App>) -> Result<(), String> {
    app.editable()?.edit(Step::Undo)
}

#[tauri::command]
pub async fn redo(app: State<'_, App>) -> Result<(), String> {
    app.editable()?.edit(Step::Redo)
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
