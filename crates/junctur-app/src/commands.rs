//! The commands the frontend invokes, each forwarding to the player or the project. They are
//! `async` because an mpv command blocks the main thread; whole-recording work goes to
//! [`blocking`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use junctur_core::edit::{Captions, ExistingClips, Fix};
use junctur_core::export::layout::Placed;
use junctur_core::resolve::{Answer, Outcome};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::analysis::{self, Analysis, Change, Notify, Progress, Reanalysed, Transcription};
use crate::cuts::EditState;
use crate::editor::{Op, Step, TranscriptView};
use crate::lock::lock;
use crate::mixer::MixerState;
use crate::player::{Core, FixDone, Player, TurnsView};
use crate::project::{self, ProjectStatus, RecordingInfo};
use crate::recent::{self, Recent};
use crate::settings::{self, Settings, SettingsView};
use crate::state::PlayerState;
use crate::translation::{Run, Translation};
use crate::voices::VoiceView;
use crate::waves::{self, WavesState};
use junctur_core::pipeline::{self, Project, SetAside, StageState};
use junctur_core::transcribe;

/// The player, or why it could not start (libmpv missing, no GL): the window still opens and
/// says so instead of the app failing to launch.
pub struct App {
    pub player: Mutex<Result<Player, String>>,
    pub analysis: Analysis,
    /// The open recording's waveforms.
    pub waves: waves::Store,
    /// The caption translation running in the background.
    pub translation: Translation,
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

    /// [`App::core`] for a command that changes the edit list: refused until the open recording's
    /// run has written its list, which would replace the edit. Held while the command works, so a
    /// run asked for meanwhile waits for it ([`Analysis::edit`]).
    pub(crate) fn editable(&self) -> Result<Editable, String> {
        let core = self.core()?;
        let file = core.file();
        let editing = match &file {
            Some(f) => Some(self.analysis.edit(Path::new(f)).ok_or(
                "analysing this recording: edits wait until its edit list is made".to_string(),
            )?),
            None => None,
        };
        Ok(Editable {
            core,
            file,
            _editing: editing,
        })
    }
}

/// The player's core for one edit of the open recording, and the mark that holds a run of that
/// recording off until the edit is done ([`App::editable`]).
pub(crate) struct Editable {
    core: Arc<Core>,
    file: Option<String>,
    _editing: Option<analysis::Editing>,
}

impl Editable {
    /// The recording this edit is for, as it was opened; the core's edits take it and refuse
    /// when another is open by the time they write.
    pub(crate) fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }
}

impl std::ops::Deref for Editable {
    type Target = Core;
    fn deref(&self) -> &Core {
        &self.core
    }
}

