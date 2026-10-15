//! The job queue: a recording's analysis stages, each leaving its result in the project folder so
//! a rerun skips what is done. Two halves, [`prepare`] (minutes) and [`finish`] (Whisper, hours), so
//! one recording's fast stages need not wait on another's transcription. Within a recording the
//! stages run one at a time: Whisper and the ONNX models share one GPU, and two at once on a full
//! card would both fall back to the CPU.

mod adopt;
mod again;
mod editlist;
mod jobs;
mod smartkeep;
mod speaker_files;
mod speakers;
mod speech;
mod stages;
mod steps;

use std::path::{Path, PathBuf};

pub use jobs::{Jobs, StageName, StageRecord, StageState, STAGES};
pub use speakers::{hearing_key, seeded, speech_key, Found, Heard, Method, Pin};
pub use speech::{Speech, SpeechTrack};
pub use steps::{Step, Steps};

pub use again::{fixes_set_aside, forget_fixes_set_aside, SetAside};
pub use editlist::{cut, hand_edited, recut};
pub use speaker_files::{
    describe, enroll, enroll_and_save, labelled, labelled_renamed, rehear, relabel, respeak,
    save_speakers, speakers_for,
};

use again::{carry_fixes, hear_again, heard_again, not_again};
#[cfg(test)]
use editlist::keep_or_write;
use editlist::{draft, rewrite, Rewrite};
use smartkeep::keep_stage;
use stages::{count_words, labels_stage, speakers_stage, speech_stage, transcribe_stage, Stage};

pub use smartkeep::{automatic_keep, judge, judged_now, standing, KeepRun, Standing, KEEP_OFF};
pub use stages::LABELS_OFF;

use crate::edit::{self, CutSettings, EditList};
use crate::labels;
use crate::runtime::DevicePref;
use crate::tracks::{self, Role, TrackReport};
use crate::transcribe::{self, Languages, ModelPref, TranscribeOptions, Transcript};
use crate::voices::Voice;
use crate::{cache, hidden, Error, Result};

const EDIT_FILE: &str = "edit.junctur";
const ENVELOPE_FILE: &str = "envelope.f32";

/// The speech tracks' envelope the `speech` stage left in project folder `dir`, which hand-made
/// cuts snap to as the reduction's do. `None` before that stage ran.
pub fn envelope(dir: &Path) -> Option<edit::Envelope> {
    speech::load_envelope(&dir.join(ENVELOPE_FILE)).ok()
}

