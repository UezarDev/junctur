//! Speech to text with word timestamps, only over speech regions, resumable chunk by chunk.
//!
//! Every finished chunk is appended to `transcript.partial.jsonl` in the project folder before the
//! next starts, so an interrupted run (a 30h stream is hours of Whisper) loses one chunk at most.
//! The finished transcript is `transcript.json`, plus a plain `transcript.txt` to read.

mod chunks;
mod languages;
mod whisper;
mod words;

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub use chunks::{plan, Chunk};
pub use languages::Languages;
pub use whisper::{choose, Choice, ModelPref, Whisper};
pub use words::Word;

use crate::ffmpeg::{PcmReader, Span};
use crate::runtime::{Device, DevicePref};
use crate::vad::{self, Region};
use crate::{models, Result};

/// One audio track to transcribe, with its speech regions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// 0-based audio ordinal.
    pub track: usize,
    pub regions: Vec<Region>,
}

/// The text of one chunk. Times are seconds in the recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub track: usize,
    pub start: f64,
    pub end: f64,
    pub lang: String,
    pub model: String,
    pub words: Vec<Word>,
}

impl Segment {
    pub fn text(&self) -> String {
        let w: Vec<&str> = self.words.iter().map(|w| w.text.as_str()).collect();
        w.join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub choice: Choice,
    pub languages: Vec<String>,
    pub segments: Vec<Segment>,
    /// Seconds of audio given to Whisper.
    pub audio_s: f64,
    /// Wall time of the run that finished it (chunks from an interrupted run not included).
    pub elapsed_s: f64,
    /// Chunks taken from an interrupted earlier run.
    #[serde(default)]
    pub resumed_chunks: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TranscribeOptions {
    pub model: ModelPref,
    pub device: DevicePref,
    pub languages: Languages,
    pub force: bool,
}

/// First line of the partial file. A partial made with other languages or another model is not
/// resumed, so one transcript never mixes models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Header {
    version: u32,
    languages: String,
    model: String,
    /// Where the run that started the partial ran; reported when a resume finds nothing left to do.
    device: Device,
}

impl Header {
    fn same_run(&self, other: &Header) -> bool {
        (self.version, &self.languages, &self.model)
            == (other.version, &other.languages, &other.model)
    }
}

/// 2: the header names the model (1f). A version-1 partial has no model, so it is not resumed.
const VERSION: u32 = 2;

/// Whether a finished transcript answers `opts`. `Auto` takes whatever model made it (rerunning
/// hours of Whisper because the default moved is `--force`'s job); an explicit model must match, so
/// `--model large-v3` is never silently ignored. The languages must match either way.
fn reusable(t: &Transcript, opts: &TranscribeOptions) -> bool {
    t.languages == opts.languages.allowed
        && (opts.model == ModelPref::Auto || t.choice.model == opts.model.name())
}

/// Where a project keeps the transcript.
pub fn path(project: &Path) -> std::path::PathBuf {
    project.join("transcript.json")
}

pub fn load(project: &Path) -> Option<Transcript> {
    serde_json::from_slice(&std::fs::read(path(project)).ok()?).ok()
}

pub fn transcribe(
    file: &Path,
    sources: &[Source],
    total: f64,
    project: &Path,
    cache_dir: &Path,
    opts: &TranscribeOptions,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Transcript> {
    let partial = project.join("transcript.partial.jsonl");
    if opts.force {
        let _ = std::fs::remove_file(&partial);
        let _ = std::fs::remove_file(path(project));
    } else if partial.is_file() {
        // A newer run was interrupted after this transcript was written: resume that run below
        // rather than hand back what it was replacing.
    } else if let Some(t) = load(project).filter(|t| reusable(t, opts)) {
        return Ok(t);
    }
    let prior = read_partial(&partial);
    // `Auto` resumes an interrupted run with the model it was started with, rather than restarting.
    let pref = match &prior {
        Some((h, _))
            if opts.model == ModelPref::Auto
                && h.version == VERSION
                && h.languages == opts.languages.key() =>
        {
            h.model.parse().unwrap_or(ModelPref::Auto)
        }
        _ => opts.model,
    };
    let want = Header {
        version: VERSION,
        languages: opts.languages.key(),
        model: pref.name(),
        device: Device::Cpu,
    };
    let (started, mut done) = match prior {
        Some((h, segs)) if h.same_run(&want) => (Some(h), segs),
        _ => (None, Vec::new()),
    };
    let resumed_chunks = done.len();

    let work: Vec<(usize, Chunk)> = sources
        .iter()
        .flat_map(|s| {
            plan(&s.regions, total)
                .into_iter()
                .map(move |c| (s.track, c))
        })
        .collect();
    let audio_s = 0.0 + work.iter().map(|(_, c)| c.end - c.start).sum::<f64>();
    let t0 = Instant::now();
    let todo: Vec<_> = work
        .iter()
        .filter(|(t, c)| !done.iter().any(|s| s.track == *t && s.start == c.start))
        .collect();
    // Nothing left to hear: no GPU query, no model load. The choice is the one that did the work.
    let mut choice = Choice {
        model: want.model.clone(),
        device: started.as_ref().map_or(Device::Cpu, |h| h.device),
        note: None,
    };
    if !todo.is_empty() {
        let model;
        (choice, model) = choose(pref, opts.device);
        if let Some(note) = &choice.note {
            eprintln!("junctur: {note}");
        }
        if started.is_none() {
            let h = Header {
                device: choice.device,
                ..want.clone()
            };
            std::fs::write(&partial, format!("{}\n", serde_json::to_string(&h)?))?;
        }
        let mut w = Whisper::open(&models::ensure(model, cache_dir)?, choice.device)?;
        let mut out = std::fs::OpenOptions::new().append(true).open(&partial)?;
        for (i, (track, c)) in todo.iter().enumerate() {
            progress(resumed_chunks + i, work.len());
            let samples = read_span(file, *track, c)?;
            let (lang, words) = w.run(&samples, &opts.languages)?;
            let seg = Segment {
                track: *track,
                start: c.start,
                end: c.end,
                lang,
                model: choice.model.clone(),
                words: words
                    .into_iter()
                    .map(|x| Word {
                        start: c.start + x.start,
                        end: c.start + x.end,
                        ..x
                    })
                    .collect(),
            };
            writeln!(out, "{}", serde_json::to_string(&seg)?)?;
            out.flush()?;
            done.push(seg);
        }
        progress(work.len(), work.len());
    }
    done.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.track.cmp(&b.track)));
    let t = Transcript {
        choice,
        languages: opts.languages.allowed.clone(),
        segments: done,
        audio_s,
        elapsed_s: t0.elapsed().as_secs_f64(),
        resumed_chunks,
    };
    std::fs::write(path(project), serde_json::to_vec_pretty(&t)?)?;
    std::fs::write(project.join("transcript.txt"), t.render())?;
    let _ = std::fs::remove_file(&partial);
    Ok(t)
}

