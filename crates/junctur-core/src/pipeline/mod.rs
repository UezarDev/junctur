//! The job queue: analysis stages run one after another for a recording, each leaving its result
//! in the project folder, so a rerun skips what is done and an interrupted one resumes.
//!
//! Sequential within a recording: Whisper and the ONNX models share one GPU, and running two at
//! once on a card another app already fills would only make both fall back to CPU. Across
//! recordings the run is two halves (2j): [`prepare`] (tracks, speech, speakers and the edit list,
//! minutes at most) and [`finish`] (Whisper, hours), so a caller can run a recording's first half
//! while another's transcription goes on, as Uezar asked: "tracks speech and speakers runs
//! generally fast, so we shouldnt hold those back". `jobs.json` records what each stage did, for
//! `junctur status`; the stage results themselves are the source of truth.

mod adopt;
mod jobs;
mod speakers;
mod speech;
mod steps;

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub use jobs::{Jobs, StageRecord, StageState, STAGES};
pub use speakers::{hearing_key, seeded, speech_key, Found, Heard, Method, Pin};
pub use speech::{Speech, SpeechTrack};
pub use steps::{Step, Steps};

use crate::edit::{self, CutSettings, EditList, Speakers};
use crate::runtime::DevicePref;
use crate::tracks::{self, Role, TrackReport};
use crate::transcribe::{self, Languages, ModelPref, Source, TranscribeOptions, Transcript};
use crate::voices::{Voice, Voices};
use crate::{cache, hidden, Error, Result};

const ENVELOPE_FILE: &str = "envelope.f32";

/// There while a re-transcription asked for (5aa: the Correct button) has not finished: a run resumed
/// after the app closed then still hears the recording again and carries the fixes onto the new words
/// ([`hear_again`], [`carry_fixes`]). Empty; the transcript it replaces is kept beside it
/// ([`BEFORE_FILE`]).
const AGAIN_FILE: &str = "transcribe-again";

/// The transcript a re-transcription replaces, copied when it is asked for (5aa repair): the fixes are
/// read against it at the end, and `transcript.json` is the new one from the moment Whisper finishes,
/// so a run that stopped between that write and the carry would otherwise carry against the new words.
const BEFORE_FILE: &str = "transcript.before.json";

/// Fixes a re-transcription could not put on any word (5aa repair: Whisper heard something else there,
/// or nothing), as they were, for the user to see and redo: kept until the app is told they were seen
/// ([`forget_fixes_set_aside`]), each later run adding to them.
const SET_ASIDE_FILE: &str = "fixes-set-aside.json";

/// A fix a re-transcription set aside ([`SET_ASIDE_FILE`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAside {
    pub track: usize,
    /// Where its words started, in source seconds.
    pub at: f64,
    /// What they were heard as before.
    pub heard: String,
    /// What the user had fixed them to; for a word time, the word as fixed or heard.
    pub text: String,
    /// A word time set by moving a cut's edge (2w) that the re-transcription could not put on a
    /// word (5ad): the start and end the user had given it. `None` for a fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retimed: Option<[f64; 2]>,
}