/// Sends a run's progress to the page, the player first taking what the run wrote, since the page
/// refetches on these events. Every change but a step also sends the transcription lane.
pub fn notifier(handle: AppHandle) -> Notify {
    Arc::new(move |p: &Progress, change| {
        // A step only moves a bar: reading the track roles on each would read the disk every time.
        if change == Change::Steps {
            let _ = handle.emit(analysis::EVENT, p);
            return;
        }
        if let Some(lane) = handle
            .try_state::<App>()
            .and_then(|a| a.analysis.lane_moved())
        {
            let _ = handle.emit(analysis::LANE_EVENT, lane);
        }
        let core = handle.try_state::<App>().and_then(|a| a.core().ok());
        if change == Change::Transcribed {
            if let Some(core) = core {
                core.transcribed(Path::new(&p.file), p.transcribed);
            }
            let _ = handle.emit(analysis::EVENT, p);
            return;
        }
        if let Some(core) = core {
            // The mutes follow the track roles as soon as the first stage is done; read on each
            // line after, which is cheap and catches a re-analysis whose roles moved.
            let tracks_done = p.project.as_ref().is_some_and(|s| {
                s.stages
                    .iter()
                    .any(|st| st.name == "tracks" && st.state == StageState::Done)
            });
            if tracks_done {
                core.tracks_known(Path::new(&p.file));
            }
            if !p.phase.busy() || change == Change::Listed {
                core.analysed(Path::new(&p.file), p.words_through());
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

/// Runs `f` on the async runtime's blocking threads: its result as it is, and a panic in it as the
/// join error's text.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Whether the page has had the command line's file.
static STARTUP_TAKEN: AtomicBool = AtomicBool::new(false);

/// A file named on the command line (`junctur-app <file>`, or "Open with"), for the page to open
/// once it is ready. Only the first page start gets it, so a reloaded page does not open it again.
#[tauri::command]
pub fn startup_file() -> Option<String> {
    once(std::env::args().nth(1))
}

/// `arg` the first time, nothing after ([`STARTUP_TAKEN`]).
fn once(arg: Option<String>) -> Option<String> {
    if STARTUP_TAKEN.swap(true, Ordering::SeqCst) {
        None
    } else {
        arg
    }
}

#[tauri::command]
pub async fn player_state(app: State<'_, App>) -> Result<PlayerState, String> {
    app.with(|p| Ok(p.state()))
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
        .any(|st| st.name == "edit" && st.state == StageState::Done);
    let unanalysed = s.edit.is_none() && (s.edit_error.is_none() || !finished);
    // The list comes before the transcript: a run whose transcription never finished is
    // resumed, reusing finished stages and chunks and keeping the user's edits.
    let untranscribed = s
        .stages
        .iter()
        .any(|st| st.name == "transcribe" && st.state != StageState::Done);
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

/// Analyses `path` again with the settings now ([`Analysis::reanalyse`]); `fresh` re-cuts the
/// list, and without `replace` a list holding the user's edits starts nothing, for the page to ask.
#[tauri::command]
pub async fn reanalyse(
    handle: AppHandle,
    app: State<'_, App>,
    path: String,
    fresh: bool,
    replace: bool,
) -> Result<Reanalysed, String> {
    // Off the async runtime: asking whether the list holds user edits reads the track analysis
    // and the transcript.
    let analysis = app.analysis.clone();
    tauri::async_runtime::spawn_blocking(move || {
        analysis.reanalyse(Path::new(&path), fresh, replace, notifier(handle))
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

/// Plays source seconds `start` to `end` on a loop (review's one clip), from `start`; the first
/// call puts the player in review (the whole file plays until `end_review`).
#[tauri::command]
pub async fn play_between(app: State<'_, App>, start: f64, end: f64) -> Result<(), String> {
    app.with(|p| p.play_between(start, end))
}

/// Plays from source second `at` (from just before a dragged cut edge, so the move is heard).
#[tauri::command]
pub async fn play_from(app: State<'_, App>, at: f64) -> Result<(), String> {
    app.with(|p| p.play_from(at))
}

/// Stops the loop and pauses, still in review (past the last clip).
#[tauri::command]
pub async fn pause_review(app: State<'_, App>) -> Result<(), String> {
    app.with(|p| p.pause_review())
}

/// Leaves review: no loop, 1x, the skip toggle as it was.
#[tauri::command]
pub async fn end_review(app: State<'_, App>) -> Result<(), String> {
    app.with(|p| p.end_review())
}

/// Playback speed, a quarter to four times.
#[tauri::command]
pub async fn set_speed(app: State<'_, App>, speed: f64) -> Result<(), String> {
    app.with(|p| p.set_speed(speed))
}

/// Returns the source time the seek lands on (a cut lands on the next kept range while skipping).
#[tauri::command]
pub async fn seek(app: State<'_, App>, seconds: f64, exact: bool) -> Result<f64, String> {
    app.with(|p| p.seek(seconds, exact))
}

/// The open recording's tracks with their mutes and roles.
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

/// Renames speaker `id` in the edit list.
#[tauri::command]
pub async fn rename_speaker(app: State<'_, App>, id: u32, name: String) -> Result<(), String> {
    let e = app.editable()?;
    e.rename_speaker(e.file(), id, &name)
}

/// What a fix did besides itself, for the page: the entries the word list gained, and the fixes
/// also made on the words heard like it with what those were heard as (for [`keep_heard`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fixed {
    pub learned: Vec<String>,
    pub others: Vec<Fix>,
    pub heard: Vec<String>,
}

/// Saves what `done` teaches to the word list (no prompt, never twice). A word list that cannot be
/// saved does not undo the fix: it is logged.
fn learned(done: FixDone, fixed: &str) -> Fixed {
    let learned = settings::learn(&done.learn).unwrap_or_else(|err| {
        eprintln!("junctur-app: adding {fixed:?}'s words to the word list: {err}");
        Vec::new()
    });
    Fixed {
        learned,
        others: done.others,
        heard: done.heard,
    }
}

/// Fixes the spelling of transcript word `index` (empty, or the heard text again, removes the fix),
/// and every other word heard like it; its spelling joins the word list when the recording never
/// heard it (or only changed its case).
#[tauri::command]
pub async fn fix_word(app: State<'_, App>, index: usize, text: String) -> Result<Fixed, String> {
    let e = app.editable()?;
    let done = e.fix_word(e.file(), index, &text)?;
    Ok(learned(done, &text))
}

/// Marks transcript word `index` as checked: it keeps what Whisper heard and is no longer
/// flagged. Nothing joins the word list.
#[tauri::command]
pub async fn confirm_word(app: State<'_, App>, index: usize) -> Result<(), String> {
    let e = app.editable()?;
    e.confirm_word(e.file(), index)
}

/// Fixes transcript words `first..=last` as one text (empty or the heard words again clears their
/// fixes), and the same words heard elsewhere; its words the recording never heard join the word
/// list. One word is learned as [`fix_word`] learns it.
#[tauri::command]
pub async fn fix_words(
    app: State<'_, App>,
    first: usize,
    last: usize,
    text: String,
) -> Result<Fixed, String> {
    let e = app.editable()?;
    let done = e.fix_words(e.file(), first, last, &text)?;
    Ok(learned(done, &text))
}

/// Takes back fixes a fix made on other words ("Undo those"), where they are still as made:
/// those words keep what they were heard as (`heard`, beside each), and no later spread reaches
/// them. How many were still as made (none once a re-transcription moved them).
#[tauri::command]
pub async fn keep_heard(
    app: State<'_, App>,
    others: Vec<Fix>,
    heard: Vec<String>,
) -> Result<usize, String> {
    let e = app.editable()?;
    e.keep_heard(e.file(), &others, &heard)
}

/// The fixes a re-transcription of the open recording could not put on any word, until
/// [`forget_fixes_set_aside`].
#[tauri::command]
pub async fn fixes_set_aside(app: State<'_, App>) -> Result<Vec<SetAside>, String> {
    let Some(file) = app.core()?.file() else {
        return Ok(Vec::new());
    };
    let p = Project::open(Path::new(&file), None, None).map_err(|e| e.to_string())?;
    Ok(pipeline::fixes_set_aside(&p))
}

/// The user has seen `seen`, fixes set aside for the open recording as [`fixes_set_aside`] listed
/// them: those are forgotten, and any a run set aside since stay.
#[tauri::command]
pub async fn forget_fixes_set_aside(
    app: State<'_, App>,
    seen: Vec<SetAside>,
) -> Result<(), String> {
    let Some(file) = app.core()?.file() else {
        return Ok(());
    };
    let p = Project::open(Path::new(&file), None, None).map_err(|e| e.to_string())?;
    pipeline::forget_fixes_set_aside(&p, &seen).map_err(|e| e.to_string())
}

/// What the Correct button started.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Corrected {
    /// The run hearing the speakers and the words again.
    pub progress: Progress,
    /// The fixes its first step made on words heard like a fixed one, and what those were heard as
    /// (the note's Undo, as for a fix).
    pub others: Vec<Fix>,
    pub heard: Vec<String>,
    /// Heard texts fixed two ways, left as heard.
    pub unclear: Vec<String>,
}

/// The Correct button: every fix spread to the words heard like it, then a run hearing the speakers
/// and the words again, keeping the user's edit list (`Analysis::correct`). Refused while analysed.
#[tauri::command]
pub async fn correct(handle: AppHandle, app: State<'_, App>) -> Result<Corrected, String> {
    let e = app.editable()?;
    let path = e.file().ok_or("no recording is open")?.to_string();
    // The run first, while this edit's mark is held: refused, nothing has changed; started, it
    // waits for the mark before it reads anything, so the spread below is in the list it keeps.
    let progress = app.analysis.correct(Path::new(&path), notifier(handle))?;
    let s = e.spread_fixes(e.file()).unwrap_or_else(|err| {
        eprintln!("junctur-app: Correct's spread: {err}");
        Default::default()
    });
    drop(e);
    Ok(Corrected {
        progress,
        others: s.added,
        heard: s.heard,
        unclear: s.unclear,
    })
}

/// Sets speaker `id`'s caption colour (`#RRGGBB`).
#[tauri::command]
pub async fn recolor_speaker(app: State<'_, App>, id: u32, color: String) -> Result<(), String> {
    let e = app.editable()?;
    e.recolor_speaker(e.file(), id, &color)
}

/// Sets the caption settings: switches, styling, how cues are cut, the Text+ look, text options
/// that change only what captions show, and `translate` ([`translate_captions`] makes it).
#[tauri::command]
pub async fn set_captions(app: State<'_, App>, captions: Captions) -> Result<(), String> {
    let e = app.editable()?;
    e.set_captions(e.file(), captions)
}

/// What an export wrote: the file's captions.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub cues: usize,
    /// Per-speaker files beside it from an older export.
    pub stale: Vec<String>,
}

/// Writes the captions as one SRT to `path`, people talking at once stacked in one caption.
#[tauri::command]
pub async fn export_captions(app: State<'_, App>, path: String) -> Result<Exported, String> {
    let core = app.core()?;
    let f = blocking(move || core.export_captions(Path::new(&path))).await?;
    Ok(Exported {
        cues: f.cues,
        stale: f.stale.iter().map(|p| p.display().to_string()).collect(),
    })
}

/// The captions as the export cuts them, for the preview: output seconds, placed where the Resolve
/// send puts them, `aspect` the picture's height over its width.
#[tauri::command]
pub async fn caption_cues(app: State<'_, App>, aspect: f64) -> Result<Vec<Placed>, String> {
    let core = app.core()?;
    blocking(move || core.caption_cues(aspect)).await
}

/// Sends the edit into DaVinci Resolve as a timeline with Text+ captions, through the Junctur
/// script there. `answer` decides on a timeline of that name already there (none: answers
/// `exists`), `existing` on clips Junctur did not place in the captions' way. Takes minutes.
#[tauri::command]
pub async fn send_to_resolve(
    app: State<'_, App>,
    answer: Option<Answer>,
    existing: Option<ExistingClips>,
) -> Result<Outcome, String> {
    let core = app.core()?;
    tauri::async_runtime::spawn_blocking(move || core.send_to_resolve(answer, existing))
        .await
        .map_err(|e| e.to_string())?
}

/// Translates the open recording's captions in the background, into the language its caption
/// settings name: the sentences no translation is kept for yet. Progress comes as `translation`
/// events. Gathering the sentences reads the whole transcript, so off the async runtime.
#[tauri::command]
pub async fn translate_captions(app: State<'_, App>, handle: AppHandle) -> Result<Run, String> {
    let core = app.core()?;
    let job = tauri::async_runtime::spawn_blocking(move || core.translation_job())
        .await
        .map_err(|e| e.to_string())??;
    Ok(app.translation.start(handle, job))
}

/// Stops the caption translation after the sentences in hand; those done are kept.
#[tauri::command]
pub async fn cancel_translation(app: State<'_, App>) -> Result<(), String> {
    app.translation.cancel();
    Ok(())
}

/// The latest translation run, and how much of the open recording's captions is left to
/// translate.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationStatus {
    pub run: Run,
    pub missing: usize,
    pub sentences: usize,
}

#[tauri::command]
pub async fn translation_status(app: State<'_, App>) -> Result<TranslationStatus, String> {
    let core = app.core()?;
    let (missing, sentences) = tauri::async_runtime::spawn_blocking(move || core.untranslated())
        .await
        .map_err(|e| e.to_string())??;
    Ok(TranslationStatus {
        run: app.translation.run(),
        missing,
        sentences,
    })
}

#[derive(Serialize)]
pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
}

/// The languages captions can be translated into, by English name.
#[tauri::command]
pub async fn translation_languages() -> Vec<Language> {
    let mut l: Vec<Language> = junctur_core::translate::LANGUAGES
        .iter()
        .map(|&(code, name)| Language { code, name })
        .collect();
    l.sort_by_key(|l| l.name);
    l
}

/// The Fusion Titles in the open Resolve project's media pool, for the template picker,
/// through the Junctur script running there. Off the async runtime: a script not listening takes
/// seconds to give up on, a busy Resolve longer.
#[tauri::command]
pub async fn resolve_templates() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(junctur_core::resolve::templates)
        .await
        .map_err(|e| e.to_string())?
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
    tauri::async_runtime::spawn_blocking(move || {
        analysis.exclusive(|| e.reanalyze_speakers(e.file()))
    })
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
    let e = app.editable()?;
    e.forget_voice(e.file(), &name)
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

/// Solos or unsolos audio track `track` (0-based) in the preview; not remembered.
#[tauri::command]
pub async fn set_solo(app: State<'_, App>, track: usize, on: bool) -> Result<(), String> {
    app.core()?.set_solo(track, on)
}

/// The open recording's speaker turns, for the Speakers lane.
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
    handle: AppHandle,
    from: f64,
    to: f64,
    columns: usize,
) -> Result<tauri::ipc::Response, String> {
    let bytes = blocking(move || {
        Ok(handle
            .state::<App>()
            .waves
            .columns(from, to, columns)
            .map_or_else(Vec::new, |v| v.bytes()))
    })
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// A recent recording's length and where its analysis stands (the start window). Off the
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

/// Closes the open recording, back to the start window. Its waveform read stops; an analysis of it
/// goes on, and reopening the recording shows where it is.
#[tauri::command]
pub async fn close_file(app: State<'_, App>) -> Result<(), String> {
    // The player first: an open on its way checks it after starting its waveforms (`open_file`).
    let closed = app.core()?.close();
    app.waves.close();
    closed
}

/// The Settings dialog's contents.
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
        core.set_caption_defaults(view.settings.captions.clone());
    }
    Ok(view)
}

/// The Captions panel's "Save as default": these become the caption defaults.
#[tauri::command]
pub async fn save_caption_defaults(
    app: State<'_, App>,
    captions: Captions,
) -> Result<SettingsView, String> {
    let view = settings::save_caption_defaults(captions)?;
    if let Ok(core) = app.core() {
        core.set_caption_defaults(view.settings.captions.clone());
    }
    Ok(view)
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

/// Opens Windows' snap layouts for the window, when the pointer rests on the title bar's maximise
/// button (see `window`).
#[tauri::command]
pub fn snap_layouts(window: tauri::WebviewWindow) -> Result<(), String> {
    crate::window::snap_layouts(&window)
}

/// A line from the frontend for the app's log (timings test mode reports).
#[tauri::command]
pub fn ui_log(message: String) {
    crate::testmode::ui_log(message);
}

#[cfg(test)]
mod tests {
    use super::*;

    // The only test of the process-wide `STARTUP_TAKEN`, which `startup_file` hands out through.
    #[test]
    fn startup_file_is_given_once() {
        assert_eq!(once(Some("a.mp4".into())).as_deref(), Some("a.mp4"));
        // A reloaded page asks again: the file is not opened a second time.
        assert_eq!(once(Some("a.mp4".into())), None);
        assert_eq!(once(None), None);
    }
}
