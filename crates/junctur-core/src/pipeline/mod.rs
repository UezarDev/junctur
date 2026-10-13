//! The job queue: analysis stages run one after another for a recording, each leaving its result
//! in the project folder, so a rerun skips what is done and an interrupted one resumes.
//!
//! Sequential on purpose: Whisper and the ONNX models share one GPU, and running two at once on a
//! card another app already fills would only make both fall back to CPU. `jobs.json` records what
//! each stage did, for `junctur status`; the stage results themselves are the source of truth.

mod adopt;
mod jobs;
mod speakers;
mod speech;

use std::path::{Path, PathBuf};
use std::time::Instant;

pub use jobs::{Jobs, StageRecord, StageState, STAGES};
pub use speakers::{hearing_key, seeded, speech_key, Found, Heard, Method, Pin};
pub use speech::{Speech, SpeechTrack};

use crate::edit::{self, CutSettings, EditList, Speakers};
use crate::runtime::DevicePref;
use crate::tracks::{self, Role, TrackReport};
use crate::transcribe::{self, Languages, ModelPref, Source, TranscribeOptions, Transcript};
use crate::voices::{Voice, Voices};
use crate::{cache, hidden, Error, Result};

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
    pub device: DevicePref,
    pub model: ModelPref,
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
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();

    let report = jobs.stage(&p.dir, "tracks", log, |_| {
        tracks::analyze(&p.file, &tracks_options(p, opts.force))
    })?;
    let speech = speech_stage(p, &mut jobs, &report, opts.force, log)?;
    // Speakers are an extra, not core: when they cannot be found (the model cannot be fetched
    // offline, say), the run goes on without them and the edit list keeps the names it had.
    let speakers = if opts.no_speakers {
        None
    } else {
        match speakers_stage(p, &mut jobs, &report, &speech, opts, log) {
            Ok(f) => Some(f),
            Err(e) => {
                log(&format!("speakers: {e}; going on without them"));
                None
            }
        }
    };
    let transcript = if opts.no_transcript {
        None
    } else {
        Some(transcribe_stage(p, &mut jobs, &report, &speech, opts, log)?)
    };
    let edit = jobs.stage(&p.dir, "edit", log, |_| {
        cut(
            p,
            &report,
            &speech,
            speakers.as_ref(),
            transcript.as_ref(),
            &opts.cut,
        )
    })?;
    Ok(Outcome {
        report,
        speech,
        speakers,
        transcript,
        edit,
    })
}

/// Speech regions and the cut envelope on every speech track, reused unless `force`.
fn speech_stage(
    p: &Project,
    jobs: &mut Jobs,
    report: &TrackReport,
    force: bool,
    log: &mut dyn FnMut(&str),
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
            let (s, env) = speech::detect(&p.file, &sources, &p.cache_dir)?;
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
                opts.device,
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
pub fn rehear(
    p: &Project,
    voices: &[Voice],
    device: DevicePref,
    log: &mut dyn FnMut(&str),
) -> Result<Found> {
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
        device,
        force: true,
        voices: voices.to_vec(),
        ..Default::default()
    };
    speakers_stage(p, &mut jobs, &report, &speech, &opts, log)
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
/// pinned to them (5c) follow the new name.
pub fn labelled_renamed(p: &Project, voices: &[Voice], from: &str, to: &str) -> Result<Found> {
    let mut found = p
        .speakers()
        .ok_or_else(|| Error::Edit("run `junctur analyze` on this file first".into()))?;
    found.rename_pins(from, to);
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
fn transcribe_stage(
    p: &Project,
    jobs: &mut Jobs,
    report: &TrackReport,
    speech: &Speech,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Transcript> {
    let t_opts = TranscribeOptions {
        model: opts.model,
        device: opts.device,
        languages: opts.languages.clone(),
        force: opts.force,
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
    let d = draft(p, report, speech, speakers, transcript, settings);
    if let (Some(old), true) = (&d.old, d.users) {
        back_up(&p.dir, old)?;
    }
    // Whole or not at all: a list cut short by a close would open as an analysed recording whose
    // list does not parse.
    crate::output::write_replacing(&p.edit_path(), d.list.to_text())?;
    Ok(d.list)
}

/// A re-cut not yet written: the new list, the text on disk it replaces, and whether that text is
/// the user's (a re-cut backs it up).
struct Draft {
    list: EditList,
    old: Option<String>,
    users: bool,
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
    // Caption settings are the user's, like the names: a re-cut keeps them.
    // None stays None: a list that followed the user's defaults still does (2i).
    let captions = parsed.and_then(|e| e.captions);
    let speakers = speakers_for(speakers, previous.as_ref());
    let make = |settings: &CutSettings| EditList {
        source: p.file.clone(),
        duration: report.duration,
        settings: *settings,
        keep: edit::keep_ranges(&regions, &env, report.duration, settings),
        speakers: speakers.clone(),
        captions,
        windows: edit::windows(&regions, &starts, report.duration, settings.window),
    };
    let list = make(settings);
    let users = old.as_deref().is_some_and(|old| {
        // A list written before speakers existed (or with `--no-speakers`) is compared as Junctur
        // would have written it then, without them: gaining speakers is not a reason to back it up.
        let had_speakers = previous.as_ref().is_some_and(|s| !s.is_empty());
        old != list.to_text()
            && !is_generated(old, |s| {
                let mut e = make(s);
                if !had_speakers {
                    e.speakers = Speakers::default();
                }
                e.to_text()
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
        force: false,
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
fn is_generated(old: &str, regenerate: impl Fn(&CutSettings) -> String) -> bool {
    let body = edit::unstamped(old);
    edit::parse(body).is_ok_and(|e| regenerate(&e.settings) == body)
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
        if is_generated(old, regenerate) {
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
}