/// Where a recording's derived files live: `<cache>/projects/<key>/` unless the caller names a
/// folder (e.g. one inside a git repo, to track the edit list).
#[derive(Debug, Clone)]
pub struct Project {
    pub file: PathBuf,
    pub dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Project {
    pub fn open(file: &Path, cache_dir: Option<PathBuf>, dir: Option<PathBuf>) -> Result<Self> {
        let cache_dir = cache_dir.unwrap_or_else(cache::default_dir);
        let in_cache = dir.is_none();
        let dir = match dir {
            Some(d) => d,
            None => cache_dir
                .join("projects")
                .join(cache::key(file, "project1")?),
        };
        std::fs::create_dir_all(&dir)?;
        // A run started by a packaged app (Claude desktop) may have analysed this recording into
        // a hidden copy of the cache ([`adopt`]). Its track reports are keyed by file and settings, so
        // a missing one is taken alone, once per process.
        if in_cache {
            adopt::project(&dir);
        }
        hidden::adopt_files_once(&cache_dir.join("tracks"), |_| true);
        Ok(Self {
            // Absolute: the edit list and the FCPXML name the source, and must work from anywhere.
            // Not `canonicalize`, whose `\\?\` form Resolve cannot open.
            file: std::path::absolute(file)?,
            dir,
            cache_dir,
        })
    }

    pub fn edit_path(&self) -> PathBuf {
        self.dir.join(EDIT_FILE)
    }
    fn speech_path(&self) -> PathBuf {
        self.dir.join("speech.json")
    }
    fn speakers_path(&self) -> PathBuf {
        self.dir.join("speakers.json")
    }
    fn envelope_path(&self) -> PathBuf {
        self.dir.join(ENVELOPE_FILE)
    }

    pub fn jobs(&self) -> Jobs {
        Jobs::load(&self.dir)
    }

    pub fn speech(&self) -> Option<Speech> {
        serde_json::from_slice(&std::fs::read(self.speech_path()).ok()?).ok()
    }

    /// Who talks when, as the `speakers` stage left it; `None` before it ran (or when the file
    /// cannot be read: the stage sets such a file aside, [`speakers_stage`]).
    pub fn speakers(&self) -> Option<Found> {
        self.speakers_file().ok().flatten()
    }

    /// `speakers.json`: `None` when there is none, an error when it is there but unreadable. It
    /// holds the user's pins, so an unreadable one is not the same as none.
    fn speakers_file(&self) -> Result<Option<Found>> {
        match std::fs::read(self.speakers_path()) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn edit(&self) -> Result<EditList> {
        self.parse_edit(&std::fs::read_to_string(self.edit_path())?)
    }

    /// `text` read as this project's edit list, refused as [`Self::edit`] refuses it.
    pub fn parse_edit(&self, text: &str) -> Result<EditList> {
        edit::parse(text).map_err(|e| Error::Edit(format!("{}: {e}", self.edit_path().display())))
    }
}

#[derive(Debug, Clone, Default)]
pub struct PipelineOptions {
    /// Where Whisper and the labels' vision tower may run. Only they use it: tracks, speech and
    /// speakers always run on the CPU (CAM++ on the GPU finds a speaker per sentence).
    pub device: DevicePref,
    pub model: ModelPref,
    /// The word list Whisper is told (the caller reads the config file). Not part of what makes
    /// a transcript up to date: a changed list applies to the next transcription only.
    pub words: Vec<String>,
    pub languages: Languages,
    pub cut: CutSettings,
    /// Redo every stage instead of reusing results.
    pub force: bool,
    /// Stop after speech detection (no Whisper).
    pub no_transcript: bool,
    /// Skip telling speakers apart (the edit list then names none).
    pub no_speakers: bool,
    /// Enrolled voices to label speakers by (the caller reads the voices file).
    pub voices: Vec<Voice>,
    /// Whether the silent parts are labelled, and with which list ([`labels`]).
    pub labels: labels::Settings,
    /// Smart keep's settings and this run's prompt ([`crate::keep`]).
    pub keep: KeepRun,
    /// Whether an edit list holding the user's edits is kept or re-cut ([`KeepList`]).
    pub keep_list: KeepList,
    /// Hear the speakers again from the audio, cut at the user's pins, instead of reusing the
    /// hearing (the app's Correct pop-up; what [`rehear`] does outside a run).
    pub rehear: bool,
    /// Transcribe the recording again, the word list as it is now, and carry the edit list's fixes
    /// onto the new words ([`finish`]; the app's Correct pop-up). Apart from
    /// `rehear` and `keep_list`, so either hearing can be asked for alone.
    pub retranscribe: bool,
    /// What of the user's this run drops instead of carrying ([`Forget`]).
    pub forget: Forget,
    /// Only this stretch of the recording, in source seconds: the words transcribed again on the
    /// chunks it touches, the cut made again inside it, the fixes dropped inside it; the rest of
    /// the transcript and the list as they were. The speakers are heard over the whole recording
    /// only, so it is refused with `rehear` or `forget.names` ([`check`]).
    pub within: Option<(f64, f64)>,
}

/// What a run drops of the user's instead of carrying it onto the new analysis (the app's Correct
/// pop-up, its "keep" boxes unticked). Each kept list that loses something is backed up first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forget {
    /// The speakers' names in the list and the seconds pinned to people (`speakers.json`, copied to
    /// `speakers.json.bak*` first): the speakers are heard again and named afresh, the remembered
    /// voices still naming who they recognise. Implies `rehear`.
    pub names: bool,
    /// The fixed words and the word times set by moving a cut's edge, in `within` or everywhere:
    /// dropped at the list's write, so a re-transcription after has nothing to carry.
    pub fixes: bool,
}

impl Forget {
    pub fn any(&self) -> bool {
        self.names || self.fixes
    }
}

/// There from the moment a hearing that names the speakers afresh is in `speakers.json` (its pins
/// left out, [`speakers_stage`]) until the edit list has its speakers named afresh too: a run stopped
/// in between would otherwise carry the old names onto the new hearing on its resume. Empty.
const AFRESH_FILE: &str = "speakers-afresh";

/// What the edit stage of this run forgets: what it was asked, and the names when an earlier run that
/// was asked to name the speakers afresh stopped before its list ([`AFRESH_FILE`]) and speakers were
/// found this time (with none, the list keeps what it has, as any run without speakers).
fn forget_now(p: &Project, opts: &PipelineOptions, found: bool) -> Forget {
    Forget {
        names: opts.forget.names || (found && p.dir.join(AFRESH_FILE).is_file()),
        ..opts.forget
    }
}

/// Why a run asked to name the speakers afresh stopped: `why` is the speakers stage's error (their
/// model not downloaded offline, say, or the new hearing not written), which its own line would
/// only show in passing.
fn not_named_afresh(why: &str) -> Error {
    Error::Edit(format!(
        "the speakers were not named afresh, because they could not be heard or saved ({why}): the run stopped there, with the edit list and the words as they were. Keep the names and voices to redo the rest without hearing them"
    ))
}

/// The edit list has the speakers as heard last: an [`AFRESH_FILE`] is done with.
fn afresh_done(p: &Project, log: &mut dyn FnMut(&str)) {
    let path = p.dir.join(AFRESH_FILE);
    if let Err(e) = crate::output::remove_retrying(&path) {
        if path.exists() {
            log(&format!("could not remove {}: {e}", path.display()));
        }
    }
}

/// Refuses options a run cannot do soundly, before anything is written: a stretch with the speakers
/// heard again (they are clustered over the whole recording, and a stretch's clusters match no one
/// outside it), and a stretch's words transcribed again with nothing to splice them into or a
/// transcript that does not answer the settings (it would mix two models or languages; under `Auto`
/// the stretch is heard by the transcript's own model, which must be one this version loads), or
/// while an earlier transcription is unfinished (a run that redoes nothing resumes it first).
fn check(p: &Project, opts: &PipelineOptions) -> Result<()> {
    let Some((a, b)) = opts.within else {
        return Ok(());
    };
    let refuse = |why: &str| Err(Error::Edit(why.to_string()));
    if !(a.is_finite() && b.is_finite() && a < b) {
        return refuse("the stretch to analyse again is empty");
    }
    if opts.rehear || opts.forget.names {
        return refuse(
            "the speakers are heard over the whole recording only: analyse the whole timeline to hear them again",
        );
    }
    if opts.retranscribe && !opts.no_transcript {
        if transcribe::interrupted(&p.dir) {
            return refuse(transcribe::UNFINISHED);
        }
        let t = TranscribeOptions {
            model: opts.model,
            languages: opts.languages.clone(),
            ..Default::default()
        };
        match transcribe::made_with(&p.dir) {
            None => return refuse("there is no transcript to transcribe a part of again: analyse the whole timeline"),
            Some(Err(e)) => return refuse(&format!("the transcript could not be read ({e}): analyse the whole timeline")),
            Some(Ok((model, languages))) if !transcribe::answers(&model, &languages, &t) => {
                return refuse("the transcript was made with another model or languages than the settings ask: analyse the whole timeline, or one transcript would mix both")
            }
            // Heard with the model that made it ([`transcribe::stretch_model`]), so it must be
            // one this version loads.
            Some(Ok((model, _))) if model.parse::<ModelPref>().is_err() => {
                return refuse(&format!("the transcript was made with {model}, which this version cannot run: analyse the whole timeline"))
            }
            Some(Ok(_)) => {}
        }
    }
    Ok(())
}

/// What a run's `edit` stage does with an edit list on disk that holds the user's edits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeepList {
    /// Re-cut it, keeping it as `edit.junctur.bak*` (`junctur analyze`, and the app's Correct
    /// pop-up with Cuts unticked, which asks first); with `within`, only inside that stretch.
    #[default]
    No,
    /// Keep it, taking only the speakers the run found; one that does not parse (a broken hand
    /// edit) is backed up and re-cut, or the recording would never have a list (the app resuming a
    /// run the user edited during).
    Edits,
    /// As `Edits`, and one that does not parse is left as it is too: the run fails saying so,
    /// since re-cutting is only ever the user's choice here (the app's Correct pop-up with Cuts
    /// kept).
    Whole,
}

