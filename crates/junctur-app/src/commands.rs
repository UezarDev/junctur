//! The commands the frontend invokes. Thin: each forwards to the player or the project.
//!
//! The player's commands are `async` so Tauri runs them on its async runtime rather than the main
//! thread: an mpv command blocks until mpv takes it, and a dragged seek bar sends many.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use junctur_core::edit::Captions;
use junctur_core::export::layout::Placed;
use junctur_core::resolve::Sent;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::analysis::{self, Analysis, Change, Notify, Progress, Reanalysed, Transcription};
use crate::cuts::EditState;
use crate::editor::{Op, Step, TranscriptView};
use crate::lock::lock;
use crate::mixer::MixerState;
use crate::player::{Core, Player, TurnsView};
use crate::project::{self, ProjectStatus, RecordingInfo};
use crate::recent::{self, Recent};
use crate::settings::{self, Settings, SettingsView};
use crate::state::PlayerState;
use crate::translation::{Run, Translation};
use crate::voices::VoiceView;
use crate::waves::{self, WavesState};
use junctur_core::pipeline::StageState;
use junctur_core::transcribe;

/// The player, or why it could not start (libmpv missing, no GL): the window still opens and
/// says so instead of the app failing to launch.
pub struct App {
    pub player: Mutex<Result<Player, String>>,
    pub analysis: Analysis,
    /// The open recording's waveforms (2g).
    pub waves: waves::Store,
    /// The caption translation running in the background (5k).
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

