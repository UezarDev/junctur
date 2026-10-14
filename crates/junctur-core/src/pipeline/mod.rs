//! The job queue: a recording's analysis stages, each leaving its result in the project folder so
//! a rerun skips what is done. Two halves, [`prepare`] (minutes) and [`finish`] (Whisper, hours), so
//! one recording's fast stages need not wait on another's transcription. Within a recording the
//! stages run one at a time: Whisper and the ONNX models share one GPU, and two at once on a full
//! card would both fall back to the CPU.

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

const EDIT_FILE: &str = "edit.junctur";
const ENVELOPE_FILE: &str = "envelope.f32";

/// There while a re-transcription asked for (the Correct button) has not finished: a run resumed
/// after the app closed still hears the recording again and carries the fixes onto the new words
/// ([`hear_again`], [`carry_fixes`]). Empty; the old transcript is kept in [`BEFORE_FILE`].
const AGAIN_FILE: &str = "transcribe-again";

/// What [`AGAIN_FILE`] holds once the re-transcription it asks for has written its transcript and
/// the carry is still to come: a run that finds it carries rather than hearing the
/// recording again. Empty while the hearing is still asked for.
const HEARD: &[u8] = b"heard";

/// The transcript a re-transcription replaces, copied when it is asked for: the fixes are
/// read against it at the end, and `transcript.json` is the new one from the moment Whisper finishes,
/// so a run that stopped between that write and the carry would otherwise carry against the new words.
const BEFORE_FILE: &str = "transcript.before.json";

/// Fixes a re-transcription could not put on any word (Whisper heard something else there, or
/// nothing), as they were, for the user to see and redo: kept until the app is told they were seen
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
    /// A word time set by moving a cut's edge that the re-transcription could not put on a
    /// word: the start and end the user had given it. `None` for a fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retimed: Option<[f64; 2]>,
    /// Its track has no words in the new transcript at all: not heard otherwise, not
    /// heard (or not transcribed) this time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub silent: bool,
    /// Not a fix: what the carry could not do, said where the user looks until they have seen it,
    /// since a log line is gone by the run's end. The other fields are empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl SetAside {
    /// A [`SetAside::note`].
    fn note(text: &str) -> Self {
        SetAside {
            track: 0,
            at: 0.0,
            heard: String::new(),
            text: String::new(),
            retimed: None,
            silent: false,
            note: Some(text.to_string()),
        }
    }
}