/// What a run's first half leaves for the second ([`prepare`]): the edit list is written by then.
pub struct Prepared {
    pub report: TrackReport,
    pub speech: Speech,
    pub speakers: Option<Found>,
    pub edit: EditList,
}

/// What a finished run produced.
pub struct Outcome {
    pub report: TrackReport,
    pub speech: Speech,
    pub speakers: Option<Found>,
    pub transcript: Option<Transcript>,
    pub edit: EditList,
}

/// The options the `tracks` stage analyses `p` with: one place for the stage and for every reader
/// of what it cached, whose cache key depends on them.
pub fn tracks_options(p: &Project, force: bool) -> tracks::AnalyzeOptions {
    tracks::AnalyzeOptions {
        cache_dir: Some(p.cache_dir.clone()),
        use_cache: !force,
        ..Default::default()
    }
}

/// The track report the `tracks` stage left for `p`, without analysing: `None` before it ran.
/// For the app's player, which only wants the roles when they are known.
pub fn cached_tracks(p: &Project) -> Option<TrackReport> {
    tracks::cached(&p.file, &tracks_options(p, false))
}

/// Run every stage in order. `log` gets one line per event, for the CLI to print.
pub fn run(p: &Project, opts: &PipelineOptions, log: &mut dyn FnMut(&str)) -> Result<Outcome> {
    run_with(p, opts, log, steps::none())
}

