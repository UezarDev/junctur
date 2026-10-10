//! The job queue: analysis stages run one after another for a recording, each leaving its result
//! in the project folder, so a rerun skips what is done and an interrupted one resumes.
//!
//! Sequential on purpose: Whisper and the ONNX models share one GPU, and running two at once on a
//! card another app already fills would only make both fall back to CPU. `jobs.json` records what
//! each stage did, for `junctur status`; the stage results themselves are the source of truth.

mod jobs;
mod speech;

use std::path::{Path, PathBuf};
use std::time::Instant;

pub use jobs::{Jobs, StageRecord, StageState, STAGES};
pub use speech::{Speech, SpeechTrack};

use crate::edit::{self, CutSettings, EditList};
use crate::runtime::DevicePref;
use crate::tracks::{self, Role, TrackReport};
use crate::transcribe::{self, Languages, ModelPref, Source, TranscribeOptions, Transcript};
use crate::{cache, Error, Result};

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
    fn envelope_path(&self) -> PathBuf {
        self.dir.join("envelope.f32")
    }

    pub fn jobs(&self) -> Jobs {
        Jobs::load(&self.dir)
    }

    pub fn speech(&self) -> Option<Speech> {
        serde_json::from_slice(&std::fs::read(self.speech_path()).ok()?).ok()
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
}

/// What a finished run produced.
pub struct Outcome {
    pub report: TrackReport,
    pub speech: Speech,
    pub transcript: Option<Transcript>,
    pub edit: EditList,
}

/// Run every stage in order. `log` gets one line per event, for the CLI to print.
pub fn run(p: &Project, opts: &PipelineOptions, log: &mut dyn FnMut(&str)) -> Result<Outcome> {
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();

    let report = jobs.stage(&p.dir, "tracks", log, || {
        let a = tracks::AnalyzeOptions {
            cache_dir: Some(p.cache_dir.clone()),
            use_cache: !opts.force,
            ..Default::default()
        };
        tracks::analyze(&p.file, &a)
    })?;
    let sources = speech_tracks(&report);
    if sources.is_empty() {
        log("no track carries speech: the edit list will be empty");
    }

    let speech = match p.speech().filter(|_| !opts.force) {
        Some(s) if p.envelope_path().is_file() => {
            jobs.skip(&p.dir, "speech", log)?;
            s
        }
        _ => jobs.stage(&p.dir, "speech", log, || {
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

    let transcript = if opts.no_transcript {
        None
    } else {
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
        let mut last = 0;
        let t = jobs.stage(&p.dir, "transcribe", log, || {
            transcribe::transcribe(
                &p.file,
                &src,
                report.duration,
                &p.dir,
                &p.cache_dir,
                &t_opts,
                &mut |done, total| {
                    // Progress every 5%, to stderr: stdout is the report.
                    let pct = done * 100 / total.max(1);
                    if pct >= last + 5 || done == total {
                        last = pct;
                        eprintln!("junctur: transcribing {done}/{total} chunks ({pct}%)");
                    }
                },
            )
        })?;
        Some(t)
    };

    let edit = jobs.stage(&p.dir, "edit", log, || {
        cut(p, &report, &speech, transcript.as_ref(), &opts.cut)
    })?;
    Ok(Outcome {
        report,
        speech,
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

/// (Re)write the edit list from cached analysis. A hand-edited list is kept as `edit.junctur.bak`
/// before being replaced.
pub fn cut(
    p: &Project,
    report: &TrackReport,
    speech: &Speech,
    transcript: Option<&Transcript>,
    settings: &CutSettings,
) -> Result<EditList> {
    let env = speech::load_envelope(&p.envelope_path()).unwrap_or_default();
    let regions = speech.all_regions();
    let starts: Vec<f64> = transcript
        .map(|t| t.words().map(|w| w.start).collect())
        .unwrap_or_default();
    let e = EditList {
        source: p.file.clone(),
        duration: report.duration,
        settings: *settings,
        keep: edit::keep_ranges(&regions, &env, report.duration, settings),
        windows: edit::windows(&regions, &starts, report.duration, settings.window),
    };
    let text = e.to_text();
    let path = p.edit_path();
    if let Ok(old) = std::fs::read_to_string(&path) {
        if old != text {
            std::fs::write(p.dir.join("edit.junctur.bak"), old)?;
        }
    }
    std::fs::write(&path, text)?;
    Ok(e)
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
    let e = cut(p, &report, &speech, transcript.as_ref(), settings)?;
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