/// The fixes re-transcriptions of `p` set aside and the user has not yet seen.
pub fn fixes_set_aside(p: &Project) -> Vec<SetAside> {
    std::fs::read(p.dir.join(SET_ASIDE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Forgets the fixes set aside for `p` that the user has seen (`seen`, as [`fixes_set_aside`] gave
/// them): only those, so any a run added since the panel listed them stay. Under the edit list's
/// writer lock, as a run's additions are ([`keep_set_aside`]), so neither lands inside the other.
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
    /// Where Whisper may run. Only transcription uses it: tracks, speech and speakers always run on
    /// the CPU (CAM++ on the GPU finds a speaker per sentence).
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
    /// Whether an edit list holding the user's edits is kept or re-cut ([`KeepList`]).
    pub keep_list: KeepList,
    /// Hear the speakers again from the audio, cut at the user's pins, instead of reusing the
    /// hearing (the Correct button; what [`rehear`] does outside a run).
    pub rehear: bool,
    /// Transcribe the recording again, the word list as it is now, and carry the edit list's fixes
    /// onto the new words ([`finish`]; the Correct button).
    pub retranscribe: bool,
}

/// What a run's `edit` stage does with an edit list on disk that holds the user's edits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeepList {
    /// Re-cut it, keeping it as `edit.junctur.bak*` (`junctur analyze`, and the app's "Start over
    /// with a fresh cut", which asks first).
    #[default]
    No,
    /// Keep it, taking only the speakers the run found; one that does not parse (a broken hand
    /// edit) is backed up and re-cut, or the recording would never have a list (the app resuming a
    /// run the user edited during).
    Edits,
    /// As `Edits`, and one that does not parse is left as it is too: the run fails saying so,
    /// since re-cutting is only ever the user's choice here (the app's Re-analyse).
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
/// transcript is whole, `between` is called between chunks and may block: the app's pause;
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
    Ok(Outcome {
        report,
        speech,
        speakers,
        transcript,
        edit,
    })
}

/// The `edit` stage's write: unless `keep` is [`KeepList::No`], a list holding the user's edits is
/// kept with only its speakers replaced, through [`edit::amend`]; one that does not parse is re-cut
/// with [`KeepList::Edits`] and fails the stage with [`KeepList::Whole`]. Decided again under the
/// writer lock ([`Draft::recheck`]), since `d` was drafted outside it.
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

/// Whether this run's transcription hears the recording again: asked for now (`opts.retranscribe`,
/// which leaves [`AGAIN_FILE`]), or by an earlier run that stopped before its first chunk. An earlier
/// ask whose carry never ran keeps its copy of the old transcript, which the fixes are on; one whose
/// transcription finished is carried, not heard again.
fn hear_again(p: &Project, opts: &PipelineOptions, log: &mut dyn FnMut(&str)) -> Result<bool> {
    let mark = p.dir.join(AGAIN_FILE);
    if opts.retranscribe {
        if mark.is_file() {
            // Asked for again: heard again, whatever the last ask got to.
            crate::output::write_replacing(&mark, b"")?;
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
            // Nor a copy without its mark: nothing would ever remove it.
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

/// A re-transcription asked for has written its transcript: the mark says so ([`HEARD`], written by
/// [`heard_again`] once the transcription stage ends) and the transcript is there. Not told by the
/// transcript differing from the copy, which a hearing that came out the same never would.
fn heard_again_already(p: &Project) -> bool {
    std::fs::read(p.dir.join(AGAIN_FILE)).is_ok_and(|m| m == HEARD)
        && transcribe::path(&p.dir).is_file()
}

/// The transcription stage of a run with [`AGAIN_FILE`] there has ended with the new transcript
/// the mark says so, so a run stopped before the carry carries on the next open
/// instead of hearing the recording again.
fn heard_again(p: &Project, log: &mut dyn FnMut(&str)) {
    let mark = p.dir.join(AGAIN_FILE);
    if mark.is_file() {
        if let Err(e) = crate::output::write_replacing(&mark, HEARD) {
            log(&format!(
                "could not mark the transcription as heard again ({e})"
            ));
        }
    }
}

/// A transcription that failed: a re-transcription asked for is forgotten when nothing of
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

/// After a re-transcription ([`AGAIN_FILE`] there): the fixes and retimes, read against the old
/// transcript ([`BEFORE_FILE`]) at the end so edits made during the run count, move onto the new
/// words ([`edit::carry`], [`edit::carry_times`]) and fixes spread ([`edit::spread`]); what lands on
/// no word is set aside ([`SET_ASIDE_FILE`]). Nothing is carried with no old copy or no new words.
/// The mark and copy always go, or they would ask for another re-transcription.
fn carry_fixes(p: &Project, t: &Transcript, log: &mut dyn FnMut(&str)) {
    let mark = p.dir.join(AGAIN_FILE);
    if !mark.is_file() {
        return;
    }
    // The set-aside file is written under the same lock as the list, so the app's "seen"
    // cannot land between its read and its write.
    let _writing = edit::hold();
    let old = std::fs::read(p.dir.join(BEFORE_FILE))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Transcript>(&b).map_err(|e| e.to_string()));
    let words = t.in_order();
    // Said where the user looks too, when there was anything to carry.
    let left = |why: &str, log: &mut dyn FnMut(&str)| {
        log(&format!(
            "{why}: fixed words and word times are left where they were"
        ));
        let any = p
            .edit()
            .is_ok_and(|e| !e.fixes.is_empty() || !e.times.is_empty());
        let note = format!("Correct could not move your fixed words and word times onto the new transcript ({why}); they are where they were, and some may now show on other words.");
        if any {
            if let Err(e) = keep_set_aside(p, vec![SetAside::note(&note)]) {
                log(&format!("{SET_ASIDE_FILE} not written ({e})"));
            }
        }
    };
    match old {
        Err(e) => left(&format!("the transcript before was not kept: {e}"), log),
        Ok(_) if words.is_empty() => left("the new transcript has no words", log),
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

/// [`carry_fixes`]' work on the list, `before` and `words` the old and new transcripts. A result
/// the parser would refuse ([`EditList::checked_text`]) is retried without the spread, keeping its
/// set-aside; if that fails too, the list stays as it was and nothing is set aside.
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
    // A track the new transcript has no words on: its fixes are set aside as not heard, not as
    // heard otherwise.
    let heard: std::collections::HashSet<usize> = words.iter().map(|&(t, _)| t).collect();
    let why = |track: usize| {
        if heard.contains(&track) {
            "is heard otherwise now"
        } else {
            "is on a track with no words this time"
        }
    };
    let mut aside: Vec<SetAside> = Vec::new();
    match amended {
        Ok(edit::Amended::Written(_) | edit::Amended::Same(_)) if took == Took::Nothing => {
            log("edit list: carrying the fixed words would give a list that does not read back; they are left as they were");
            aside.push(SetAside::note("Correct could not move your fixed words and word times onto the new transcript (the edit list would not have read back); they are where they were, and some may now show on other words."));
        }
        Ok(edit::Amended::Written(_) | edit::Amended::Same(_)) => {
            if took == Took::Carried {
                log("edit list: spreading the fixed words would give a list that does not read back; carried without spreading");
                aside.push(SetAside::note("Correct moved your fixed words onto the new transcript but did not give them to the other words heard the same way (the edit list would not have read back)."));
            }
            log(&carried_line(&done));
            for g in &done.fixes.gone {
                log(&format!(
                    "fixed word set aside: \"{}\" (heard \"{}\") at {} {}",
                    g.fix.text,
                    g.heard.join(" "),
                    clock(g.from),
                    why(g.fix.track)
                ));
                aside.push(SetAside {
                    track: g.fix.track,
                    at: g.from,
                    heard: g.heard.join(" "),
                    text: g.fix.text.clone(),
                    retimed: None,
                    silent: !heard.contains(&g.fix.track),
                    note: None,
                });
            }
            for g in &done.times.gone {
                log(&format!(
                    "word time set aside: \"{}\" at {} {}",
                    g.heard,
                    clock(g.start),
                    why(g.time.track)
                ));
                aside.push(SetAside {
                    track: g.time.track,
                    at: g.start,
                    heard: g.heard.clone(),
                    text: g.fixed.clone().unwrap_or_else(|| g.heard.clone()),
                    retimed: Some([g.time.from, g.time.to]),
                    silent: !heard.contains(&g.time.track),
                    note: None,
                });
            }
        }
        Ok(edit::Amended::Left) => {
            log("edit list: edited outside Junctur, its fixed words are left as they are")
        }
        Ok(edit::Amended::Missing) => {}
        Err(e) => log(&format!("edit list: fixed words not carried ({e})")),
    }
    if let Err(e) = keep_set_aside(p, aside) {
        log(&format!("{SET_ASIDE_FILE} not written ({e})"));
    }
}

/// Adds `new` to the fixes set aside for `p` ([`SET_ASIDE_FILE`]). Called under the edit list's
/// writer lock ([`carry_fixes`]), as [`forget_fixes_set_aside`] is.
fn keep_set_aside(p: &Project, new: Vec<SetAside>) -> Result<()> {
    if new.is_empty() {
        return Ok(());
    }
    let mut all = fixes_set_aside(p);
    all.extend(new);
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
    // An unreadable file holds pins nothing else knows: kept aside under another name and
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
            // The user's pins cut the windows and outlive the hearing they were made on.
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

/// Remembers `name`'s voice from `stretches` of this recording (source seconds; one word will do)
/// into `voices`, pins them to `name` ([`Found::pin`]) and labels the speakers with it. Nothing is
/// written ([`enroll_and_save`] writes both). Returns the speakers and the seconds of speech this
/// added to the voice.
pub fn enroll(
    p: &Project,
    voices: &mut Voices,
    name: &str,
    stretches: &[(f64, f64)],
) -> Result<(Found, f64)> {
    let mut found = p.speakers().ok_or_else(not_analysed)?;
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
    let again = speakers::relabel(&found, &p.dir, &changed.usable_owned())?;
    *voices = changed;
    Ok((again, seconds))
}

/// [`enroll`], then saves the voices to `voices_path`, then the speakers: new labels never stand on
/// voices that were not saved.
pub fn enroll_and_save(
    p: &Project,
    voices: &mut Voices,
    voices_path: &Path,
    name: &str,
    stretches: &[(f64, f64)],
) -> Result<(Found, f64)> {
    let (found, added) = enroll(p, voices, name, stretches)?;
    voices.save(voices_path)?;
    save_speakers(p, &found)?;
    Ok((found, added))
}

/// Hears the project's speakers again from the audio ("Analyse speakers again"), the windows
/// cut at the pins the user made ([`speakers::seeded`]), and labels them with `voices`; the other
/// stages' results are reused, never rerun (they need nothing new: `--force` would redo Whisper
/// for minutes). Saves `speakers.json`; the edit list is the caller's ([`respeak`]).
pub fn rehear(p: &Project, voices: &[Voice], log: &mut dyn FnMut(&str)) -> Result<Found> {
    let speech = p.speech().ok_or_else(not_analysed)?;
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
    let found = p.speakers().ok_or_else(not_analysed)?;
    speakers::relabel(&found, &p.dir, voices)
}

/// [`labelled`] after a remembered person was renamed `from` -> `to` in `voices`: the seconds
/// pinned to them follow the new name. Speakers heard by an older Junctur cannot be
/// labelled again, but their pins still follow the name, so that the next hearing gives those
/// seconds to the renamed person instead of dropping them; the rest is kept as it is.
pub fn labelled_renamed(p: &Project, voices: &[Voice], from: &str, to: &str) -> Result<Found> {
    let mut found = p.speakers().ok_or_else(not_analysed)?;
    found.rename_pins(from, to);
    if found.heard_by_an_older_junctur() {
        return Ok(found);
    }
    speakers::relabel(&found, &p.dir, voices)
}

/// Writes `speakers.json` whole ([`crate::output::write_replacing`]): it holds the user's pins, and
/// a write torn by a crash would lose them.
fn write_speakers(p: &Project, found: &Found) -> Result<()> {
    crate::output::write_replacing(&p.speakers_path(), serde_json::to_vec_pretty(found)?)?;
    Ok(())
}

/// Renames `path` to the first free `<name>.unreadable`, `<name>.unreadable2`, ... beside it;
/// returns the new path.
fn set_aside(path: &Path) -> Result<PathBuf> {
    let free = crate::output::first_free(path, "unreadable");
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
    // From the read to the save: word counts the app's own analysis writes meanwhile
    // ([`edit::amend`]) would otherwise be saved over.
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

/// Labels the project's speakers again with `voices` (after enrolling or forgetting one), from
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
    // list does not parse. And only a list that reads back, as every edit-list writer.
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
    // Fixed words too: a re-cut changes what is kept, not what a word says.
    let fixes = parsed.as_ref().map_or_else(Vec::new, |e| e.fixes.clone());
    // And retimed words: where a word is said is not what a re-cut decides.
    let times = parsed.as_ref().map_or_else(Vec::new, |e| e.times.clone());
    // Caption settings are the user's, like the names: a re-cut keeps them.
    // None stays None: a list that followed the user's defaults still does.
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
                // As this call would write it, and as a run writes it before its transcript (no
                // word counts) and once it is done (the finished transcript's).
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
/// or whose analysis is gone. For the app to ask before a re-analysis.
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

/// Why a re-run with `opts` would redo more than the cut, one line each for the user: no
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

/// Keeps `old` at the first free `edit.junctur.bak`, `edit.junctur.bak2`, ... ([`edit::backup_path`],
/// the names the editor's saves use too).
fn back_up(dir: &Path, old: &str) -> Result<()> {
    std::fs::write(edit::backup_path(&dir.join(EDIT_FILE)), old)?;
    Ok(())
}

/// What a step that needs the analysis answers before there is one.
fn not_analysed() -> Error {
    Error::Edit("run `junctur analyze` on this file first".into())
}

/// Re-cut from what earlier stages left, without running them.
pub fn recut(p: &Project, settings: &CutSettings) -> Result<EditList> {
    let speech = p.speech().ok_or_else(not_analysed)?;
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
mod tests;