/// [`run`], telling `steps` how far each track has got through the speech, speakers and
/// transcribe stages as they go (the app's progress bars). [`prepare`], then [`finish`].
pub fn run_with(
    p: &Project,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
    steps: Steps,
) -> Result<Outcome> {
    let prepared = prepare(p, opts, log, steps)?;
    finish(p, opts, prepared, log, steps, &mut |_| {}, &mut || {})
}

/// A run's first half: tracks, speech, speakers and the edit list, written at its end without word
/// counts (nothing else in it needs the transcript, so the recording is editable before Whisper
/// starts). With `opts.keep_list` a list holding the user's edits is kept, with this run's speakers
/// ([`KeepList`]).
pub fn prepare(
    p: &Project,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
    steps: Steps,
) -> Result<Prepared> {
    check(p, opts)?;
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();

    let report = jobs.stage(&p.dir, StageName::Tracks, log, |_| {
        tracks::analyze(&p.file, &tracks_options(p, opts.force))
    })?;
    let speech = speech_stage(
        Stage {
            p,
            jobs: &mut jobs,
            report: &report,
            steps,
        },
        opts.force,
        log,
    )?;
    // Speakers are an extra, not core: when they cannot be found (the model cannot be fetched
    // offline, say), the run goes on without them and the edit list keeps the names it had.
    let heard = if opts.no_speakers {
        Err("telling the speakers apart is off for this run".to_string())
    } else {
        let stage = Stage {
            p,
            jobs: &mut jobs,
            report: &report,
            steps,
        };
        speakers_stage(stage, &speech, opts, log).map_err(|e| e.to_string())
    };
    let speakers = match heard {
        Ok(f) => Some(f),
        // Asked for by the user, so not gone on without: stopped saying why, before the list.
        Err(why) if opts.forget.names => return Err(not_named_afresh(&why)),
        Err(why) => {
            log(&format!("speakers: {why}; going on without them"));
            None
        }
    };
    let edit = jobs.stage(&p.dir, StageName::Edit, log, |log| {
        let d = draft(p, &report, &speech, speakers.as_ref(), None, &opts.cut);
        let how = Rewrite {
            keep: opts.keep_list,
            within: opts.within,
            forget: forget_now(p, opts, speakers.is_some()),
        };
        rewrite(p, d, speakers.as_ref(), &how, log)
    })?;
    afresh_done(p, log);
    Ok(Prepared {
        report,
        speech,
        speakers,
        edit,
    })
}