/// An earlier run's header and the chunks it finished, or None when there is no readable partial
/// (missing, or a header this version cannot read). A torn last line (killed mid-write) is dropped.
fn read_partial(path: &Path) -> Option<(Header, Vec<Segment>)> {
    let body = std::fs::read_to_string(path).ok()?;
    let mut lines = body.lines();
    let h = serde_json::from_str::<Header>(lines.next()?).ok()?;
    Some((
        h,
        lines.filter_map(|l| serde_json::from_str(l).ok()).collect(),
    ))
}

fn read_span(file: &Path, track: usize, c: &Chunk) -> Result<Vec<f32>> {
    let span = Span {
        start: c.start,
        len: Some(c.end - c.start),
    };
    let mut r = PcmReader::open(file, track, vad::RATE, 1, span)?;
    let mut out = Vec::new();
    loop {
        let x = r.read(1 << 16)?;
        if x.is_empty() {
            break;
        }
        out.extend(x);
    }
    r.finish()?;
    Ok(out)
}

impl Transcript {
    /// One line per chunk: `[h:mm:ss] (lang) text`.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for seg in &self.segments {
            let text = seg.text();
            if text.is_empty() {
                continue;
            }
            let t = seg.start as u64;
            s.push_str(&format!(
                "[{}:{:02}:{:02}] ({}) {text}\n",
                t / 3600,
                t / 60 % 60,
                t % 60,
                seg.lang
            ));
        }
        s
    }

    pub fn words(&self) -> impl Iterator<Item = &Word> {
        self.segments.iter().flat_map(|s| &s.words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(languages: &str, model: &str) -> Header {
        Header {
            version: VERSION,
            languages: languages.into(),
            model: model.into(),
            device: Device::Cuda,
        }
    }

    #[test]
    fn partial_resume_drops_torn_line_and_foreign_header() {
        let dir = std::env::temp_dir().join(format!("junctur-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("p.jsonl");
        let h = header("es+en", "large-v3-turbo");
        let seg = Segment {
            track: 0,
            start: 1.0,
            end: 2.0,
            lang: "es".into(),
            model: "large-v3-turbo".into(),
            words: vec![],
        };
        let body = format!(
            "{}\n{}\n{{\"track\":0,\"sta",
            serde_json::to_string(&h).unwrap(),
            serde_json::to_string(&seg).unwrap()
        );
        std::fs::write(&p, body).unwrap();
        let (got, segs) = read_partial(&p).unwrap();
        assert_eq!((got.clone(), segs), (h.clone(), vec![seg]));
        assert!(got.same_run(&Header {
            device: Device::Cpu,
            ..h.clone()
        }));
        assert!(!got.same_run(&header("en", "large-v3-turbo")));
        assert!(!got.same_run(&header("es+en", "large-v3")));
        // A 1b partial (version 1, no model) is not resumed.
        std::fs::write(&p, "{\"version\":1,\"languages\":\"es+en\"}\n").unwrap();
        assert!(read_partial(&p).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finished_transcript_reuse_follows_the_model_and_languages() {
        let t = |model: &str| Transcript {
            choice: Choice {
                model: model.into(),
                device: Device::Cuda,
                note: None,
            },
            languages: Languages::default().allowed,
            segments: vec![],
            audio_s: 0.0,
            elapsed_s: 0.0,
            resumed_chunks: 0,
        };
        let opts = |model: ModelPref| TranscribeOptions {
            model,
            ..Default::default()
        };
        // Auto keeps whatever made it, a 1b q5_0 transcript included.
        assert!(reusable(&t("large-v3-q5_0"), &opts(ModelPref::Auto)));
        assert!(reusable(
            &t("large-v3-turbo"),
            &opts(ModelPref::LargeV3Turbo)
        ));
        // An explicit model that differs re-transcribes.
        assert!(!reusable(&t("large-v3-turbo"), &opts(ModelPref::LargeV3)));
        assert!(!reusable(
            &t("large-v3-q5_0"),
            &opts(ModelPref::LargeV3Turbo)
        ));
        let mut other = t("large-v3-turbo");
        other.languages = vec!["en".into()];
        assert!(!reusable(&other, &opts(ModelPref::Auto)));
    }
}
