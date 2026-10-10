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
use crate::runtime::DevicePref;
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

/// First line of the partial file: a partial made with other languages is not resumed.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Header {
    version: u32,
    languages: String,
}

const VERSION: u32 = 1;

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
    } else if let Some(t) = load(project) {
        return Ok(t);
    }
    let header = Header {
        version: VERSION,
        languages: opts.languages.key(),
    };
    let mut done = read_partial(&partial, &header);
    if done.is_empty() {
        std::fs::write(&partial, format!("{}\n", serde_json::to_string(&header)?))?;
    }
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
    let (choice, model) = choose(opts.model, opts.device);
    let t0 = Instant::now();
    let todo: Vec<_> = work
        .iter()
        .filter(|(t, c)| !done.iter().any(|s| s.track == *t && s.start == c.start))
        .collect();
    if !todo.is_empty() {
        if let Some(note) = &choice.note {
            eprintln!("junctur: {note}");
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

/// Chunks already finished by an earlier run. A torn last line (killed mid-write) is dropped.
fn read_partial(path: &Path, header: &Header) -> Vec<Segment> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines = body.lines();
    match lines
        .next()
        .and_then(|l| serde_json::from_str::<Header>(l).ok())
    {
        Some(h) if h == *header => lines.filter_map(|l| serde_json::from_str(l).ok()).collect(),
        _ => Vec::new(),
    }
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

    #[test]
    fn partial_resume_drops_torn_line_and_foreign_header() {
        let dir = std::env::temp_dir().join(format!("junctur-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("p.jsonl");
        let h = Header {
            version: VERSION,
            languages: "es+en".into(),
        };
        let seg = Segment {
            track: 0,
            start: 1.0,
            end: 2.0,
            lang: "es".into(),
            model: "m".into(),
            words: vec![],
        };
        let body = format!(
            "{}\n{}\n{{\"track\":0,\"sta",
            serde_json::to_string(&h).unwrap(),
            serde_json::to_string(&seg).unwrap()
        );
        std::fs::write(&p, body).unwrap();
        assert_eq!(read_partial(&p, &h), vec![seg]);
        let other = Header {
            version: VERSION,
            languages: "en".into(),
        };
        assert!(read_partial(&p, &other).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