/// A run's second half: the labels of the silent parts when they are on ([`labels`]), then Whisper
/// over the speech, piece by piece (`transcribed` is told where the transcript is whole, `between`
/// is called between chunks and may block: the app's pause; [`transcribe::Reports`]), then the words
/// counted into the edit list's score windows in place ([`count_words`]), then smart keep's rules when
/// the recording has a prompt ([`judge`]), transcript or not. Never re-cuts: edits made since [`prepare`] stay.
pub fn finish(
    p: &Project,
    opts: &PipelineOptions,
    prepared: Prepared,
    log: &mut dyn FnMut(&str),
    steps: Steps,
    transcribed: &mut dyn FnMut(f64),
    between: &mut dyn FnMut(),
) -> Result<Outcome> {
    let Prepared {
        report,
        speech,
        speakers,
        mut edit,
    } = prepared;
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();
    // Labels are an extra, like the speakers: when they cannot be made (the model cannot be
    // fetched offline, say), the run goes on to the transcript.
    let stage = Stage {
        p,
        jobs: &mut jobs,
        report: &report,
        steps,
    };
    if let Err(e) = labels_stage(stage, &speech, opts, log) {
        log(&format!("labels: {e}; going on without them"));
    }
    let transcript = if opts.no_transcript {
        None
    } else {
        let again = hear_again(p, opts, log)?;
        let t = transcribe_stage(
            Stage {
                p,
                jobs: &mut jobs,
                report: &report,
                steps,
            },
            &speech,
            opts,
            again,
            log,
            transcribed,
            between,
        );
        let t = match t {
            Ok(t) => t,
            Err(e) => {
                not_again(p, log);
                return Err(e);
            }
        };
        heard_again(p, log);
        carry_fixes(p, &t, log);
        // The counts are for later modes, nothing reads them yet: a list that cannot be written
        // now is no reason to fail a finished transcription.
        match count_words(p, &speech, &t, log) {
            Ok(Some(e)) => edit = e,
            Ok(None) => {}
            Err(e) => log(&format!("edit list: word counts not written ({e})")),
        }
        Some(t)
    };
    // An extra, like the labels: smart keep that fails (a file of its that cannot be read or
    // written) leaves the run done. With or without a transcript: the rules read no words (3e).
    let stage = Stage {
        p,
        jobs: &mut jobs,
        report: &report,
        steps,
    };
    match keep_stage(stage, &speech, opts, log) {
        Ok(Some(_)) => {
            if let Ok(e) = p.edit() {
                edit = e;
            }
        }
        Ok(None) => {}
        Err(e) => log(&format!("keep: {e}; going on without it")),
    }
    Ok(Outcome {
        report,
        speech,
        speakers,
        transcript,
        edit,
    })
}

/// Tracks to listen to: every distinct track whose role says it carries speech (voice or mix),
/// read as it is. Music and game tracks are left alone.
pub fn speech_tracks(report: &TrackReport) -> Vec<usize> {
    report
        .logical
        .iter()
        .filter(|l| matches!(l.role, Role::Voice | Role::Mix))
        .filter(|l| l.speech.as_ref().is_none_or(|s| s.share > 0.0))
        .map(|l| l.members[0])
        .collect()
}