/// The fixes re-transcriptions of `p` set aside and the user has not yet seen (5aa).
pub fn fixes_set_aside(p: &Project) -> Vec<SetAside> {
    std::fs::read(p.dir.join(SET_ASIDE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Forgets the fixes set aside for `p` that the user has seen (`seen`, as [`fixes_set_aside`] gave
/// them): only those (5ad: the whole file went, with any a run had added since the panel listed
/// them, unseen). Under the edit list's writer lock, as a run's additions are ([`keep_set_aside`]),
/// so neither read-modify-write lands inside the other.
pub fn forget_fixes_set_aside(p: &Project, seen: &[SetAside]) -> Result<()> {
    let _writing = edit::hold();
    let path = p.dir.join(SET_ASIDE_FILE);
    let left: Vec<SetAside> = fixes_set_aside(p)
        .into_iter()
        .filter(|s| !seen.contains(s))
        .collect();
    if !left.is_empty() {
        crate::output::write_replacing(&path, serde_json::to_vec_pretty(&left)?)?;
        return Ok(());
    }
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

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
        // 2f: a run started by a packaged app (Claude desktop) may have analysed this recording into
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
        self.dir.join("edit.junctur")
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

    /// `speakers.json`: `None` when there is none, an error when it is there but unreadable. Since
    /// 5c it holds the user's pins, so an unreadable one is not the same as none.
    fn speakers_file(&self) -> Result<Option<Found>> {
        match std::fs::read(self.speakers_path()) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn edit(&self) -> Result<EditList> {
        let text = std::fs::read_to_string(self.edit_path())?;
        edit::parse(&text).map_err(|e| Error::Edit(format!("{}: {e}", self.edit_path().display())))
    }
}

#[derive(Debug, Clone, Default)]
pub struct PipelineOptions {
    /// Where Whisper may run. Only transcription uses it: tracks, speech and speakers always run on
    /// the CPU (5r: CAM++ on the GPU found a speaker per sentence).
    pub device: DevicePref,
    pub model: ModelPref,
    /// The word list Whisper is told (5h; the caller reads the config file). Not part of what makes
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
    /// Enrolled voices to label speakers by (5b; the caller reads the voices file).
    pub voices: Vec<Voice>,
    /// Whether an edit list holding the user's edits is kept or re-cut ([`KeepList`]).
    pub keep_list: KeepList,
    /// Hear the speakers again from the audio, cut at the user's pins, instead of reusing the
    /// hearing (5aa: the Correct button; what 5c's [`rehear`] does outside a run).
    pub rehear: bool,
    /// Transcribe the recording again, the word list as it is now, and carry the edit list's fixes
    /// onto the new words ([`finish`], 5aa: the Correct button).
    pub retranscribe: bool,
}

/// What a run's `edit` stage does with an edit list on disk that holds the user's edits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeepList {
    /// Re-cut it, keeping it as `edit.junctur.bak*` (`junctur analyze`, and the app's "Start over
    /// with a fresh cut", which asks first).
    #[default]
    No,
    /// Keep it, taking only the speakers the run found; one that does not parse (a run killed
    /// mid-write before 1d, a broken hand edit) is backed up and re-cut, or the recording would
    /// never have a list (2j: the app resuming a run the user edited during).
    Edits,
    /// As `Edits`, and one that does not parse is left as it is too: the run fails saying so,
    /// since re-cutting is only ever the user's choice here (5s: the app's Re-analyse).
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
/// For the app's player, which only wants the roles when they are known (2e).
pub fn cached_tracks(p: &Project) -> Option<TrackReport> {
    tracks::cached(&p.file, &tracks_options(p, false))
}

/// Run every stage in order. `log` gets one line per event, for the CLI to print.
pub fn run(p: &Project, opts: &PipelineOptions, log: &mut dyn FnMut(&str)) -> Result<Outcome> {
    run_with(p, opts, log, steps::none())
}

/// [`run`], telling `steps` how far each track has got through the speech, speakers and
/// transcribe stages as they go (2h: the app's progress bars). [`prepare`], then [`finish`].
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
/// counts (2j: nothing else in it needs the transcript, so the recording is editable before Whisper
/// starts). With `opts.keep_list` a list holding the user's edits is kept, with this run's speakers
/// ([`KeepList`]).
pub fn prepare(
    p: &Project,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
    steps: Steps,
) -> Result<Prepared> {
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();

    let report = jobs.stage(&p.dir, "tracks", log, |_| {
        tracks::analyze(&p.file, &tracks_options(p, opts.force))
    })?;
    let speech = speech_stage(p, &mut jobs, &report, opts.force, log, steps)?;
    // Speakers are an extra, not core: when they cannot be found (the model cannot be fetched
    // offline, say), the run goes on without them and the edit list keeps the names it had.
    let speakers = if opts.no_speakers {
        None
    } else {
        match speakers_stage(p, &mut jobs, &report, &speech, opts, log, steps) {
            Ok(f) => Some(f),
            Err(e) => {
                log(&format!("speakers: {e}; going on without them"));
                None
            }
        }
    };
    let edit = jobs.stage(&p.dir, "edit", log, |log| {
        let d = draft(p, &report, &speech, speakers.as_ref(), None, &opts.cut);
        keep_or_write(p, d, speakers.as_ref(), opts.keep_list, log)
    })?;
    Ok(Prepared {
        report,
        speech,
        speakers,
        edit,
    })
}

/// A run's second half: Whisper over the speech, piece by piece (`transcribed` is told where the
/// transcript is whole, `between` is called between chunks and may block: the app's pause, 2k;
/// [`transcribe::Reports`]), then the words counted into the edit list's score windows in place
/// ([`count_words`]). Never re-cuts: edits made since [`prepare`] stay.
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
    let transcript = if opts.no_transcript {
        None
    } else {
        let again = hear_again(p, opts, log)?;
        let t = transcribe_stage(
            p,
            &mut jobs,
            &report,
            &speech,
            opts,
            again,
            log,
            steps,
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
    Ok(Outcome {
        report,
        speech,
        speakers,
        transcript,
        edit,
    })
}

/// The `edit` stage's write: unless `keep` is [`KeepList::No`], a list on disk holding the user's
/// edits is kept, only its speakers replaced by the ones this run found (5s: the app's Re-analyse
/// matches the speakers against the remembered voices again and puts them on top of the user's cut,
/// names, fixes and caption settings). A list that does not parse is re-cut with [`KeepList::Edits`]
/// and left, failing the stage, with [`KeepList::Whole`].
///
/// Whether the list is the user's is decided again, and the list written, under the edit-list writer
/// lock ([`Draft::recheck`], 5s repair): `d` was drafted outside it (seconds on a long recording),
/// and a save in between would otherwise be re-cut over without a backup.
///
/// The speakers go in through [`edit::amend`], as the word counts do: one writer at a time, an app
/// save saved again as the app would, and a list edited outside Junctur (comments, another layout)
/// left whole rather than rewritten for its speakers. They are worked out from the list as amend reads
/// it ([`speakers_for`]), so its names and colours carry over by what each speaker says.
fn keep_or_write(
    p: &Project,
    d: Draft,
    found: Option<&Found>,
    keep: KeepList,
    log: &mut dyn FnMut(&str),
) -> Result<EditList> {
    let _writing = edit::hold();
    let d = d.recheck(p);
    if keep != KeepList::No && d.users {
        let amended = edit::amend(&p.edit_path(), |e| {
            if found.is_some() {
                e.speakers = speakers_for(found, Some(&e.speakers));
            }
        })?;
        match amended {
            edit::Amended::Written(e) => {
                log("edit list: it holds your edits, kept with the speakers matched again");
                return Ok(e);
            }
            edit::Amended::Same(e) => {
                log("edit list: it holds your edits, kept as it is");
                return Ok(e);
            }
            edit::Amended::Left => match p.edit() {
                Ok(kept) => {
                    log(
                        "edit list: edited outside Junctur, kept as it is with the speakers it had",
                    );
                    return Ok(kept);
                }
                Err(e) if keep == KeepList::Whole => {
                    return Err(Error::Edit(format!(
                        "{e}. It is left as it is: fix it, or start over with a fresh cut, which keeps it as a backup"
                    )));
                }
                Err(_) => {}
            },
            edit::Amended::Missing => {}
        }
    }
    write_draft(p, d)
}

/// Whether this run's transcription hears the recording again (5aa): asked for now
/// (`opts.retranscribe`, which leaves [`AGAIN_FILE`]), or asked for by an earlier run that stopped
/// before its first chunk (the file is there, no partial to resume), so a reopened recording does not
/// settle for the old transcript. A run stopped later resumes its partial as any run does.
///
/// An earlier ask whose carry never ran (5ad) keeps its copy of the old transcript when Correct is
/// asked again: `transcript.json` may be that run's new transcript by then, and the fixes are still on
/// the old one's words. And a run that finds that ask finished (the mark, no partial, and a
/// `transcript.json` that is not the copy: Whisper wrote the new one, then the app stopped before the
/// carry) does not hear the recording again: it carries.
fn hear_again(p: &Project, opts: &PipelineOptions, log: &mut dyn FnMut(&str)) -> Result<bool> {
    let mark = p.dir.join(AGAIN_FILE);
    if opts.retranscribe {
        if mark.is_file() {
            log("transcribing again with the word list as it is now (the fixed words still on the transcript before the last ask)");
            return Ok(true);
        }
        // The old transcript first, then the mark: a mark is never there without its copy.
        let before = p.dir.join(BEFORE_FILE);
        match std::fs::read(transcribe::path(&p.dir)) {
            Ok(b) => crate::output::write_replacing(&before, b)?,
            Err(_) => {
                let _ = std::fs::remove_file(&before);
            }
        }
        if let Err(e) = crate::output::write_replacing(&mark, b"") {
            // Nor a copy without its mark (5ad): nothing would ever remove it.
            let _ = std::fs::remove_file(&before);
            return Err(e.into());
        }
        log("transcribing again with the word list as it is now");
        return Ok(true);
    }
    if !mark.is_file() || transcribe::interrupted(&p.dir) {
        return Ok(false);
    }
    if heard_again_already(p) {
        log("the transcription asked for again had finished: its fixed words are carried now");
        return Ok(false);
    }
    Ok(true)
}

/// A re-transcription asked for has written its transcript (5ad): `transcript.json` is there and is
/// not the copy of the one it replaces (or there was none to copy).
fn heard_again_already(p: &Project) -> bool {
    let Ok(now) = std::fs::read(transcribe::path(&p.dir)) else {
        return false;
    };
    std::fs::read(p.dir.join(BEFORE_FILE)).map_or(true, |before| before != now)
}

/// A transcription that failed (5aa repair): a re-transcription asked for is forgotten when nothing of
/// it is left to resume, or every later run of the recording would hear it again from the start. One
/// with a partial keeps its mark: the next run resumes it, and its end must still carry the fixes.
fn not_again(p: &Project, log: &mut dyn FnMut(&str)) {
    if transcribe::interrupted(&p.dir) || !p.dir.join(AGAIN_FILE).is_file() {
        return;
    }
    for f in [AGAIN_FILE, BEFORE_FILE] {
        let path = p.dir.join(f);
        if let Err(e) = crate::output::remove_retrying(&path) {
            if path.exists() {
                log(&format!("could not remove {}: {e}", path.display()));
            }
        }
    }
    log("the transcription again stopped before its first chunk: asked for no longer");
}

/// After a re-transcription (5aa, [`AGAIN_FILE`] there): the edit list's fixes, read against the
/// transcript it replaced ([`BEFORE_FILE`]: every fix made before, and during the run on a word it
/// shares with the old one), are moved onto the new words ([`edit::carry`]), and so are the words the
/// user retimed by moving a cut's edge (5ad, [`edit::carry_times`]); then every fix is spread to the
/// words heard alike ([`edit::spread`]), in the edit list on disk through [`edit::amend`] as the word
/// counts go. Read at the end rather than written down at the start, so a word the user kept as heard
/// while it ran ("Undo those") is kept. A fix or time put on no word is set aside ([`SET_ASIDE_FILE`]),
/// never lost. The mark and the copy go in every case, the log saying when the list could not take
/// them: left behind, they would ask for another re-transcription.
///
/// Not carried, and said (5ad): with no copy of the old transcript (missing or unreadable, so the
/// fixes' words cannot be found), or a new transcript with no words at all (a run that heard nothing).
/// The fixes and times are then left as they are rather than spread from keys that name other words.
fn carry_fixes(p: &Project, t: &Transcript, log: &mut dyn FnMut(&str)) {
    let mark = p.dir.join(AGAIN_FILE);
    if !mark.is_file() {
        return;
    }
    // The set-aside file is written under the same lock as the list (5ad), so the app's "seen"
    // cannot land between its read and its write.
    let _writing = edit::hold();
    let old = std::fs::read(p.dir.join(BEFORE_FILE))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Transcript>(&b).map_err(|e| e.to_string()));
    let words = t.in_order();
    match old {
        Err(e) => log(&format!(
            "the transcript before was not kept ({e}): fixed words and word times are left where they were"
        )),
        Ok(_) if words.is_empty() => log(
            "the new transcript has no words: fixed words and word times are left where they were",
        ),
        Ok(old) => carry_onto(p, &old.in_order(), &words, log),
    }
    for f in [AGAIN_FILE, BEFORE_FILE] {
        let path = p.dir.join(f);
        if let Err(e) = crate::output::remove_retrying(&path) {
            if path.exists() {
                log(&format!("could not remove {}: {e}", path.display()));
            }
        }
    }
}

/// What [`carry_all`] did to a list.
#[derive(Default)]
struct Carry {
    fixes: edit::CarryReport,
    times: edit::TimesReport,
    spread: edit::Spread,
}

/// How much of a carry the list could take ([`carry_onto`]).
#[derive(PartialEq)]
enum Took {
    /// Carried and spread.
    All,
    /// Carried; the spread would not have read back.
    Carried,
    /// Nothing: left as it was.
    Nothing,
}

/// The fixes and retimes of `e` moved from `before`'s words onto `words`, every fix spread after
/// when `spread`.
fn carry_all(
    e: &mut EditList,
    before: &[(usize, &transcribe::Word)],
    words: &[(usize, &transcribe::Word)],
    spread: bool,
) -> Carry {
    // The retimes are read with the fixes as they are on the old words (a word heard now as its fix
    // is the same word).
    let timed = edit::snapshot_times(&e.times, &e.fixes, before);
    let carried = edit::snapshot(&e.fixes, before);
    let mut done = Carry {
        fixes: edit::carry(&mut e.fixes, &carried, words),
        times: edit::carry_times(&mut e.times, &timed, words),
        ..Carry::default()
    };
    if spread {
        done.spread = edit::spread(&mut e.fixes, words, edit::Which::All);
    }
    done
}

/// [`carry_fixes`]' work on the list, `before` and `words` the old and new transcripts. A result the
/// parser would refuse is not written ([`EditList::checked_text`]: two fixes on one word, a text too
/// long): the carry is tried without the spread, its set-aside kept (5ad: the fallback restored the
/// fixes and dropped what the carry had set aside, unlisted); and when that would not read back
/// either, the fixes and times stay as they were and nothing is set aside, nothing having been taken
/// out.
fn carry_onto(
    p: &Project,
    before: &[(usize, &transcribe::Word)],
    words: &[(usize, &transcribe::Word)],
    log: &mut dyn FnMut(&str),
) {
    let mut done = Carry::default();
    let mut took = Took::Nothing;
    let amended = edit::amend(&p.edit_path(), |e| {
        let was = (e.fixes.clone(), e.times.clone());
        for (spread, how) in [(true, Took::All), (false, Took::Carried)] {
            let d = carry_all(e, before, words, spread);
            if e.checked_text().is_ok() {
                (done, took) = (d, how);
                return;
            }
            (e.fixes, e.times) = was.clone();
        }
    });
    match amended {
        Ok(edit::Amended::Written(_) | edit::Amended::Same(_)) if took == Took::Nothing => log(
            "edit list: carrying the fixed words would give a list that does not read back; they are left as they were",
        ),
        Ok(edit::Amended::Written(_) | edit::Amended::Same(_)) => {
            if took == Took::Carried {
                log("edit list: spreading the fixed words would give a list that does not read back; carried without spreading");
            }
            log(&carried_line(&done));
            for g in &done.fixes.gone {
                log(&format!(
                    "fixed word set aside: \"{}\" (heard \"{}\") at {} is heard otherwise now",
                    g.fix.text,
                    g.heard.join(" "),
                    clock(g.from)
                ));
            }
            for g in &done.times.gone {
                log(&format!(
                    "word time set aside: \"{}\" at {} is heard otherwise now",
                    g.heard,
                    clock(g.start)
                ));
            }
            if let Err(e) = keep_set_aside(p, &done.fixes.gone, &done.times.gone) {
                log(&format!("{SET_ASIDE_FILE} not written ({e})"));
            }
        }
        Ok(edit::Amended::Left) => {
            log("edit list: edited outside Junctur, its fixed words are left as they are")
        }
        Ok(edit::Amended::Missing) => {}
        Err(e) => log(&format!("edit list: fixed words not carried ({e})")),
    }
}

/// Adds `gone` and `times` to the fixes set aside for `p` ([`SET_ASIDE_FILE`]). Called under the
/// edit list's writer lock ([`carry_fixes`]), as [`forget_fixes_set_aside`] is.
fn keep_set_aside(p: &Project, gone: &[edit::Carried], times: &[edit::CarriedTime]) -> Result<()> {
    if gone.is_empty() && times.is_empty() {
        return Ok(());
    }
    let mut all = fixes_set_aside(p);
    all.extend(gone.iter().map(|g| SetAside {
        track: g.fix.track,
        at: g.from,
        heard: g.heard.join(" "),
        text: g.fix.text.clone(),
        retimed: None,
    }));
    all.extend(times.iter().map(|g| SetAside {
        track: g.time.track,
        at: g.start,
        heard: g.heard.clone(),
        text: g.fixed.clone().unwrap_or_else(|| g.heard.clone()),
        retimed: Some([g.time.from, g.time.to]),
    }));
    crate::output::write_replacing(
        &p.dir.join(SET_ASIDE_FILE),
        serde_json::to_vec_pretty(&all)?,
    )?;
    Ok(())
}

/// The log line on fixes carried onto a new transcript.
fn carried_line(c: &Carry) -> String {
    let (r, s, t) = (&c.fixes, &c.spread, &c.times);
    let mut line = format!(
        "fixed words: {} kept on their words, {} no longer needed, {} set aside; {} more place(s) fixed",
        r.kept,
        r.right,
        r.gone.len(),
        s.added.len()
    );
    if !s.unclear.is_empty() {
        line.push_str(&format!(
            "; fixed two ways, left as heard: {}",
            s.unclear.join(", ")
        ));
    }
    if t.kept + t.right + t.gone.len() > 0 {
        line.push_str(&format!(
            "; word times: {} kept on their words, {} no longer needed, {} set aside",
            t.kept,
            t.right,
            t.gone.len()
        ));
    }
    line
}

/// `h:mm:ss` of source seconds, for the log.
fn clock(s: f64) -> String {
    let t = s.max(0.0) as u64;
    format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

/// Puts the transcript's word counts into the score windows of the edit list on disk, changing
/// nothing else, when rewriting it loses nothing of the user's ([`edit::amend`]); the list on disk
/// afterwards, when it reads.
fn count_words(
    p: &Project,
    speech: &Speech,
    t: &Transcript,
    log: &mut dyn FnMut(&str),
) -> Result<Option<EditList>> {
    let regions = speech.all_regions();
    let starts: Vec<f64> = t.words().map(|w| w.start).collect();
    let amended = edit::amend(&p.edit_path(), |e| {
        e.windows = edit::windows(&regions, &starts, e.duration, e.settings.window);
    })?;
    Ok(match amended {
        edit::Amended::Written(e) | edit::Amended::Same(e) => Some(e),
        edit::Amended::Left => {
            log("edit list: edited outside Junctur, its word counts are left as they are");
            None
        }
        edit::Amended::Missing => None,
    })
}

/// Speech regions and the cut envelope on every speech track, reused unless `force`.
fn speech_stage(
    p: &Project,
    jobs: &mut Jobs,
    report: &TrackReport,
    force: bool,
    log: &mut dyn FnMut(&str),
    steps: Steps,
) -> Result<Speech> {
    let sources = speech_tracks(report);
    if sources.is_empty() {
        log("no track carries speech: the edit list will be empty");
    }
    let speech = match p.speech().filter(|_| !force) {
        Some(s) if p.envelope_path().is_file() => {
            jobs.skip(&p.dir, "speech", log)?;
            s
        }
        _ => jobs.stage(&p.dir, "speech", log, |_| {
            let read = |track, at| steps(steps::step(report, "speech", track, at));
            let (s, env) = speech::detect(&p.file, &sources, &p.cache_dir, &read)?;
            speech::save_envelope(&p.envelope_path(), &env)?;
            std::fs::write(p.speech_path(), serde_json::to_vec_pretty(&s)?)?;
            Ok(s)
        })?,
    };
    for t in &speech.tracks {
        log(&format!(
            "track {}: speech {:.0}% of the time, {} regions",
            t.track + 1,
            t.share * 100.0,
            t.regions.len()
        ));
    }
    Ok(speech)
}

/// Who talks when, reused unless `force` or the speech regions it was found from changed.
fn speakers_stage(
    p: &Project,
    jobs: &mut Jobs,
    report: &TrackReport,
    speech: &Speech,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
    steps: Steps,
) -> Result<Found> {
    // An unreadable file holds pins (5c) nothing else knows: kept aside under another name and
    // said, not silently replaced by a file without them.
    let previous = match p.speakers_file() {
        Ok(f) => f,
        Err(e) => {
            let kept = set_aside(&p.speakers_path())?;
            log(&format!(
                "speakers.json could not be read ({e}): kept as {}; the seconds pinned to people in it are not used",
                kept.display()
            ));
            None
        }
    };
    let cached = previous
        .clone()
        .filter(|_| !opts.force)
        .filter(|_| !opts.rehear)
        .and_then(|f| speakers::cached(f, speech, &voice_tracks(report), &p.dir));
    let found = match cached {
        // Heard before: only labelled again, with the voices as they are now (cheap).
        Some((f, embeddings)) => {
            jobs.skip(&p.dir, "speakers", log)?;
            let again = speakers::label(&f, &embeddings, &opts.voices);
            if again != f {
                write_speakers(p, &again)?;
            }
            again
        }
        None => jobs.stage(&p.dir, "speakers", log, |_| {
            // The user's pins (5c) cut the windows and outlive the hearing they were made on.
            let pins = previous
                .as_ref()
                .map(|f| f.pins.clone())
                .unwrap_or_default();
            let mut f = speakers::find(
                &p.file,
                speech,
                &voice_tracks(report),
                &opts.voices,
                &pins,
                &p.dir,
                &p.cache_dir,
                &mut |track, at| steps(steps::step(report, "speakers", track, at)),
            )?;
            // Heard again: the voice names the old labels knew still are the voices', not
            // names for whatever cluster now sits there.
            if let Some(old) = &previous {
                speakers::remember_known(&mut f, old);
            }
            write_speakers(p, &f)?;
            Ok(f)
        })?,
    };
    log(&describe(&found));
    Ok(found)
}

/// Remembers `name`'s voice from `stretches` of this recording (source seconds; 5b): the cached
/// windows there, without overlapping talk, are averaged into a voiceprint added to `voices`, and
/// the project's speakers are labelled with it. Nothing is written: the caller saves the voices
/// file first and then the speakers ([`save_speakers`]), so a failure on the way leaves the old
/// voices with the old labels, and new labels never stand on voices that were not saved. The edit
/// list is the caller's to update ([`speakers_for`], [`respeak`]).
///
/// Since 5c any stretch will do, one word included ([`speakers::voice_sample`]'s fallbacks, then
/// the audio itself: [`speakers::embed_stretches`]), and the stretches are pinned to `name`
/// ([`Found::pin`]): labelled as that person from now on, whatever the windows there sound like.
/// Returns the speakers and the seconds of speech this added to the voice.
pub fn enroll(
    p: &Project,
    voices: &mut Voices,
    name: &str,
    stretches: &[(f64, f64)],
) -> Result<(Found, f64)> {
    let mut found = p
        .speakers()
        .ok_or_else(|| Error::Edit("run `junctur analyze` on this file first".into()))?;
    let (mut sample, mut seconds) = speakers::voice_sample(&found, &p.dir, stretches)?;
    if sample.is_empty() {
        let track = found.heard.as_ref().map_or(0, |h| h.track);
        (sample, seconds) = speakers::embed_stretches(&p.file, track, stretches, &p.cache_dir)?;
    }
    let refs: Vec<&[f32]> = sample.iter().map(Vec::as_slice).collect();
    let mut changed = voices.clone();
    changed.enroll(name, &refs, seconds)?;
    let named = changed.get(name).map_or(name, |v| v.name.as_str());
    found.pin(named, stretches);
    let usable: Vec<Voice> = changed.usable().into_iter().cloned().collect();
    let again = speakers::relabel(&found, &p.dir, &usable)?;
    *voices = changed;
    Ok((again, seconds))
}

/// Hears the project's speakers again from the audio (5c: "Analyse speakers again"), the windows
/// cut at the pins the user made ([`speakers::seeded`]), and labels them with `voices`; the other
/// stages' results are reused, never rerun (they need nothing new: `--force` would redo Whisper
/// for minutes). Saves `speakers.json`; the edit list is the caller's ([`respeak`]).
pub fn rehear(p: &Project, voices: &[Voice], log: &mut dyn FnMut(&str)) -> Result<Found> {
    let missing = || Error::Edit("run `junctur analyze` on this file first".into());
    let speech = p.speech().ok_or_else(missing)?;
    let report = tracks::analyze(
        &p.file,
        &tracks::AnalyzeOptions {
            cache_dir: Some(p.cache_dir.clone()),
            ..Default::default()
        },
    )?;
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();
    let opts = PipelineOptions {
        force: true,
        voices: voices.to_vec(),
        ..Default::default()
    };
    speakers_stage(p, &mut jobs, &report, &speech, &opts, log, steps::none())
}

/// The project's speakers labelled again with `voices` from what the stage heard (no audio),
/// without saving them.
pub fn labelled(p: &Project, voices: &[Voice]) -> Result<Found> {
    let found = p
        .speakers()
        .ok_or_else(|| Error::Edit("run `junctur analyze` on this file first".into()))?;
    speakers::relabel(&found, &p.dir, voices)
}

/// [`labelled`] after a remembered person was renamed `from` -> `to` in `voices`: the seconds
/// pinned to them (5c) follow the new name. Speakers heard by an older Junctur (5r) cannot be
/// labelled again, but their pins still follow the name, so that the next hearing gives those
/// seconds to the renamed person instead of dropping them; the rest is kept as it is.
pub fn labelled_renamed(p: &Project, voices: &[Voice], from: &str, to: &str) -> Result<Found> {
    let mut found = p
        .speakers()
        .ok_or_else(|| Error::Edit("run `junctur analyze` on this file first".into()))?;
    found.rename_pins(from, to);
    if found.heard_by_an_older_junctur() {
        return Ok(found);
    }
    speakers::relabel(&found, &p.dir, voices)
}

/// Writes `speakers.json` whole through a temporary file of its own renamed over it (retried
/// briefly when a reader holds it on Windows, like the voices file): since 5c it holds the user's
/// pins, and a write torn by a crash would lose them.
fn write_speakers(p: &Project, found: &Found) -> Result<()> {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = p.speakers_path();
    let tmp = path.with_extension(format!("json.{}-{n}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(found)?)?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, &path) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
        }
    }
}

/// Renames `path` to the first free `<name>.unreadable`, `<name>.unreadable2`, ... beside it;
/// returns the new path.
fn set_aside(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let free = (1..)
        .map(|n| match n {
            1 => path.with_file_name(format!("{name}.unreadable")),
            n => path.with_file_name(format!("{name}.unreadable{n}")),
        })
        .find(|p| !p.exists())
        .expect("some name is free");
    std::fs::rename(path, &free)?;
    Ok(free)
}

/// Saves `found` as the project's speakers (`speakers.json`), when it differs from what is there.
pub fn save_speakers(p: &Project, found: &Found) -> Result<()> {
    if p.speakers().as_ref() != Some(found) {
        write_speakers(p, found)?;
    }
    Ok(())
}

/// Puts `found`'s speakers into the project's edit list in place of the ones it names, keeping
/// everything else (kept ranges, captions, the names of speakers who are not enrolled voices), and
/// saves it as the app's editor does ([`edit::save`]: atomic, a hand edit backed up first) when
/// that changes it. For the CLI after enrolling. `None` when there is no readable edit list;
/// else the list and where a replaced hand edit was kept, if one was.
pub fn respeak(p: &Project, found: &Found) -> Result<Option<(EditList, Option<PathBuf>)>> {
    // From the read to the save (2m): word counts the app's own analysis writes meanwhile
    // ([`edit::amend`], 2j) would otherwise be saved over.
    let _writing = edit::hold();
    let Ok(mut e) = p.edit() else {
        return Ok(None);
    };
    let speakers = speakers_for(Some(found), Some(&e.speakers));
    let mut backup = None;
    if speakers != e.speakers {
        e.speakers = speakers;
        backup = edit::save(&p.edit_path(), &e)?.backup;
    }
    Ok(Some((e, backup)))
}

/// One line on what the speakers stage found, for the log.
pub fn describe(found: &Found) -> String {
    match found.method {
        Method::Tracks => format!("{} speakers, one per speech track", found.tracks.len()),
        Method::Voices => {
            let known: Vec<&str> = found
                .people
                .iter()
                .flatten()
                .map(|p| p.name.as_str())
                .collect();
            let overlaps = found.heard.as_ref().map_or(0, |h| h.overlaps.len());
            format!(
                "{} speaker(s) told apart by voice{}, {} turns, {overlaps} stretches of two talking at once",
                found.tracks.len(),
                if known.is_empty() {
                    String::new()
                } else {
                    format!(" (recognised: {})", known.join(", "))
                },
                found.turns.len()
            )
        }
        Method::None => "no speakers: no track carries speech".into(),
    }
}

/// Labels the project's speakers again with `voices` (5b: after enrolling or forgetting one), from
/// what the stage heard, and saves the result; no audio is read. The edit list is not touched: the
/// caller puts the new speakers into the list it holds ([`speakers_for`]).
pub fn relabel(p: &Project, voices: &[Voice]) -> Result<Found> {
    let again = labelled(p, voices)?;
    save_speakers(p, &again)?;
    Ok(again)
}

/// Whisper over the speech regions, resumed or reused when it can be.
#[allow(clippy::too_many_arguments)]
fn transcribe_stage(
    p: &Project,
    jobs: &mut Jobs,
    report: &TrackReport,
    speech: &Speech,
    opts: &PipelineOptions,
    again: bool,
    log: &mut dyn FnMut(&str),
    steps: Steps,
    transcribed: &mut dyn FnMut(f64),
    between: &mut dyn FnMut(),
) -> Result<Transcript> {
    let t_opts = TranscribeOptions {
        model: opts.model,
        device: opts.device,
        languages: opts.languages.clone(),
        force: opts.force,
        again,
        words: opts.words.clone(),
    };
    let src: Vec<Source> = speech
        .tracks
        .iter()
        .map(|t| Source {
            track: t.track,
            regions: t.regions.clone(),
        })
        .collect();
    jobs.stage(&p.dir, "transcribe", log, |log| {
        transcribe::transcribe(
            &p.file,
            &src,
            report.duration,
            &p.dir,
            &p.cache_dir,
            &t_opts,
            log,
            transcribe::Reports {
                reach: &mut |track, at| steps(steps::step(report, "transcribe", track, at)),
                transcribed: &mut *transcribed,
                between: &mut *between,
            },
        )
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

/// The speech tracks whose role is a single voice (a mic, a Discord), not a mix.
fn voice_tracks(report: &TrackReport) -> Vec<usize> {
    report
        .logical
        .iter()
        .filter(|l| l.role == Role::Voice)
        .map(|l| l.members[0])
        .collect()
}

/// (Re)write the edit list from cached analysis. A hand-edited list is kept as a backup before
/// being replaced ([`back_up`]). Speaker names already in the list are kept by id, so a re-cut never
/// undoes a rename; without `speakers` (skipped, or the stage failed) the list keeps the speakers it
/// had.
pub fn cut(
    p: &Project,
    report: &TrackReport,
    speech: &Speech,
    speakers: Option<&Found>,
    transcript: Option<&Transcript>,
    settings: &CutSettings,
) -> Result<EditList> {
    write_draft(p, draft(p, report, speech, speakers, transcript, settings))
}

/// Writes a re-cut, backing up the list it replaces when that is the user's: decided again from the
/// list on disk under the edit-list writer lock ([`Draft::recheck`]).
fn write_draft(p: &Project, d: Draft) -> Result<EditList> {
    let _writing = edit::hold();
    let d = d.recheck(p);
    if let (Some(old), true) = (&d.old, d.users) {
        back_up(&p.dir, old)?;
    }
    // Whole or not at all: a list cut short by a close would open as an analysed recording whose
    // list does not parse. And only a list that reads back (5ad), as every edit-list writer.
    let text = d.list.checked_text().map_err(Error::Edit)?;
    crate::output::write_replacing(&p.edit_path(), text)?;
    Ok(d.list)
}

/// A re-cut not yet written: the new list, the text on disk it replaces, and whether that text is
/// the user's (a re-cut backs it up).
struct Draft {
    list: EditList,
    old: Option<String>,
    users: bool,
}

impl Draft {
    /// The draft against the list on disk now. Called under [`edit::hold`]: a list written since
    /// it was drafted (an app save, which takes the same lock) is the user's unless it is the very
    /// list this draft would write, so a re-cut backs it up instead of losing it, and never backs up
    /// the stale text it was drafted from instead.
    fn recheck(mut self, p: &Project) -> Draft {
        let now = std::fs::read_to_string(p.edit_path()).ok();
        if now != self.old {
            self.users = now
                .as_deref()
                .is_some_and(|t| edit::unstamped(t) != self.list.to_text());
            self.old = now;
        }
        self
    }
}

/// What [`cut`] would write, and whether it would back up what is there.
fn draft(
    p: &Project,
    report: &TrackReport,
    speech: &Speech,
    speakers: Option<&Found>,
    transcript: Option<&Transcript>,
    settings: &CutSettings,
) -> Draft {
    let env = speech::load_envelope(&p.envelope_path()).unwrap_or_default();
    let regions = speech.all_regions();
    let starts: Vec<f64> = transcript
        .map(|t| t.words().map(|w| w.start).collect())
        .unwrap_or_default();
    let old = std::fs::read_to_string(p.edit_path()).ok();
    let parsed = old.as_deref().and_then(|t| edit::parse(t).ok());
    let previous = parsed.as_ref().map(|e| e.speakers.clone());
    // Fixed words too (5h): a re-cut changes what is kept, not what a word says.
    let fixes = parsed.as_ref().map_or_else(Vec::new, |e| e.fixes.clone());
    // And retimed words (2w): where a word is said is not what a re-cut decides.
    let times = parsed.as_ref().map_or_else(Vec::new, |e| e.times.clone());
    // Caption settings are the user's, like the names: a re-cut keeps them.
    // None stays None: a list that followed the user's defaults still does (2i).
    let captions = parsed.and_then(|e| e.captions);
    let speakers = speakers_for(speakers, previous.as_ref());
    let make = |settings: &CutSettings, starts: &[f64]| EditList {
        source: p.file.clone(),
        duration: report.duration,
        settings: *settings,
        keep: edit::keep_ranges(&regions, &env, report.duration, settings),
        speakers: speakers.clone(),
        captions: captions.clone(),
        fixes: fixes.clone(),
        times: times.clone(),
        windows: edit::windows(&regions, starts, report.duration, settings.window),
    };
    let list = make(settings, &starts);
    // The finished transcript's word starts, read only when a comparison needs them (a 30 h
    // transcript is tens of MB).
    let finished: OnceCell<Vec<f64>> = OnceCell::new();
    let users = old.as_deref().is_some_and(|old| {
        // A list written before speakers existed (or with `--no-speakers`) is compared as Junctur
        // would have written it then, without them: gaining speakers is not a reason to back it up.
        let had_speakers = previous.as_ref().is_some_and(|s| !s.is_empty());
        old != list.to_text()
            && !is_generated(old, |s| {
                // As this call would write it, and as a run writes it before its transcript (2j:
                // no word counts) and once it is done (the finished transcript's).
                let done = finished.get_or_init(|| {
                    transcribe::load(&p.dir)
                        .map(|t| t.words().map(|w| w.start).collect())
                        .unwrap_or_default()
                });
                [&starts[..], &[], &done[..]]
                    .into_iter()
                    .map(|w| {
                        let mut e = make(s, w);
                        if !had_speakers {
                            e.speakers = Speakers::default();
                        }
                        e.to_text()
                    })
                    .collect()
            })
    });
    Draft { list, old, users }
}

/// Whether the edit list on disk holds anything of the user's: a re-run of the pipeline (whose
/// `edit` stage re-cuts) would back it up and replace it. Drafted from the cached analysis with the
/// list's own settings; nothing is written. False without a list; true for one that does not parse
/// or whose analysis is gone. For the app to ask before a re-analysis (2d).
pub fn hand_edited(p: &Project) -> Result<bool> {
    let Ok(old) = std::fs::read_to_string(p.edit_path()) else {
        return Ok(false);
    };
    let Ok(list) = edit::parse(edit::unstamped(&old)) else {
        return Ok(true);
    };
    let Some(speech) = p.speech() else {
        return Ok(true);
    };
    let report = tracks::analyze(&p.file, &tracks_options(p, false))?;
    let transcript = transcribe::load(&p.dir);
    let speakers = p.speakers();
    let d = draft(
        p,
        &report,
        &speech,
        speakers.as_ref(),
        transcript.as_ref(),
        &list.settings,
    );
    Ok(d.users)
}

/// Why a re-run with `opts` would redo more than the cut, one line each for the user (2d): no
/// transcript yet, or one made with another model or languages than `opts` asks. Empty when the
/// analysis answers `opts`; `Auto` takes any model, as [`run`] does, so this never offers a re-run
/// that would redo nothing.
pub fn outdated(p: &Project, opts: &PipelineOptions) -> Vec<String> {
    if opts.no_transcript {
        return Vec::new();
    }
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

/// The speakers a re-cut writes: found ones named as `previous` named them, or with nothing
/// found (skipped, or the stage failed) the ones `previous` had, so neither erases a rename.
pub fn speakers_for(found: Option<&Found>, previous: Option<&Speakers>) -> Speakers {
    match found {
        Some(f) => Speakers::found(f.turns.clone(), &f.tracks, &f.people, previous, &f.known),
        None => previous.cloned().unwrap_or_default(),
    }
}

/// Whether `old`, the edit list about to be replaced, is Junctur's own: `regenerate` remakes a list
/// from the current analysis, and a list it reproduces from its own recorded settings has nothing of
/// the user's in it. An untouched app save is compared without its stamp: a list the app only
/// renamed a speaker in is reproduced by the re-cut (names are carried over). Anything else (a hand
/// edit, a list that no longer parses, one made from older analysis) is the user's.
fn is_generated(old: &str, regenerate: impl Fn(&CutSettings) -> Vec<String>) -> bool {
    let body = edit::unstamped(old);
    edit::parse(body).is_ok_and(|e| regenerate(&e.settings).iter().any(|t| t == body))
}

/// Keeps `old` at the first free `edit.junctur.bak`, `edit.junctur.bak2`, ... Backups are never
/// overwritten: a single `.bak` rewritten on every re-cut lost the hand edit to the next generated
/// list (1d).
fn back_up(dir: &Path, old: &str) -> Result<()> {
    let free = (1..)
        .map(|n| match n {
            1 => dir.join("edit.junctur.bak"),
            n => dir.join(format!("edit.junctur.bak{n}")),
        })
        .find(|p| !p.exists())
        .expect("some name is free");
    std::fs::write(free, old)?;
    Ok(())
}

/// Re-cut from what earlier stages left, without running them.
pub fn recut(p: &Project, settings: &CutSettings) -> Result<EditList> {
    let missing = || Error::Edit("run `junctur analyze` on this file first".into());
    let speech = p.speech().ok_or_else(missing)?;
    let report = tracks::analyze(&p.file, &tracks_options(p, false))?;
    let transcript = transcribe::load(&p.dir);
    let t0 = Instant::now();
    let e = cut(
        p,
        &report,
        &speech,
        p.speakers().as_ref(),
        transcript.as_ref(),
        settings,
    )?;
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();
    jobs.set(
        &p.dir,
        "edit",
        StageState::Done,
        Some(t0.elapsed().as_secs_f64()),
        None,
    )?;
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The code of every `.rs` file under `src/<rel>` (a folder or one file), without its
    /// `mod tests`, CRLF read as LF.
    fn code_under(rel: &str) -> Vec<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut todo = vec![root.join(rel)];
        let mut out = Vec::new();
        while let Some(at) = todo.pop() {
            if at.is_dir() {
                todo.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
            } else if at.extension().is_some_and(|x| x == "rs") {
                let src = std::fs::read_to_string(&at).unwrap().replace("\r\n", "\n");
                let code = match src.find("#[cfg(test)]\nmod tests") {
                    Some(k) => src[..k].to_string(),
                    None => src,
                };
                let name = at.strip_prefix(&root).unwrap().display().to_string();
                out.push((name, code));
            }
        }
        out
    }

    /// 5r: tracks, speech and speakers open every model on the CPU; only Whisper may use the GPU
    /// (Uezar: "GPU only for the transcribing part"). CAM++ on ONNX Runtime's CUDA provider made a
    /// speaker of every sentence. The stages' code (every file in their folders, so a new one is
    /// read too) may open a model only through [`runtime::open_on_cpu`], and may not name a device
    /// at all; this file, which runs them, hands the run's device to transcription only.
    #[test]
    fn tracks_speech_and_speakers_open_their_models_on_the_cpu_only() {
        let mut stages = Vec::new();
        for rel in [
            "tracks",
            "diarize",
            "vad.rs",
            "pipeline/speech.rs",
            "pipeline/speakers.rs",
        ] {
            stages.extend(code_under(rel));
        }
        assert!(stages.len() >= 9, "{}", stages.len());
        let mut opened = 0;
        for (name, code) in &stages {
            for banned in [
                "DevicePref",
                "Device::",
                "runtime::open(",
                "runtime::open_with_threads",
                "Session::builder",
                "ort::ep",
            ] {
                assert!(!code.contains(banned), "{name} has {banned}");
            }
            opened += code.matches("runtime::open_on_cpu(").count();
        }
        // The VAD's, the segmenter's and the embedder's.
        assert!(opened >= 3, "{opened}");
        let (_, this) = code_under("pipeline/mod.rs").remove(0);
        for banned in [
            "runtime::open",
            "Session::builder",
            "ort::ep",
            "DevicePref::Auto",
        ] {
            assert!(!this.contains(banned), "pipeline/mod.rs has {banned}");
        }
        // Every `device` field this file writes is transcription's (its own options' declaration
        // aside), and the run's device is only ever written into one.
        let mut fields = 0;
        for (k, _) in this.match_indices("device:") {
            let before = &this[..k];
            let opened = &before[..before.rfind(" {").expect("a struct")];
            let owner = opened.rsplit([' ', '(', '=']).next().unwrap();
            if !opened.ends_with("pub struct PipelineOptions") {
                assert_eq!(owner, "TranscribeOptions", "a device field at byte {k}");
                fields += 1;
            }
        }
        assert!(fields > 0);
        for (k, _) in this.match_indices(".device") {
            let line = this[..k].rsplit('\n').next().unwrap();
            assert!(line.trim_start().starts_with("device: "), "{line}.device");
        }
    }

    fn list(settings: CutSettings, keep_end: f64) -> String {
        EditList {
            source: PathBuf::from("in.mp4"),
            duration: 10.0,
            settings,
            keep: vec![edit::Range {
                start: 0.0,
                end: keep_end,
            }],
            speakers: Default::default(),
            captions: Default::default(),
            fixes: Vec::new(),
            times: Vec::new(),
            windows: vec![],
        }
        .to_text()
    }

    /// What [`cut`] does with the list it replaces.
    fn back_up_hand_edit(
        dir: &Path,
        old: &str,
        regenerate: impl Fn(&CutSettings) -> String,
    ) -> Result<()> {
        if is_generated(old, |s| vec![regenerate(s)]) {
            return Ok(());
        }
        back_up(dir, old)
    }

    #[test]
    fn only_hand_edits_are_backed_up_and_never_over_each_other() {
        let dir = std::env::temp_dir().join(format!("junctur-bak-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // What the analysis makes: the kept end follows the padding.
        let regenerate = |s: &CutSettings| list(*s, 5.0 + s.pad_after);
        let wide = CutSettings {
            pad_after: 1.0,
            ..Default::default()
        };
        let bak = |name: &str| std::fs::read_to_string(dir.join(name)).ok();

        // A generated list replaced by a re-cut: nothing to keep.
        back_up_hand_edit(&dir, &regenerate(&CutSettings::default()), regenerate).unwrap();
        assert_eq!(bak("edit.junctur.bak"), None);
        // Two hand edits, each replaced by a re-cut: both kept.
        let hand1 = list(CutSettings::default(), 2.0);
        back_up_hand_edit(&dir, &hand1, regenerate).unwrap();
        back_up_hand_edit(&dir, &regenerate(&wide), regenerate).unwrap();
        let hand2 = list(wide, 3.0);
        back_up_hand_edit(&dir, &hand2, regenerate).unwrap();
        assert_eq!(bak("edit.junctur.bak"), Some(hand1));
        assert_eq!(bak("edit.junctur.bak2"), Some(hand2));
        assert_eq!(bak("edit.junctur.bak3"), None);
        // An untouched app save the re-cut reproduces (a speaker renamed in the app, the stamp
        // aside) is Junctur's own; an app save with other ranges is the user's.
        let renamed = dir.join("renamed.junctur");
        let wide_list = edit::parse(&regenerate(&wide)).unwrap();
        edit::save(&renamed, &wide_list).unwrap();
        let stamped = std::fs::read_to_string(&renamed).unwrap();
        assert_ne!(stamped, regenerate(&wide));
        back_up_hand_edit(&dir, &stamped, regenerate).unwrap();
        assert_eq!(bak("edit.junctur.bak3"), None);
        edit::save(&renamed, &edit::parse(&list(wide, 4.0)).unwrap()).unwrap();
        let app_cut = std::fs::read_to_string(&renamed).unwrap();
        back_up_hand_edit(&dir, &app_cut, regenerate).unwrap();
        assert_eq!(bak("edit.junctur.bak3"), Some(app_cut));
        std::fs::remove_file(dir.join("edit.junctur.bak3")).unwrap();
        // A list that does not parse is the user's too.
        back_up_hand_edit(&dir, "junctur-edit 1\nkeep x\n", regenerate).unwrap();
        assert!(bak("edit.junctur.bak3").is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_speakers_file_is_set_aside_and_writes_leave_no_temporary() {
        let dir = std::env::temp_dir().join(format!("junctur-spk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = Project {
            file: PathBuf::from("in.mp4"),
            dir: dir.clone(),
            cache_dir: dir.clone(),
        };
        assert!(p.speakers_file().unwrap().is_none());
        std::fs::write(p.speakers_path(), "{ nope").unwrap();
        assert!(p.speakers_file().is_err());
        assert!(p.speakers().is_none());
        let kept = set_aside(&p.speakers_path()).unwrap();
        assert_eq!(kept, dir.join("speakers.json.unreadable"));
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "{ nope");
        assert!(!p.speakers_path().exists());
        let mut found = Found {
            people: vec![],
            heard: None,
            known: vec![],
            method: Method::None,
            tracks: vec![],
            turns: vec![],
            speech: String::new(),
            pins: vec![],
        };
        found.pin("Ana", &[(1.0, 2.0)]);
        write_speakers(&p, &found).unwrap();
        write_speakers(&p, &found).unwrap();
        assert_eq!(p.speakers(), Some(found));
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        // A second unreadable file does not replace the first one set aside.
        std::fs::write(p.speakers_path(), "x").unwrap();
        assert_eq!(
            set_aside(&p.speakers_path()).unwrap(),
            dir.join("speakers.json.unreadable2")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_recut_without_found_speakers_keeps_the_ones_the_list_had() {
        let turn = |speaker| edit::Turn {
            start: speaker as f64,
            end: speaker as f64 + 1.0,
            speaker,
        };
        let mut had = Speakers::new(vec![turn(1), turn(2)], &[None, None], None);
        had.rename(2, "Ana");
        // `--no-speakers`, or a failed stage: nothing found, nothing erased.
        assert_eq!(speakers_for(None, Some(&had)), had);
        assert!(speakers_for(None, None).is_empty());
        // Found again: the rename carries over.
        let found = Found {
            people: vec![],
            heard: None,
            known: vec![],
            method: Method::Voices,
            tracks: vec![None, None],
            turns: vec![turn(2)],
            speech: String::new(),
            pins: vec![],
        };
        assert_eq!(speakers_for(Some(&found), Some(&had)).name(2), Some("Ana"));
        // Ana's stretch is speaker 1 now (an enrolled voice took the old numbering): the name
        // goes with what she says, not with the number.
        let renumbered = Found {
            turns: vec![edit::Turn {
                speaker: 1,
                ..turn(2)
            }],
            ..found.clone()
        };
        let s = speakers_for(Some(&renumbered), Some(&had));
        assert_eq!((s.name(1), s.name(2)), (Some("Ana"), Some("Speaker 2")));
        // A name an enrolled voice had (here: since forgotten) is never carried onto a cluster.
        let forgotten = Found {
            known: vec!["ANA".into()],
            ..found
        };
        assert_eq!(
            speakers_for(Some(&forgotten), Some(&had)).name(2),
            Some("Speaker 2")
        );
    }

    fn scratch_project(name: &str) -> (PathBuf, Project) {
        let d = std::env::temp_dir().join(format!("junctur-2d-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
        (d, p)
    }

    #[test]
    fn a_rename_moves_the_pins_of_speakers_an_older_junctur_heard() {
        // 5r: heard before the fix, so not labelled again; the renamed person's pins must still
        // follow the name, or the next hearing would drop them as a name no voice has.
        let (d, p) = scratch_project("rename-stale");
        let mut found = Found {
            method: Method::Voices,
            tracks: vec![],
            people: vec![],
            turns: vec![],
            speech: String::new(),
            heard: Some(Heard {
                track: 0,
                windows: vec![],
                overlaps: vec![],
                key: "before 5r".into(),
                dim: 0,
                embeddings: String::new(),
            }),
            known: vec![],
            pins: vec![],
        };
        found.pin("Ana", &[(1.0, 2.0)]);
        found.pin("Bea", &[(3.0, 4.0)]);
        write_speakers(&p, &found).unwrap();
        assert!(labelled(&p, &[]).is_err(), "not labelled from old windows");
        let renamed = labelled_renamed(&p, &[], "ana", "Anna").unwrap();
        let names: Vec<&str> = renamed.pins.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Anna", "Bea"]);
        assert_eq!((renamed.turns, renamed.heard), (found.turns, found.heard));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn outdated_names_what_a_rerun_would_redo() {
        let (d, p) = scratch_project("outdated");
        let opts = |model| PipelineOptions {
            model,
            ..Default::default()
        };
        assert_eq!(
            outdated(&p, &opts(ModelPref::Auto)),
            vec!["There is no transcript yet."]
        );
        // Without a transcript asked for, nothing is missing.
        let none = PipelineOptions {
            no_transcript: true,
            ..Default::default()
        };
        assert!(outdated(&p, &none).is_empty());
        let made = |langs: &str| {
            format!(
                r#"{{"choice":{{"model":"large-v3-turbo","device":"cuda","note":null}},
                "languages":{langs},"segments":[],"audio_s":0,"elapsed_s":0}}"#
            )
        };
        std::fs::write(transcribe::path(&p.dir), made(r#"["es","en"]"#)).unwrap();
        // `auto` takes whatever model made it; the same model explicitly too.
        assert!(outdated(&p, &opts(ModelPref::Auto)).is_empty());
        assert!(outdated(&p, &opts(ModelPref::LargeV3Turbo)).is_empty());
        let why = outdated(&p, &opts(ModelPref::LargeV3));
        assert_eq!(
            why,
            vec!["Transcribed with large-v3-turbo; the settings ask for large-v3."]
        );
        std::fs::write(transcribe::path(&p.dir), made(r#"["en"]"#)).unwrap();
        let why = outdated(&p, &opts(ModelPref::Auto));
        assert_eq!(why, vec!["Transcribed in en; the settings ask for es+en."]);
        // Unreadable is not missing: a run would make it again, and the user hears why.
        std::fs::write(transcribe::path(&p.dir), "{ torn").unwrap();
        let why = outdated(&p, &opts(ModelPref::Auto));
        assert!(
            why[0].starts_with("The transcript could not be read"),
            "{why:?}"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_resumed_run_keeps_the_users_list_but_not_a_broken_one() {
        let (d, p) = scratch_project("keeplist");
        let fresh = || edit::parse(&list(CutSettings::default(), 5.0)).unwrap();
        let draft_over = |old: &str| Draft {
            list: fresh(),
            old: Some(old.to_string()),
            users: true,
        };
        let mut log = |_: &str| {};
        // The user cut while it transcribed, then the app closed: resumed, their list stays.
        let mine = list(CutSettings::default(), 2.0);
        std::fs::write(p.edit_path(), &mine).unwrap();
        let kept = keep_or_write(&p, draft_over(&mine), None, KeepList::Edits, &mut log).unwrap();
        assert_eq!(kept.keep[0].end, 2.0);
        assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), mine);
        assert!(!p.dir.join("edit.junctur.bak").exists());
        // A re-analysis (not kept): backed up and re-cut.
        let recut = keep_or_write(&p, draft_over(&mine), None, KeepList::No, &mut log).unwrap();
        assert_eq!(recut.keep[0].end, 5.0);
        assert_eq!(
            std::fs::read_to_string(p.dir.join("edit.junctur.bak")).unwrap(),
            mine
        );
        // A list that does not parse is never kept, or the recording would never get one.
        std::fs::write(p.edit_path(), "junctur-edit 1\nkeep x\n").unwrap();
        let recut = keep_or_write(
            &p,
            draft_over("junctur-edit 1\nkeep x\n"),
            None,
            KeepList::Edits,
            &mut log,
        );
        assert_eq!(recut.unwrap().keep[0].end, 5.0);
        assert!(p.dir.join("edit.junctur.bak2").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_reanalysis_keeps_the_users_cut_names_and_fixes_and_takes_the_new_speakers() {
        // 5s: Re-analyse keeps the list the user edited and puts on top of it only the speakers
        // the run matched against the remembered voices.
        let (d, p) = scratch_project("reanalyse-keeps");
        let turn = |start: f64, speaker| edit::Turn {
            start,
            end: start + 1.0,
            speaker,
        };
        // The user's list, saved by the app: their own cut, speaker 2 named Ana in a colour of
        // their own, a word fixed and caption settings of their own.
        let mut mine = edit::parse(&list(CutSettings::default(), 2.0)).unwrap();
        mine.keep.push(edit::Range {
            start: 3.5,
            end: 4.5,
        });
        mine.speakers = Speakers::new(vec![turn(1.0, 1), turn(4.0, 2)], &[None, None], None);
        mine.speakers.rename(2, "Ana");
        mine.speakers.recolor(2, "#123456");
        edit::set_fix(&mut mine.fixes, 0, 4.25, 0, "anna", "Ana").unwrap();
        mine.captions = Some(edit::Captions {
            chars: 21,
            ..Default::default()
        });
        edit::save(&p.edit_path(), &mine).unwrap();
        let saved = std::fs::read_to_string(p.edit_path()).unwrap();
        // The run's own cut, which a fresh cut would write.
        let fresh = || edit::parse(&list(CutSettings::default(), 5.0)).unwrap();
        let draft_over = |old: &str| Draft {
            list: fresh(),
            old: Some(old.to_string()),
            users: true,
        };
        // Matched again: Ana's stretch is speaker 1 now (a remembered voice took the old
        // numbering), and a third stretch was found.
        let found = Found {
            people: vec![],
            heard: None,
            known: vec![],
            method: Method::Voices,
            tracks: vec![None, None],
            turns: vec![turn(4.0, 1), turn(1.0, 2), turn(7.0, 2)],
            speech: String::new(),
            pins: vec![],
        };
        let mut log = |_: &str| {};
        let kept = keep_or_write(
            &p,
            draft_over(&saved),
            Some(&found),
            KeepList::Whole,
            &mut log,
        )
        .unwrap();
        assert_eq!(kept.keep, mine.keep, "the user's cut, not the fresh one");
        assert_eq!(kept.fixes, mine.fixes);
        assert_eq!(kept.captions, mine.captions);
        assert_eq!(kept.settings, mine.settings);
        assert_eq!(
            kept.speakers.name(1),
            Some("Ana"),
            "the name follows her stretch"
        );
        assert_eq!(kept.speakers.color(1), Some("#123456"));
        assert_eq!(kept.speakers.name(2), Some("Speaker 2"));
        let starts: Vec<(f64, u32)> = kept
            .speakers
            .turns
            .iter()
            .map(|t| (t.start, t.speaker))
            .collect();
        assert_eq!(starts, [(1.0, 2), (4.0, 1), (7.0, 2)]);
        // Written as the app saves, and no backup: nothing of the user's was replaced.
        let text = std::fs::read_to_string(p.edit_path()).unwrap();
        assert_ne!(edit::unstamped(&text), text, "stamped");
        assert_eq!(edit::parse(edit::unstamped(&text)).unwrap(), kept);
        assert!(!p.dir.join("edit.junctur.bak").exists());
        // Run again with nothing new: the same list, untouched.
        let again = keep_or_write(
            &p,
            draft_over(&text),
            Some(&found),
            KeepList::Whole,
            &mut log,
        )
        .unwrap();
        assert_eq!(again, kept);
        assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), text);
        // No speakers this time (the stage failed): the list keeps the ones it has.
        let none = keep_or_write(&p, draft_over(&text), None, KeepList::Whole, &mut log).unwrap();
        assert_eq!(none, kept);
        // A list edited outside Junctur (a comment of the user's) is left whole, speakers and all,
        // rather than rewritten without the comment.
        let by_hand = format!("# my notes\n{}", mine.to_text());
        std::fs::write(p.edit_path(), &by_hand).unwrap();
        let left = keep_or_write(
            &p,
            draft_over(&by_hand),
            Some(&found),
            KeepList::Whole,
            &mut log,
        )
        .unwrap();
        assert_eq!(left.speakers, mine.speakers);
        assert_eq!(left.keep, mine.keep);
        assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), by_hand);
        assert!(!p.dir.join("edit.junctur.bak").exists());
        // A list that does not parse is left as it is too, and the stage says so: re-cutting is
        // only "Start over with a fresh cut", which asks first.
        let broken = "junctur-edit 1\nkeep x\n";
        std::fs::write(p.edit_path(), broken).unwrap();
        let err = keep_or_write(
            &p,
            draft_over(broken),
            Some(&found),
            KeepList::Whole,
            &mut log,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("left as it is"), "{err}");
        assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), broken);
        assert!(!p.dir.join("edit.junctur.bak").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_list_saved_after_the_draft_is_backed_up_not_lost() {
        // 5s repair: the draft reads the list outside the writer lock; a save landing before the
        // write is decided on again under it.
        let (d, p) = scratch_project("recheck");
        // Junctur's own list, as an untouched app save (so the user's save below is no backup).
        let generated = list(CutSettings::default(), 5.0);
        edit::save(&p.edit_path(), &edit::parse(&generated).unwrap()).unwrap();
        let drafted_from = std::fs::read_to_string(p.edit_path()).unwrap();
        let drafted = || Draft {
            list: edit::parse(&generated).unwrap(),
            old: Some(drafted_from.clone()),
            users: false,
        };
        // The user cut meanwhile: a re-cut keeps their save as the backup, not the stale text.
        let saved = edit::parse(&list(CutSettings::default(), 2.0)).unwrap();
        edit::save(&p.edit_path(), &saved).unwrap();
        let theirs = std::fs::read_to_string(p.edit_path()).unwrap();
        let mut log = |_: &str| {};
        keep_or_write(&p, drafted(), None, KeepList::No, &mut log).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.dir.join("edit.junctur.bak")).unwrap(),
            theirs
        );
        // Kept instead, when the run keeps the user's list (the editor's own backup of the plain
        // re-cut it saves over aside).
        edit::save(&p.edit_path(), &saved).unwrap();
        let kept = keep_or_write(&p, drafted(), None, KeepList::Whole, &mut log).unwrap();
        assert_eq!(kept.keep, saved.keep);
        assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), theirs);
        // A save of the very list the draft would write is nothing to back up.
        edit::save(&p.edit_path(), &edit::parse(&generated).unwrap()).unwrap();
        let backups = || {
            std::fs::read_dir(&p.dir)
                .unwrap()
                .filter(|e| {
                    let name = e.as_ref().unwrap().file_name();
                    name.to_string_lossy().contains(".bak")
                })
                .count()
        };
        let before = backups();
        keep_or_write(&p, drafted(), None, KeepList::No, &mut log).unwrap();
        assert_eq!(backups(), before);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_reanalysis_run_keeps_the_users_cut_and_names_and_takes_the_speakers_found() {
        // 5s repair: the whole first half a Re-analyse runs (`prepare` with `KeepList::Whole`),
        // every stage's result in the cache as an earlier analysis left it, so no media or model
        // is needed: tracks and speech reused, the speakers found again from the cache (two voice
        // tracks, one speaker each), the edit list kept.
        let (d, p) = scratch_project("prepare-keeps");
        let voice = |track: usize| tracks::LogicalTrack {
            members: vec![track],
            sameness: tracks::Sameness::Single,
            activity: crate::signal::Activity {
                mean_db: -30.0,
                peak_db: -10.0,
                near_silent_share: 0.0,
                silent: false,
            },
            speech: None,
            role: Role::Voice,
            confidence: 1.0,
            reasons: vec![],
        };
        let report = TrackReport {
            file: p.file.clone(),
            duration: 10.0,
            windows: vec![],
            window_seconds: 1.0,
            tracks: vec![],
            logical: vec![voice(0), voice(1)],
            speech_error: None,
            from_cache: false,
        };
        let entry = tracks::entry(&p.file, &tracks_options(&p, false)).unwrap();
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, serde_json::to_vec(&report).unwrap()).unwrap();
        let region = |start: f64, end: f64| crate::vad::Region { start, end };
        let speech = Speech {
            tracks: vec![
                speech::SpeechTrack {
                    track: 0,
                    regions: vec![region(1.0, 3.0)],
                    share: 0.2,
                },
                speech::SpeechTrack {
                    track: 1,
                    regions: vec![region(5.0, 8.0)],
                    share: 0.3,
                },
            ],
            seconds: 5.0,
        };
        std::fs::write(p.speech_path(), serde_json::to_vec(&speech).unwrap()).unwrap();
        std::fs::write(p.envelope_path(), b"").unwrap();
        let turn = |start: f64, end: f64, speaker| edit::Turn {
            start,
            end,
            speaker,
        };
        // The speakers as the stage finds them now: one per voice track, in track order.
        let found = Found {
            people: vec![None, None],
            heard: None,
            known: vec![],
            method: Method::Tracks,
            tracks: vec![Some(0), Some(1)],
            turns: vec![turn(1.0, 3.0, 1), turn(5.0, 8.0, 2)],
            speech: speakers::speech_key(&speech),
            pins: vec![],
        };
        write_speakers(&p, &found).unwrap();
        // The user's list, from when track 0's talk was speaker 2 (numbered otherwise then),
        // named Ana, with a cut of their own that the analysis would never make.
        let mut mine = edit::parse(&list(CutSettings::default(), 10.0)).unwrap();
        mine.source = p.file.clone();
        mine.keep = vec![edit::Range {
            start: 1.5,
            end: 2.5,
        }];
        mine.speakers = Speakers::new(
            vec![turn(1.0, 3.0, 2), turn(5.0, 8.0, 1)],
            &[Some(1), Some(0)],
            None,
        );
        mine.speakers.rename(2, "Ana");
        edit::set_fix(&mut mine.fixes, 0, 1.5, 0, "anna", "Ana").unwrap();
        edit::save(&p.edit_path(), &mine).unwrap();
        let opts = PipelineOptions {
            keep_list: KeepList::Whole,
            ..Default::default()
        };
        let mut log = |_: &str| {};
        let prepared = prepare(&p, &opts, &mut log, steps::none()).unwrap();
        let kept = p.edit().unwrap();
        assert_eq!(prepared.edit, kept);
        assert_eq!(kept.keep, mine.keep, "the user's cut");
        assert_eq!(kept.fixes, mine.fixes);
        assert_eq!(
            kept.speakers.name(1),
            Some("Ana"),
            "the name follows her talk"
        );
        assert_eq!(kept.speakers.name(2), Some("Speaker 2"));
        let turns: Vec<(f64, u32)> = kept
            .speakers
            .turns
            .iter()
            .map(|t| (t.start, t.speaker))
            .collect();
        assert_eq!(turns, [(1.0, 1), (5.0, 2)]);
        assert!(!p.dir.join("edit.junctur.bak").exists());
        // Not kept (a fresh cut): the analysis's own cut, the user's kept as a backup.
        let opts = PipelineOptions::default();
        let recut = prepare(&p, &opts, &mut log, steps::none()).unwrap().edit;
        assert_ne!(recut.keep, mine.keep);
        assert_eq!(recut.speakers.name(1), Some("Ana"));
        assert!(p.dir.join("edit.junctur.bak").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn hand_edited_without_a_list_or_its_analysis() {
        let (d, p) = scratch_project("handedited");
        assert!(!hand_edited(&p).unwrap(), "no list: nothing to lose");
        std::fs::write(
            p.edit_path(),
            "junctur-edit 1
keep x
",
        )
        .unwrap();
        assert!(
            hand_edited(&p).unwrap(),
            "a list that does not parse is the user's"
        );
        std::fs::write(p.edit_path(), list(CutSettings::default(), 5.0)).unwrap();
        assert!(
            hand_edited(&p).unwrap(),
            "a list whose analysis is gone cannot be remade"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn transcript_of(words: &[(f64, &str)]) -> Transcript {
        Transcript {
            choice: transcribe::Choice {
                model: "large-v3-turbo".into(),
                device: crate::runtime::Device::Cpu,
                note: None,
            },
            languages: vec![],
            segments: vec![transcribe::Segment {
                track: 0,
                start: 0.0,
                end: 10.0,
                lang: "en".into(),
                model: "large-v3-turbo".into(),
                words: words
                    .iter()
                    .map(|&(start, text)| transcribe::Word {
                        start,
                        end: start + 0.3,
                        text: text.into(),
                        p: 1.0,
                        p_min: None,
                    })
                    .collect(),
            }],
            audio_s: 10.0,
            elapsed_s: 1.0,
            resumed_chunks: 0,
        }
    }

    #[test]
    fn a_retranscription_carries_the_fixes_onto_the_new_words_and_spreads_them() {
        // 5aa: the Correct button's re-transcription. At its end the fixes, read against the
        // transcript it replaces, are moved onto the new words and spread to the words heard
        // alike, and the mark that asked for it goes.
        let (d, p) = scratch_project("carry");
        let fix = |start: f64, text: &str| edit::Fix {
            track: 0,
            start,
            nth: 0,
            through: None,
            text: text.into(),
        };
        let mut e = EditList {
            source: p.file.clone(),
            duration: 10.0,
            settings: CutSettings::default(),
            keep: vec![edit::Range {
                start: 0.0,
                end: 10.0,
            }],
            speakers: Default::default(),
            captions: Default::default(),
            fixes: vec![fix(1.0, "Vayne,"), fix(5.0, "Mell")],
            times: Vec::new(),
            windows: vec![],
        };
        std::fs::write(p.edit_path(), e.to_text()).unwrap();
        let old = transcript_of(&[(1.0, " Bane,"), (3.0, " Bane."), (5.0, " Mel")]);
        std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&old).unwrap()).unwrap();
        let lines = std::cell::RefCell::new(Vec::<String>::new());
        let mut log = |l: &str| lines.borrow_mut().push(l.to_string());
        let asked = PipelineOptions {
            retranscribe: true,
            ..Default::default()
        };
        assert!(hear_again(&p, &asked, &mut log).unwrap());
        let mark = p.dir.join(AGAIN_FILE);
        assert!(mark.is_file());
        // Asked and not started (the app closed before the first chunk): heard again, not settled
        // for the old transcript; once a partial is there, it is resumed as any run is.
        let plain = PipelineOptions::default();
        assert!(hear_again(&p, &plain, &mut log).unwrap());
        let partial = p.dir.join("transcript.partial.jsonl");
        std::fs::write(&partial, "{}").unwrap();
        assert!(!hear_again(&p, &plain, &mut log).unwrap());
        std::fs::remove_file(&partial).unwrap();
        // While it ran, the user kept the "Bane." at 3 as heard ("Undo those" on a spread): it
        // stays so, carried like any fix, and no spread reaches it.
        e.fixes.push(fix(3.0, "Bane."));
        std::fs::write(p.edit_path(), e.to_text()).unwrap();
        // The new transcript: "Bane," a little later, "Mel" now "and", a third "Bane" at 7. It is
        // in `transcript.json` already, as when the app stopped between Whisper's end and the carry
        // (5aa repair): the copy of the old one is what the fixes are read against.
        let new = transcript_of(&[
            (1.1, " Bane,"),
            (3.0, " Bane."),
            (5.0, " and"),
            (7.0, " bane"),
        ]);
        std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&new).unwrap()).unwrap();
        carry_fixes(&p, &new, &mut log);
        assert!(!mark.exists(), "the mark goes");
        assert!(!p.dir.join(BEFORE_FILE).exists(), "and the copy");
        // "Mel" is heard as "and" now: set aside, not lost, until the user has seen it.
        assert_eq!(
            fixes_set_aside(&p),
            [SetAside {
                track: 0,
                at: 5.0,
                heard: "Mel".into(),
                text: "Mell".into(),
                retimed: None,
            }]
        );
        forget_fixes_set_aside(&p, &fixes_set_aside(&p)).unwrap();
        assert!(fixes_set_aside(&p).is_empty());
        forget_fixes_set_aside(&p, &[]).unwrap();
        e = p.edit().unwrap();
        let shown: Vec<(f64, &str)> = e.fixes.iter().map(|f| (f.start, f.text.as_str())).collect();
        assert_eq!(shown, [(1.1, "Vayne,"), (3.0, "Bane."), (7.0, "Vayne")]);
        let said = lines.borrow().clone();
        assert!(
            said.iter().any(|l| l
                == "fixed words: 2 kept on their words, 0 no longer needed, 1 set aside; 1 more place(s) fixed"),
            "{said:?}"
        );
        assert!(said
            .iter()
            .any(|l| l.contains("\"Mell\" (heard \"Mel\") at 0:00:05 is heard otherwise now")));
        // Nothing asked: nothing carried, the list as it is.
        carry_fixes(&p, &new, &mut log);
        assert_eq!(p.edit().unwrap(), e);
        assert!(!hear_again(&p, &plain, &mut log).unwrap());
        // A transcription that fails before its first chunk forgets the ask (else every later
        // run would hear the recording again); one with a partial keeps it for its resume.
        assert!(hear_again(&p, &asked, &mut log).unwrap());
        std::fs::write(&partial, "{}").unwrap();
        not_again(&p, &mut log);
        assert!(mark.is_file());
        std::fs::remove_file(&partial).unwrap();
        not_again(&p, &mut log);
        assert!(!mark.exists() && !p.dir.join(BEFORE_FILE).exists());
        assert!(!hear_again(&p, &plain, &mut log).unwrap());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_retranscription_carries_word_times_and_survives_being_stopped_or_asked_again() {
        // 5ad: the leftovers of 5aa's review. A word retimed by moving a cut's edge (2w) is carried
        // like a fix; Correct asked again before an earlier one's carry keeps that one's copy of the
        // old transcript; a finished re-transcription is carried, not heard again; no copy means the
        // fixes are left, said; a failed mark takes the copy with it; "seen" forgets only what was
        // seen.
        let (d, p) = scratch_project("carry-times");
        let e = EditList {
            source: p.file.clone(),
            duration: 10.0,
            settings: CutSettings::default(),
            keep: vec![edit::Range {
                start: 0.0,
                end: 10.0,
            }],
            speakers: Default::default(),
            captions: Default::default(),
            fixes: vec![edit::Fix {
                track: 0,
                start: 1.0,
                nth: 0,
                through: None,
                text: "Vayne".into(),
            }],
            // "Bane" heard 1.0 to 1.3, its end moved out to 1.6; "Mel" 5.0 to 5.3, to 5.5.
            times: vec![
                edit::Retime {
                    track: 0,
                    start: 1.0,
                    nth: 0,
                    from: 1.0,
                    to: 1.6,
                },
                edit::Retime {
                    track: 0,
                    start: 5.0,
                    nth: 0,
                    from: 5.0,
                    to: 5.5,
                },
            ],
            windows: vec![],
        };
        std::fs::write(p.edit_path(), e.to_text()).unwrap();
        let old = transcript_of(&[(1.0, " Bane"), (5.0, " Mel")]);
        let old_bytes = serde_json::to_vec(&old).unwrap();
        std::fs::write(transcribe::path(&p.dir), &old_bytes).unwrap();
        let lines = std::cell::RefCell::new(Vec::<String>::new());
        let mut log = |l: &str| lines.borrow_mut().push(l.to_string());
        let asked = PipelineOptions {
            retranscribe: true,
            ..Default::default()
        };
        let plain = PipelineOptions::default();
        let (mark, before) = (p.dir.join(AGAIN_FILE), p.dir.join(BEFORE_FILE));
        assert!(hear_again(&p, &asked, &mut log).unwrap());
        // Whisper wrote the new transcript, then the app stopped before the carry.
        let new = transcript_of(&[(1.2, " Bane"), (5.0, " and")]);
        std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&new).unwrap()).unwrap();
        // Correct asked again: the copy stays the transcript the fixes are on.
        assert!(hear_again(&p, &asked, &mut log).unwrap());
        assert_eq!(std::fs::read(&before).unwrap(), old_bytes);
        // Opened again instead: that transcription had finished, so it is carried, not heard
        // again.
        assert!(!hear_again(&p, &plain, &mut log).unwrap());
        carry_fixes(&p, &new, &mut log);
        assert!(!mark.exists() && !before.exists());
        let now = p.edit().unwrap();
        // The fix and the time moved onto "Bane" at 1.2, its end still the user's 1.6.
        assert_eq!(
            (now.fixes[0].start, now.fixes[0].text.as_str()),
            (1.2, "Vayne")
        );
        assert_eq!(
            now.times,
            [edit::Retime {
                track: 0,
                start: 1.2,
                nth: 0,
                from: 1.2,
                to: 1.6,
            }]
        );
        // "Mel" is heard as "and" now: its time is set aside and listed, with what it was.
        let aside = fixes_set_aside(&p);
        assert_eq!(
            aside,
            [SetAside {
                track: 0,
                at: 5.0,
                heard: "Mel".into(),
                text: "Mel".into(),
                retimed: Some([5.0, 5.5]),
            }]
        );
        assert!(lines.borrow().iter().any(|l| l
            == "fixed words: 1 kept on their words, 0 no longer needed, 0 set aside; 0 more place(s) fixed; word times: 1 kept on their words, 0 no longer needed, 1 set aside"),
            "{:?}", lines.borrow());
        // A run adds one more after the panel listed the first: "seen" forgets only that first.
        let more = SetAside {
            track: 1,
            at: 7.0,
            heard: "hom".into(),
            text: "home".into(),
            retimed: None,
        };
        std::fs::write(
            p.dir.join(SET_ASIDE_FILE),
            serde_json::to_vec(&[aside[0].clone(), more.clone()]).unwrap(),
        )
        .unwrap();
        forget_fixes_set_aside(&p, &aside).unwrap();
        assert_eq!(fixes_set_aside(&p), std::slice::from_ref(&more));
        forget_fixes_set_aside(&p, &[more]).unwrap();
        assert!(!p.dir.join(SET_ASIDE_FILE).exists());
        // No copy of the old transcript (removed, or unreadable): the fixes are left, and said.
        let listed = p.edit().unwrap();
        assert!(hear_again(&p, &asked, &mut log).unwrap());
        std::fs::write(&before, "not json").unwrap();
        let newer = transcript_of(&[(2.0, " Bane")]);
        carry_fixes(&p, &newer, &mut log);
        assert_eq!(p.edit().unwrap(), listed);
        assert!(!mark.exists() && !before.exists());
        assert!(lines
            .borrow()
            .iter()
            .any(|l| l.starts_with("the transcript before was not kept (")));
        // A mark that cannot be written leaves no copy behind.
        std::fs::create_dir(&mark).unwrap();
        assert!(hear_again(&p, &asked, &mut log).is_err());
        assert!(!before.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
