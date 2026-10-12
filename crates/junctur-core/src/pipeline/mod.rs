//! The job queue: analysis stages run one after another for a recording, each leaving its result
//! in the project folder, so a rerun skips what is done and an interrupted one resumes.
//!
//! Sequential on purpose: Whisper and the ONNX models share one GPU, and running two at once on a
//! card another app already fills would only make both fall back to CPU. `jobs.json` records what
//! each stage did, for `junctur status`; the stage results themselves are the source of truth.

mod jobs;
mod speakers;
mod speech;

use std::path::{Path, PathBuf};
use std::time::Instant;

pub use jobs::{Jobs, StageRecord, StageState, STAGES};
pub use speakers::{Found, Method};
pub use speech::{Speech, SpeechTrack};

use crate::edit::{self, CutSettings, EditList, Speakers};
use crate::runtime::DevicePref;
use crate::tracks::{self, Role, TrackReport};
use crate::transcribe::{self, Languages, ModelPref, Source, TranscribeOptions, Transcript};
use crate::{cache, Error, Result};

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
        let dir = match dir {
            Some(d) => d,
            None => cache_dir
                .join("projects")
                .join(cache::key(file, "project1")?),
        };
        std::fs::create_dir_all(&dir)?;
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

    /// Who talks when, as the `speakers` stage left it; `None` before it ran.
    pub fn speakers(&self) -> Option<Found> {
        serde_json::from_slice(&std::fs::read(self.speakers_path()).ok()?).ok()
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
}

/// What a finished run produced.
pub struct Outcome {
    pub report: TrackReport,
    pub speech: Speech,
    pub speakers: Option<Found>,
    pub transcript: Option<Transcript>,
    pub edit: EditList,
}

/// Run every stage in order. `log` gets one line per event, for the CLI to print.
pub fn run(p: &Project, opts: &PipelineOptions, log: &mut dyn FnMut(&str)) -> Result<Outcome> {
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();

    let report = jobs.stage(&p.dir, "tracks", log, |_| {
        let a = tracks::AnalyzeOptions {
            cache_dir: Some(p.cache_dir.clone()),
            use_cache: !opts.force,
            ..Default::default()
        };
        tracks::analyze(&p.file, &a)
    })?;
    let speech = speech_stage(p, &mut jobs, &report, opts.force, log)?;
    let speakers = if opts.no_speakers {
        None
    } else {
        Some(speakers_stage(p, &mut jobs, &speech, opts, log)?)
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

/// Who talks when, reused unless `force`.
fn speakers_stage(
    p: &Project,
    jobs: &mut Jobs,
    speech: &Speech,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Found> {
    let found = match p.speakers().filter(|_| !opts.force) {
        Some(f) => {
            jobs.skip(&p.dir, "speakers", log)?;
            f
        }
        None => jobs.stage(&p.dir, "speakers", log, |_| {
            let f = speakers::find(&p.file, speech, &p.cache_dir, opts.device)?;
            std::fs::write(p.speakers_path(), serde_json::to_vec_pretty(&f)?)?;
            Ok(f)
        })?,
    };
    log(&match found.method {
        Method::Tracks => format!("{} speakers, one per speech track", found.tracks.len()),
        Method::Voices => format!(
            "{} speaker(s) told apart by voice, {} turns",
            found.tracks.len(),
            found.turns.len()
        ),
        Method::None => "no speakers: no track carries speech".into(),
    });
    Ok(found)
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

/// (Re)write the edit list from cached analysis. A hand-edited list is kept as a backup before
/// being replaced ([`back_up_hand_edit`]). Speaker names already in the list are kept by id, so a
/// re-cut never undoes a rename.
pub fn cut(
    p: &Project,
    report: &TrackReport,
    speech: &Speech,
    speakers: Option<&Found>,
    transcript: Option<&Transcript>,
    settings: &CutSettings,
) -> Result<EditList> {
    let env = speech::load_envelope(&p.envelope_path()).unwrap_or_default();
    let regions = speech.all_regions();
    let starts: Vec<f64> = transcript
        .map(|t| t.words().map(|w| w.start).collect())
        .unwrap_or_default();
    let path = p.edit_path();
    let old = std::fs::read_to_string(&path).ok();
    let previous = old
        .as_deref()
        .and_then(|t| edit::parse(t).ok())
        .map(|e| e.speakers);
    let speakers = speakers.map_or_else(Speakers::default, |f| {
        Speakers::new(f.turns.clone(), &f.tracks, previous.as_ref())
    });
    let make = |settings: &CutSettings| EditList {
        source: p.file.clone(),
        duration: report.duration,
        settings: *settings,
        keep: edit::keep_ranges(&regions, &env, report.duration, settings),
        speakers: speakers.clone(),
        windows: edit::windows(&regions, &starts, report.duration, settings.window),
    };
    let e = make(settings);
    let text = e.to_text();
    if let Some(old) = old {
        if old != text {
            // A list written before speakers existed (or with `--no-speakers`) is compared as
            // Junctur would have written it then, without them: gaining speakers is not a reason
            // to back it up.
            let had_speakers = previous.as_ref().is_some_and(|s| !s.is_empty());
            back_up_hand_edit(&p.dir, &old, |s| {
                let mut e = make(s);
                if !had_speakers {
                    e.speakers = Speakers::default();
                }
                e.to_text()
            })?;
        }
    }
    std::fs::write(&path, text)?;
    Ok(e)
}

/// Keep `old`, the edit list about to be replaced, unless Junctur wrote it: `regenerate` remakes a
/// list from the current analysis, and a list it reproduces from its own recorded settings has
/// nothing of the user's in it. Anything else (a hand edit, a list that no longer parses, one made
/// from older analysis) goes to the first free `edit.junctur.bak`, `edit.junctur.bak2`, ...
/// Backups are never overwritten: a single `.bak` rewritten on every re-cut lost the hand edit to
/// the next generated list (1d).
fn back_up_hand_edit(
    dir: &Path,
    old: &str,
    regenerate: impl Fn(&CutSettings) -> String,
) -> Result<()> {
    let generated = edit::parse(old).is_ok_and(|e| regenerate(&e.settings) == old);
    if generated {
        return Ok(());
    }
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
    let opts = tracks::AnalyzeOptions {
        cache_dir: Some(p.cache_dir.clone()),
        ..Default::default()
    };
    let report = tracks::analyze(&p.file, &opts)?;
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
            windows: vec![],
        }
        .to_text()
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
        // A list that does not parse is the user's too.
        back_up_hand_edit(&dir, "junctur-edit 1\nkeep x\n", regenerate).unwrap();
        assert!(bak("edit.junctur.bak3").is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