/// Why a re-run with `opts` would redo more than the cut, one line each for the user: no
/// transcript yet, or one made with another model or languages than `opts` asks; labels asked for
/// and missing or made with another list ([`labels_outdated`]). Empty when the analysis answers
/// `opts`; `Auto` takes any model, as [`run`] does, so this never offers a re-run that would redo
/// nothing.
pub fn outdated(p: &Project, opts: &PipelineOptions) -> Vec<String> {
    if opts.no_transcript {
        return Vec::new();
    }
    let mut out = transcript_outdated(p, opts);
    out.extend(labels_outdated(p, opts));
    out
}

/// Why the labels of the silent parts do not answer `opts`, when they are on: none yet (or the last
/// run had them off), busy read from another track than the track report names now (or the read
/// failed), or made with another list or model. `None` when they are off, up to date, or
/// the speech is not found yet (the run that finds it makes them).
fn labels_outdated(p: &Project, opts: &PipelineOptions) -> Option<String> {
    if !opts.labels.on {
        return None;
    }
    let (speech, report) = (p.speech()?, cached_tracks(p)?);
    let off = p
        .jobs()
        .get(StageName::Labels)
        .is_some_and(|r| r.note.as_deref() == Some(LABELS_OFF));
    let pieces = labels::pieces::pieces(&speech.all_regions(), report.duration);
    let busy = labels::busy::track(&report);
    match labels::load(&p.dir).filter(|_| !off) {
        None => Some("The parts where nobody talks have no labels yet.".into()),
        Some(l) if l.busy_track != busy => {
            Some("The labels' busy scores are not from the game track found now.".into())
        }
        Some(l) if !labels::answers(&l, &opts.labels.labels(), &pieces, busy) => {
            Some("The labels were made with another list.".into())
        }
        Some(_) => None,
    }
}

/// [`outdated`]'s lines about the transcript.
fn transcript_outdated(p: &Project, opts: &PipelineOptions) -> Vec<String> {
    let t = TranscribeOptions {
        model: opts.model,
        device: opts.device,
        languages: opts.languages.clone(),
        ..Default::default()
    };
    match transcribe::made_with(&p.dir) {
        None => vec!["There is no transcript yet.".to_string()],
        Some(Err(e)) => vec![format!("The transcript could not be read ({e}).")],
        Some(Ok((model, languages))) if !transcribe::answers(&model, &languages, &t) => {
            vec![if languages != t.languages.allowed {
                format!(
                    "Transcribed in {}; the settings ask for {}.",
                    languages.join("+"),
                    t.languages.key()
                )
            } else {
                format!(
                    "Transcribed with {model}; the settings ask for {}.",
                    opts.model.name()
                )
            }]
        }
        Some(Ok(_)) => Vec::new(),
    }
}

/// The labels of `p` smart keep and the editor's Labels lane read: none when the last run had them off,
/// and none when `labels.json` no longer answers `settings`' list over the speech and busy track as
/// they are now (a labels run that failed since leaves the older file behind).
fn labels_now(
    p: &Project,
    jobs: &Jobs,
    settings: &labels::Settings,
    speech: &[crate::vad::Region],
    report: &TrackReport,
) -> Option<labels::Labels> {
    let off = jobs
        .get(StageName::Labels)
        .is_some_and(|r| r.note.as_deref() == Some(LABELS_OFF));
    labels::current(&p.dir, settings, speech, report).filter(|_| !off)
}

/// [`labels_now`] from what `p`'s analysis left: `None` before its speech and tracks are found.
pub fn current_labels(p: &Project, settings: &labels::Settings) -> Option<labels::Labels> {
    let (speech, report) = (p.speech()?, cached_tracks(p)?);
    labels_now(p, &p.jobs(), settings, &speech.all_regions(), &report)
}

/// What a step that needs the analysis answers before there is one.
fn not_analysed() -> Error {
    Error::Edit("run `junctur analyze` on this file first".into())
}

#[cfg(test)]
mod tests;