    /// [`App::core`] for a command that changes the edit list: refused while the open recording's
    /// run has not yet written its list, whose `edit` stage would replace it (an edit made
    /// meanwhile would only be backed up and replaced). Once written, nothing later in the run
    /// replaces it (2j): edits go on while it transcribes.
    ///
    /// The command holds what this returns while it works: a run of the recording asked for
    /// meanwhile waits for it (2m, [`Analysis::edit`]), so a voice enrolled just as a re-analysis
    /// starts is in `speakers.json` before the run's speakers stage reads it. The recording is
    /// the one open now, and the edit is made to that one or refused, never to another opened
    /// meanwhile ([`Editable::file`]).
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
    /// when another is open by the time they write (2m repair).
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

/// Sends a run's progress to the page. When the run's list is written or the run ends, the player
/// first takes what it wrote (the edit list, the envelope, a new transcript) if that recording is
/// open, and when a piece of the transcript lands it reads the words up to it: the page fetches
/// the transcript again on these events, and must not get the words read before them. Every change
/// but a step also sends the transcription lane (2k: who is on it, paused or not), which the
/// Properties tab shows whichever recording is open.
pub fn notifier(handle: AppHandle) -> Notify {
    Arc::new(move |p: &Progress, change| {
        // A step only moves a bar (2h): reading the track roles on each of the ~hundred per track
        // and stage would read the tracks report from disk every time.
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
    // Closed while this was opening (2i): `close_file` closes the player first and the waveforms
    // after, so either it stopped this read or the player no longer has the file and it stops here.
    if app.core()?.file().as_deref() != Some(path.as_str()) {
        app.waves.close_for(&absolute);
    }
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
        // 2j: the list comes before the transcript, so a list does not mean a finished run. One
        // whose transcription never finished (a closed app, a failed Whisper) is resumed: every
        // finished stage and chunk is reused and the user's edits are kept.
        let untranscribed = s
            .stages
            .iter()
            .any(|st| st.name == "transcribe" && st.state != StageState::Done);
        // Not while another process transcribes it (`junctur analyze`): the stage reads
        // `running` then too, and a second Whisper on its partial would only be refused (2j
        // repair). A run of this app's own is returned by `start` as it is.
        let elsewhere = || transcribe::transcribing_elsewhere(Path::new(&s.dir));
        if unanalysed || (untranscribed && (app.analysis.busy(file) || !elsewhere())) {
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
    // Opened while its run is past its edit list (from another recording, say): the words so
    // far, not the finished transcript's file, which may be an older run's or missing. Asked
    // again after: a run that ended meanwhile has told the player already, or tells it after
    // this, either way last.
    if let Some(t) = analysis.as_ref().and_then(Progress::words_through) {
        let core = app.core()?;
        core.transcribed(&absolute, Some(t));
        if let Some(now) = app.analysis.progress(file).filter(|p| !p.phase.busy()) {
            core.transcribed(&absolute, now.words_through());
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

/// Who is on the transcription lane, in turn, and whether it is paused (2k).
#[tauri::command]
pub async fn transcription(app: State<'_, App>) -> Result<Transcription, String> {
    Ok(app.analysis.transcription())
}

/// Pauses or resumes the transcription lane from any recording (2k): the transcribing one stops
/// after the chunk it is on. The lane as it is now, also sent to the page as the event.
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

/// Analyses `path` again with the settings now ([`Analysis::reanalyse`], 5s): the user's edit list
/// is kept, with the speakers matched again. With `fresh` the list is re-cut instead; then, without
/// `replace`, a list holding the user's edits starts nothing and says so, for the page to ask, and
/// with it the edits are kept as a backup and replaced by the fresh cut.
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

/// Plays source seconds `start` to `end` on a loop (2o: review's one clip), from `start`; the
/// first call puts the player in review (the whole file plays until `end_review`).
#[tauri::command]
pub async fn play_between(app: State<'_, App>, start: f64, end: f64) -> Result<(), String> {
    app.with(|p| p.play_between(start, end))
}

/// Plays from source second `at` (2s: from just before a dragged cut edge, so the move is heard).
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

/// Playback speed, a quarter to four times (2o).
#[tauri::command]
pub async fn set_speed(app: State<'_, App>, speed: f64) -> Result<(), String> {
    app.with(|p| p.set_speed(speed))
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

/// Cuts (`keep` false) or keeps every run of transcript words in `runs` (`[first, last]` each),
/// snapped into the pauses around them, as one undo step (2p: a selection of several stretches).
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

/// Renames speaker `id` in the edit list.
#[tauri::command]
pub async fn rename_speaker(app: State<'_, App>, id: u32, name: String) -> Result<(), String> {
    let e = app.editable()?;
    e.rename_speaker(e.file(), id, &name)
}

/// Fixes the spelling of transcript word `index` (5h) and adds the fixed spelling to the word list
/// (no prompt, never twice); resolves to the entry added, if one was. A word list that cannot be
/// saved does not undo the fix: it is logged.
#[tauri::command]
pub async fn fix_word(
    app: State<'_, App>,
    index: usize,
    text: String,
) -> Result<Option<String>, String> {
    let e = app.editable()?;
    let (heard, fixed) = e.fix_word(e.file(), index, &text)?;
    match settings::learn(&heard, &fixed) {
        Ok(added) => Ok(added),
        Err(err) => {
            eprintln!("junctur-app: adding {fixed:?} to the word list: {err}");
            Ok(None)
        }
    }
}

/// Fixes transcript words `first..=last` as one text (5x: a sentence retyped; empty or the heard
/// words again clears their fixes) and adds its new words to the word list (no prompt, never twice);
/// resolves to the entries added. One word is learned as [`fix_word`] learns it. A word list that
/// cannot be saved does not undo the fix: it is logged.
#[tauri::command]
pub async fn fix_words(
    app: State<'_, App>,
    first: usize,
    last: usize,
    text: String,
) -> Result<Vec<String>, String> {
    let e = app.editable()?;
    let (heard, fixed) = e.fix_words(e.file(), first, last, &text)?;
    let learned = if first == last {
        settings::learn(&heard, &fixed).map(|a| a.into_iter().collect())
    } else {
        settings::learn_all(&heard, &fixed)
    };
    match learned {
        Ok(added) => Ok(added),
        Err(err) => {
            eprintln!("junctur-app: adding {fixed:?}'s words to the word list: {err}");
            Ok(Vec::new())
        }
    }
}

/// Sets speaker `id`'s caption colour (`#RRGGBB`).
#[tauri::command]
pub async fn recolor_speaker(app: State<'_, App>, id: u32, color: String) -> Result<(), String> {
    let e = app.editable()?;
    e.recolor_speaker(e.file(), id, &color)
}

/// Sets the caption settings: export and preview switches, bold, italic and underline, how cues are
/// cut (with 5i's words per caption), 5g's look for Resolve's Text+ and the preview, and 5j's text
/// options (case, punctuation, censoring, find and replace), which change only what captions show,
/// and 5k's `translate`, the language they are shown in ([`translate_captions`] makes it).
#[tauri::command]
pub async fn set_captions(app: State<'_, App>, captions: Captions) -> Result<(), String> {
    let e = app.editable()?;
    e.set_captions(e.file(), captions)
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
/// each cue's parts a speaker's lines, placed where the Resolve send puts them (5g) on the
/// preview's picture, `aspect` its height over its width.
#[tauri::command]
pub async fn caption_cues(app: State<'_, App>, aspect: f64) -> Result<Vec<Placed>, String> {
    app.core()?.caption_cues(aspect)
}

/// Sends the captions into DaVinci Resolve as Text+ clips (5g), through the Junctur script running
/// there; installs or refreshes that script first. Off the async runtime: Resolve can take minutes
/// over a long recording.
#[tauri::command]
pub async fn send_to_resolve(app: State<'_, App>) -> Result<Sent, String> {
    let core = app.core()?;
    tauri::async_runtime::spawn_blocking(move || core.send_to_resolve())
        .await
        .map_err(|e| e.to_string())?
}

/// Translates the open recording's captions in the background (5k), into the language its caption
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

/// The languages captions can be translated into (5k), by English name.
#[tauri::command]
pub async fn translation_languages() -> Vec<Language> {
    let mut l: Vec<Language> = junctur_core::translate::LANGUAGES
        .iter()
        .map(|&(code, name)| Language { code, name })
        .collect();
    l.sort_by_key(|l| l.name);
    l
}

/// The voices Junctur remembers (5b).
#[tauri::command]
pub async fn voices() -> Result<Vec<VoiceView>, String> {
    crate::voices::list()
}

/// Remembers `name`'s voice from the runs of transcript words in `runs` (`[first, last]` each, 2p)
/// and labels the speakers again; returns a line for the user.
#[tauri::command]
pub async fn enroll_voice(
    app: State<'_, App>,
    runs: Vec<(usize, usize)>,
    name: String,
) -> Result<String, String> {
    let e = app.editable()?;
    e.enroll_voice(e.file(), &runs, &name)
}

/// Hears the open recording's speakers again from the audio, cut at the seconds pinned to people
/// (5c, "Analyse speakers again"), and puts them into the list in play. Takes about as long as the
/// speakers stage, so it runs on a blocking thread, not the async runtime's workers. Refused while
/// the open recording's run has not written its edit list (2d: that run writes its speakers too;
/// 2j: past it, only the transcription is left, which does not), and run behind the first halves'
/// lane ([`Analysis::exclusive`]), never beside another speakers stage. It may run beside that
/// recording's transcription: both record their stage in `jobs.json`, each change read, made and
/// written back under one lock (2m repair), so neither puts back the other's stage as it was.
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

/// Closes the open recording, back to the start window (2i; 2u: File > Back to recordings and the
/// title bar's back arrow). Its waveform read stops; an analysis of it goes on (runs are not the
/// player's), and reopening the recording shows where it is.
#[tauri::command]
pub async fn close_file(app: State<'_, App>) -> Result<(), String> {
    // The player first: an open on its way checks it after starting its waveforms (`open_file`).
    let closed = app.core()?.close();
    app.waves.close();
    closed
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
        core.set_caption_defaults(view.settings.captions.clone());
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
        core.set_caption_defaults(view.settings.captions.clone());
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
