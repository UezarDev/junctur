//! The commands the frontend invokes. Thin: each forwards to the player or the project.
//!
//! The player's commands are `async` so Tauri runs them on its async runtime rather than the main
//! thread: an mpv command blocks until mpv takes it, and a dragged seek bar sends many.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use junctur_core::edit::Captions;
use junctur_core::export::srt::Cue;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::analysis::{self, Analysis, Notify, Progress, Reanalysed};
use crate::cuts::EditState;
use crate::editor::{Op, Step, TranscriptView};
use crate::lock::lock;
use crate::mixer::MixerState;
use crate::player::{Core, Player, TurnsView};
use crate::project::{self, ProjectStatus, RecordingInfo};
use crate::recent::{self, Recent};
use crate::settings::{self, Settings, SettingsView};
use crate::state::PlayerState;
use crate::voices::VoiceView;
use crate::waves::{self, WavesState};
use junctur_core::pipeline::StageState;

/// The player, or why it could not start (libmpv missing, no GL): the window still opens and
/// says so instead of the app failing to launch.
pub struct App {
    pub player: Mutex<Result<Player, String>>,
    pub analysis: Analysis,
    /// The open recording's waveforms (2g).
    pub waves: waves::Store,
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
        if let Some(core) = handle.try_state::<App>().and_then(|a| a.core().ok()) {
            // The track roles come from a run's first stage: the mutes follow them as soon as it
            // is done, not when Whisper is (2e). Read on each line from then on, which is cheap
            // and catches a re-analysis whose roles moved; nothing is sent unless they did.
            let tracks_done = p.project.as_ref().is_some_and(|s| {
                s.stages
                    .iter()
                    .any(|st| st.name == "tracks" && st.state == StageState::Done)
            });
            if tracks_done {
                core.tracks_known(Path::new(&p.file));
            }
            if !p.phase.busy() {
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
    let mut analysis = app.analysis.progress(file);
    let mut outdated = Vec::new();
    if let Ok(s) = &status {
        // No list, or one that does not parse left by a run that never finished its `edit` stage
        // (a list the user broke by hand after a finished run is theirs to fix, not to replace).
        let finished = s
            .stages
            .iter()
            .any(|st| st.name == "edit" && st.state == StageState::Done);
        let unanalysed = s.edit.is_none() && (s.edit_error.is_none() || !finished);
        if unanalysed {
            // The file plays even when its analysis cannot start; the panel says why.
            analysis = Some(
                app.analysis
                    .start(file, notifier(handle))
                    .unwrap_or_else(|e| app.analysis.failed(file, e)),
            );
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
    // Off the async runtime: asking whether the list holds user edits reads the track analysis
    // and the transcript.
    let analysis = app.analysis.clone();
    tauri::async_runtime::spawn_blocking(move || {
        analysis.reanalyse(Path::new(&path), replace, notifier(handle))
    })
    .await
    .map_err(|e| e.to_string())?
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

/// The open recording's tracks with their mutes and roles (2e).
#[tauri::command]
pub async fn mixer(app: State<'_, App>) -> Result<MixerState, String> {
    app.with(|p| Ok(p.mixer()))
}

/// Mutes or unmutes audio track `track` (0-based) in the preview, remembered for the recording.
/// Playback only, so allowed while the recording is analysed.
#[tauri::command]
pub async fn set_muted(app: State<'_, App>, track: usize, muted: bool) -> Result<(), String> {
    app.core()?.set_muted(track, muted)
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

/// Sets the caption settings: export and preview switches, bold and italic, how cues are cut.
#[tauri::command]
pub async fn set_captions(app: State<'_, App>, captions: Captions) -> Result<(), String> {
    app.editable()?.set_captions(captions)
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

/// The captions as the export cuts them, for the preview over the picture (5d): output seconds,
/// each cue's parts a speaker's lines.
#[tauri::command]
pub async fn caption_cues(app: State<'_, App>) -> Result<Vec<Cue>, String> {
    app.core()?.caption_cues()
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

/// Hears the open recording's speakers again from the audio, cut at the seconds pinned to people
/// (5c, "Analyse speakers again"), and puts them into the list in play. Takes about as long as the
/// speakers stage, so it runs on a blocking thread, not the async runtime's workers. Refused while
/// the open recording is being analysed (2d: that run writes its speakers too), and run behind the
/// analysis gate, so it never loads the models beside a run (one at a time on the GPU).
#[tauri::command]
pub async fn reanalyze_speakers(app: State<'_, App>) -> Result<(), String> {
    let core = app.editable()?;
    let analysis = app.analysis.clone();
    tauri::async_runtime::spawn_blocking(move || analysis.exclusive(|| core.reanalyze_speakers()))
        .await
        .map_err(|e| e.to_string())?
}

/// Forgets voice `name`, relabelling the open recording.
#[tauri::command]
pub async fn forget_voice(app: State<'_, App>, name: String) -> Result<(), String> {
    // No player (libmpv missing): the voice can still be forgotten. With one, not while the open
    // recording is analysed: the run took the voices as they were and would label it with this one.
    if app.core().is_err() {
        return crate::voices::forget(None, &name).map(|_| ());
    }
    app.editable()?.forget_voice(&name)
}

#[tauri::command]
pub async fn undo(app: State<'_, App>) -> Result<(), String> {
    app.editable()?.edit(Step::Undo)
}

#[tauri::command]
pub async fn redo(app: State<'_, App>) -> Result<(), String> {
    app.editable()?.edit(Step::Redo)
}

/// The shares of the window on the right (the side panel) and at the bottom (play bar and lanes)
/// kept free of video.
#[tauri::command]
pub async fn set_video_margins(
    app: State<'_, App>,
    right: f64,
    bottom: f64,
    top: f64,
) -> Result<(), String> {
    app.core()?.set_video_margins(right, bottom, top)
}

/// Solos or unsolos audio track `track` (0-based) in the preview; not remembered (2g).
#[tauri::command]
pub async fn set_solo(app: State<'_, App>, track: usize, on: bool) -> Result<(), String> {
    app.core()?.set_solo(track, on)
}

/// The open recording's speaker turns (2g's Speakers lane).
#[tauri::command]
pub async fn turns(app: State<'_, App>) -> Result<TurnsView, String> {
    Ok(app.core()?.turns())
}

/// How far the open recording's waveforms are.
#[tauri::command]
pub async fn waves_state(app: State<'_, App>) -> Result<Option<WavesState>, String> {
    Ok(app.waves.state())
}

/// Each track's peak per column of `[from, to)` seconds, for the lanes to draw, as raw bytes
/// ([`WavesView::bytes`]; none when no recording is open): asked on every pan, zoom and step of the
/// read, where a JSON array of numbers would be several times the size and parsed each time.
#[tauri::command]
pub async fn waves(
    app: State<'_, App>,
    from: f64,
    to: f64,
    columns: usize,
) -> Result<tauri::ipc::Response, String> {
    let bytes = app
        .waves
        .columns(from, to, columns)
        .map_or_else(Vec::new, |v| v.bytes());
    Ok(tauri::ipc::Response::new(bytes))
}

/// A recent recording's length and where its analysis stands (the start window, 2g). Off the
/// async runtime: it opens the project and may start ffprobe.
#[tauri::command]
pub async fn recent_info(path: String) -> Result<RecordingInfo, String> {
    tauri::async_runtime::spawn_blocking(move || project::info(Path::new(&path), None))
        .await
        .map_err(|e| e.to_string())?
}

/// A recent recording's still, as JPEG bytes (made once into its project folder).
#[tauri::command]
pub async fn thumbnail(path: String) -> Result<tauri::ipc::Response, String> {
    let bytes =
        tauri::async_runtime::spawn_blocking(move || project::thumbnail(Path::new(&path), None))
            .await
            .map_err(|e| e.to_string())??;
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

/// Closes the open recording (2i: File > Close recording, back to the start window). Its
/// waveform read stops; an analysis of it goes on (runs are not the player's), and reopening the
/// recording shows where it is.
#[tauri::command]
pub async fn close_file(app: State<'_, App>) -> Result<(), String> {
    app.waves.close();
    app.core()?.close()
}

/// The Settings dialog's contents (2i).
#[tauri::command]
pub async fn settings() -> SettingsView {
    settings::read()
}

/// Saves the Settings dialog. Nothing is analysed again: the page asks for `outdated` after.
#[tauri::command]
pub async fn save_settings(
    app: State<'_, App>,
    settings: Settings,
) -> Result<SettingsView, String> {
    let view = settings::save(&settings)?;
    if let Ok(core) = app.core() {
        core.set_caption_defaults(view.settings.captions);
    }
    Ok(view)
}

/// The Captions panel's "Save as default": these become the caption defaults (2i).
#[tauri::command]
pub async fn save_caption_defaults(
    app: State<'_, App>,
    captions: Captions,
) -> Result<SettingsView, String> {
    let view = settings::save_caption_defaults(captions)?;
    if let Ok(core) = app.core() {
        core.set_caption_defaults(view.settings.captions);
    }
    Ok(view)
}

/// Why `path`'s analysis is out of date with the settings now (2d's notice), asked again after
/// Settings saved a model.
#[tauri::command]
pub async fn outdated(app: State<'_, App>, path: String) -> Result<Vec<String>, String> {
    let file = Path::new(&path);
    if app.analysis.busy(file) {
        return Ok(Vec::new());
    }
    Ok(analysis::outdated(file))
}

/// Opens Windows' snap layouts for the window (2i: the pointer rests on the title bar's maximise
/// button; see `window`).
#[tauri::command]
pub fn snap_layouts(window: tauri::WebviewWindow) -> Result<(), String> {
    crate::window::snap_layouts(&window)
}

/// A line from the frontend for the app's log (timings test mode reports).
#[tauri::command]
pub fn ui_log(message: String) {
    crate::testmode::ui_log(message);
}
