//! Opening and closing a recording, the recent list, and its analysis and transcription runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use junctur_core::pipeline::{StageName, StageState};
use junctur_core::transcribe;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use super::{blocking, notifier, App};
use crate::analysis::{self, Progress, Transcription};
use crate::project::{self, ProjectStatus, RecordingInfo};
use crate::recent::{self, Recent};
use crate::waves::{self, WavesState};

/// The silent parts' labels of the recording at `path` (`labels.json`), for the Labels lane; `None`
/// when it has none or its last run had them off.
#[tauri::command]
pub async fn labels(path: String) -> Result<Option<junctur_core::labels::Labels>, String> {
    blocking(move || project::labels(Path::new(&path), None)).await
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

/// Opens the project first, so the player knows the edit list before it loads (and can start in
/// the kept-ranges form); a project that cannot open still plays the whole file. A recording with
/// no edit list is analysed in the background while it plays; an analysed one says when the
/// settings moved since.
#[tauri::command]
pub async fn open_file(handle: AppHandle, path: String) -> Result<Opened, String> {
    blocking(move || open(&handle, &handle.state::<App>(), path)).await
}

/// [`open_file`]'s work: the project's status, the player's load and the waveforms' start.
fn open(handle: &AppHandle, app: &App, path: String) -> Result<Opened, String> {
    let file = Path::new(&path);
    let status = project::status(file, None);
    let edit_path = status.as_ref().ok().map(|s| s.edit_path.clone());
    app.with(|p| p.open(&path, edit_path))?;
    recent::add(file);
    let absolute = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    let dir = status.as_ref().ok().map(|s| PathBuf::from(&s.dir));
    let h = handle.clone();
    app.waves.open(
        &absolute,
        dir,
        Arc::new(move |s: &WavesState| {
            let _ = h.emit(waves::EVENT, s);
        }),
    );
    // Closed while this was opening: `close_file` closes the player first and the waveforms
    // after, so either it stopped this read or the player no longer has the file and it stops here.
    if app.core()?.file().as_deref() != Some(path.as_str()) {
        app.waves.close_for(&absolute);
    }
    let mut analysis = app.analysis.progress(file);
    let mut outdated = Vec::new();
    if let Ok(s) = &status {
        if needs_run(s, app.analysis.busy(file)) {
            // The file plays even when its analysis cannot start; the panel says why.
            analysis = Some(
                app.analysis
                    .start(file, notifier(handle.clone()))
                    .unwrap_or_else(|e| app.analysis.failed(file, e)),
            );
        } else if !app.analysis.busy(file) {
            outdated = analysis::outdated(file);
        }
    }
    // Opened while its run is past its edit list: the words so far, not an older transcript.
    // Asked again after, so a run that ended meanwhile has the last word.
    if let Some(t) = analysis.as_ref().and_then(Progress::words_through) {
        let core = app.core()?;
        core.transcribed(&absolute, Some(t));
        if let Some(now) = app.analysis.progress(file).filter(|p| !p.phase.busy()) {
            core.transcribed(&absolute, now.words_through());
        }
    }
    Ok(Opened::new(file, status, analysis, outdated))
}

/// Whether opening a recording with this project starts a run of it, `busy` being whether one of
/// this app's own is on. A recording that needs one is never offered a re-analysis (its out-of-date
/// list stays empty), whether its run started or could not.
fn needs_run(s: &ProjectStatus, busy: bool) -> bool {
    // No list, or one that does not parse left by a run that never finished its `edit` stage
    // (a list the user broke by hand after a finished run is theirs to fix, not to replace).
    let finished = s
        .stages
        .iter()
        .any(|st| st.name == StageName::Edit.name() && st.state == StageState::Done);
    let unanalysed = s.edit.is_none() && (s.edit_error.is_none() || !finished);
    // The list comes before the transcript: a run whose transcription never finished is
    // resumed, reusing finished stages and chunks and keeping the user's edits.
    let untranscribed = s
        .stages
        .iter()
        .any(|st| st.name == StageName::Transcribe.name() && st.state != StageState::Done);
    // Not while another process transcribes it (`junctur analyze`): the stage reads `running`
    // then too, and a second Whisper on its partial would only be refused. A run of this app's
    // own is returned by `start` as it is.
    let elsewhere = || transcribe::transcribing_elsewhere(Path::new(&s.dir));
    unanalysed || (untranscribed && (busy || !elsewhere()))
}

impl Opened {
    fn new(
        file: &Path,
        status: Result<ProjectStatus, String>,
        analysis: Option<Progress>,
        outdated: Vec<String>,
    ) -> Opened {
        let (project, project_error) = match status {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        Opened {
            file: std::path::absolute(file)
                .unwrap_or_else(|_| file.to_path_buf())
                .display()
                .to_string(),
            project,
            project_error,
            analysis,
            outdated,
        }
    }
}

/// The recording the player has open, as [`open_file`] returned it, for a reloaded page. Nothing
/// starts and the player is not told the words again: the run's events have kept it current.
#[tauri::command]
pub async fn current_recording(app: State<'_, App>) -> Result<Option<Opened>, String> {
    let Some(path) = app.core()?.file() else {
        return Ok(None);
    };
    let file = Path::new(&path);
    let status = project::status(file, None);
    let analysis = app.analysis.progress(file);
    let busy = app.analysis.busy(file);
    let outdated = match &status {
        Ok(s) if !busy && !needs_run(s, false) => analysis::outdated(file),
        _ => Vec::new(),
    };
    Ok(Some(Opened::new(file, status, analysis, outdated)))
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

/// Who is on the transcription lane, in turn, and whether it is paused.
#[tauri::command]
pub async fn transcription(app: State<'_, App>) -> Result<Transcription, String> {
    Ok(app.analysis.transcription())
}

/// Pauses or resumes the transcription lane after the current chunk. Returns the lane as it is
/// now, also sent to the page as the event.
#[tauri::command]
pub async fn pause_transcription(
    handle: AppHandle,
    app: State<'_, App>,
    paused: bool,
) -> Result<Transcription, String> {
    let sent = std::cell::RefCell::new(None);
    app.analysis.pause(paused, &|t| {
        let _ = handle.emit(analysis::LANE_EVENT, t);
        *sent.borrow_mut() = Some(t.clone());
    });
    sent.into_inner()
        .ok_or_else(|| "the transcription lane was not read".to_string())
}

/// A recent recording's length and where its analysis stands (the start window). Off the
/// async runtime: it opens the project and may start ffprobe.
#[tauri::command]
pub async fn recent_info(path: String) -> Result<RecordingInfo, String> {
    blocking(move || project::info(Path::new(&path), None)).await
}

/// A recent recording's still, as JPEG bytes (made once into its project folder).
#[tauri::command]
pub async fn thumbnail(path: String) -> Result<tauri::ipc::Response, String> {
    let bytes = blocking(move || project::thumbnail(Path::new(&path), None)).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Opens the open recording's project folder in the file manager. Takes no path, so the page can
/// open nothing else.
#[tauri::command]
pub async fn reveal(app: State<'_, App>) -> Result<(), String> {
    let file = app.core()?.file().ok_or("no recording is open")?;
    let dir = project::status(Path::new(&file), None)?.dir;
    let opener = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map(drop)
        .map_err(|e| format!("opening {dir}: {e}"))
}

/// Closes the open recording, back to the start window. Its waveform read stops; an analysis of it
/// goes on, and reopening the recording shows where it is.
#[tauri::command]
pub async fn close_file(app: State<'_, App>) -> Result<(), String> {
    // The player first: an open on its way checks it after starting its waveforms (`open_file`).
    let closed = app.core()?.close();
    app.waves.close();
    closed
}

/// Why `path`'s analysis is out of date with the settings now, asked again after
/// Settings saved a model.
#[tauri::command]
pub async fn outdated(app: State<'_, App>, path: String) -> Result<Vec<String>, String> {
    let file = Path::new(&path);
    if app.analysis.busy(file) {
        return Ok(Vec::new());
    }
    Ok(analysis::outdated(file))
}
