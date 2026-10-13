//! Speech to text with word timestamps, only over speech regions, resumable chunk by chunk.
//!
//! Every finished chunk is appended to `transcript.partial.jsonl` in the project folder before the
//! next starts, so an interrupted run (a 30h stream is hours of Whisper) loses one chunk at most.
//! The finished transcript is `transcript.json`, plus a plain `transcript.txt` to read.

mod audio;
mod chunks;
mod languages;
mod whisper;
mod words;

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use audio::{Group, Heard};
pub use chunks::{plan, Chunk};
pub use languages::Languages;
pub use whisper::{choose, Choice, ModelPref, Whisper};
pub use words::Word;

use crate::runtime::{Device, DevicePref};
use crate::vad::Region;
use crate::{models, Error, Result};

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
    answers(&t.choice.model, &t.languages, opts)
}

/// [`reusable`] on what a transcript records: the model that made it and its languages.
pub fn answers(model: &str, languages: &[String], opts: &TranscribeOptions) -> bool {
    languages == opts.languages.allowed.as_slice()
        && (opts.model == ModelPref::Auto || model == opts.model.name())
}

/// What a project's finished transcript was made with: its model and languages; `None` when there
/// is none, `Some(Err)` when there is one that cannot be read (a run would make it again). Reads
/// the file without building its segments (a 30 h transcript is tens of MB), for asking whether it
/// is out of date (2d).
pub fn made_with(project: &Path) -> Option<std::result::Result<(String, Vec<String>), String>> {
    #[derive(Deserialize)]
    struct Made {
        choice: MadeChoice,
        languages: Vec<String>,
    }
    #[derive(Deserialize)]
    struct MadeChoice {
        model: String,
    }
    let bytes = match std::fs::read(path(project)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(Err(e.to_string())),
    };
    Some(
        serde_json::from_slice::<Made>(&bytes)
            .map(|m| (m.choice.model, m.languages))
            .map_err(|e| e.to_string()),
    )
}

/// Where a project keeps the transcript.
pub fn path(project: &Path) -> std::path::PathBuf {
    project.join("transcript.json")
}

pub fn load(project: &Path) -> Option<Transcript> {
    serde_json::from_slice(&std::fs::read(path(project)).ok()?).ok()
}

/// Transcribe the speech regions of `sources`, resuming an interrupted run and reusing a finished
/// one that answers `opts`. `log` gets the model note and progress every 5%; `reach` is told how
/// far each track's transcript reaches ([`Reached`]), every track at the start and a chunk's track
/// after each chunk (2h).
#[allow(clippy::too_many_arguments)]
pub fn transcribe(
    file: &Path,
    sources: &[Source],
    total: f64,
    project: &Path,
    cache_dir: &Path,
    opts: &TranscribeOptions,
    log: &mut dyn FnMut(&str),
    reach: &mut dyn FnMut(usize, f64),
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
    let Resume {
        pref,
        want,
        started,
        mut done,
    } = resume_point(&partial, opts);
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
    let groups = audio::groups(&work);
    // Per group, which of its chunks still need Whisper.
    let todo: Vec<Vec<bool>> = groups
        .iter()
        .map(|g| {
            g.chunks
                .iter()
                .map(|c| {
                    !done
                        .iter()
                        .any(|s| s.track == g.track && s.start == c.start)
                })
                .collect()
        })
        .collect();
    let left = todo.iter().flatten().filter(|&&t| t).count();
    let mut reached = Reached::new(sources, &work, &done, total);
    for (track, at) in reached.all() {
        reach(track, at);
    }
    // Nothing left to hear: no GPU query, no model load. The choice is the one that did the work.
    let mut choice = Choice {
        model: want.model.clone(),
        device: started.as_ref().map_or(Device::Cpu, |h| h.device),
        note: None,
    };
    if left > 0 {
        let model;
        (choice, model) = choose(pref, opts.device);
        if let Some(note) = &choice.note {
            log(note);
        }
        let header = started.clone().unwrap_or(Header {
            device: choice.device,
            ..want.clone()
        });
        let mut w = Whisper::open(&models::ensure(model, cache_dir)?, choice.device)?;
        let mut out = open_partial(&partial, &header, &done)?;
        let mut progress = Progress::new(work.len(), log);
        progress.at(resumed_chunks);
        let mut finished = |seg: Segment| -> Result<()> {
            // One write for the line and its newline, so a kill tears at most this line.
            out.write_all(format!("{}\n", serde_json::to_string(&seg)?).as_bytes())?;
            out.flush()?;
            if let Some(at) = reached.finish(seg.track, seg.start) {
                reach(seg.track, at);
            }
            done.push(seg);
            progress.at(done.len());
            Ok(())
        };
        let run = Run {
            groups: &groups,
            todo: &todo,
            model: &choice.model,
            languages: &opts.languages,
        };
        run.chunks(file, &mut w, &mut finished)?;
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

/// Where a run starts: the model to use, the header it writes, and the chunks an interrupted run
/// already finished (`started` is that run's header, when it is resumed).
struct Resume {
    pref: ModelPref,
    want: Header,
    started: Option<Header>,
    done: Vec<Segment>,
}

fn resume_point(partial: &Path, opts: &TranscribeOptions) -> Resume {
    let prior = read_partial(partial);
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
    let (started, done) = match prior {
        Some((h, segs)) if h.same_run(&want) => (Some(h), segs),
        _ => (None, Vec::new()),
    };
    Resume {
        pref,
        want,
        started,
        done,
    }
}

/// The chunks still to transcribe, grouped for language detection.
struct Run<'a> {
    groups: &'a [Group],
    todo: &'a [Vec<bool>],
    model: &'a str,
    languages: &'a Languages,
}

/// Groups decoded ahead of Whisper: enough to keep the GPU fed, little memory (30 s each).
const AHEAD: usize = 4;

impl Run<'_> {
    /// Whisper over every group with a chunk to do, fed by one decoding thread; each finished
    /// chunk goes to `finished` as it is done.
    fn chunks(
        &self,
        file: &Path,
        w: &mut Whisper,
        finished: &mut dyn FnMut(Segment) -> Result<()>,
    ) -> Result<()> {
        let wanted = |k: usize| self.todo[k].iter().any(|&t| t);
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::sync_channel(AHEAD);
            let stop = &stop;
            let reader = scope.spawn(move || audio::read(file, self.groups, &wanted, stop, &tx));
            let heard = self.consume(rx, w, finished);
            // Whisper is done or failed: the reader stops at its next decoder read.
            stop.store(true, Ordering::Relaxed);
            // Joined here, so a panicking reader is an error `Jobs::stage` records, not an unwind.
            let joined = reader.join();
            heard?;
            joined.map_err(|_| Error::Model("the transcription reader thread panicked".into()))
        })
    }

    /// Whisper on each group as the reader sends it. Takes `rx` so it is dropped on return.
    fn consume(
        &self,
        rx: mpsc::Receiver<Result<Heard>>,
        w: &mut Whisper,
        finished: &mut dyn FnMut(Segment) -> Result<()>,
    ) -> Result<()> {
        for heard in rx {
            let Heard { group, samples } = heard?;
            let g = &self.groups[group];
            let lang = self.language(w, &samples)?;
            for ((c, s), &todo) in g.chunks.iter().zip(&samples).zip(&self.todo[group]) {
                if todo {
                    finished(self.segment(w, s, g.track, c, &lang)?)?;
                }
            }
        }
        Ok(())
    }

    /// The language of a group, from all its audio at once (Whisper's 30 s window).
    fn language(&self, w: &mut Whisper, samples: &[Vec<f32>]) -> Result<String> {
        let joined = samples.concat();
        if joined.is_empty() {
            // Past the end of the track: nothing to hear, nothing to detect.
            return Ok(self.languages.allowed[0].clone());
        }
        w.detect(&joined, self.languages)
    }

    /// Whisper on one chunk's samples, with word times moved onto the recording's timeline.
    fn segment(
        &self,
        w: &mut Whisper,
        samples: &[f32],
        track: usize,
        c: &Chunk,
        lang: &str,
    ) -> Result<Segment> {
        let words = if samples.is_empty() {
            Vec::new()
        } else {
            w.transcribe(samples, lang)?
        };
        Ok(Segment {
            track,
            start: c.start,
            end: c.end,
            lang: lang.to_string(),
            model: self.model.to_string(),
            words: words
                .into_iter()
                .map(|x| Word {
                    start: c.start + x.start,
                    end: c.start + x.end,
                    ..x
                })
                .collect(),
        })
    }
}

/// "transcribing n/total chunks (p%)" through `log`, every 5% and at the end.
struct Progress<'a> {
    total: usize,
    last: usize,
    log: &'a mut dyn FnMut(&str),
}

impl<'a> Progress<'a> {
    fn new(total: usize, log: &'a mut dyn FnMut(&str)) -> Self {
        Self {
            total,
            last: 0,
            log,
        }
    }

    fn at(&mut self, done: usize) {
        let pct = done * 100 / self.total.max(1);
        if pct >= self.last + 5 || done == self.total {
            self.last = pct;
            (self.log)(&format!(
                "transcribing {done}/{} chunks ({pct}%)",
                self.total
            ));
        }
    }
}

/// How far each track's transcript reaches: the start of its earliest chunk not transcribed yet
/// (everything before it is done, silence included), or `total` once its last chunk is. Not the
/// latest chunk's end: a resumed run can have later chunks done than earlier ones, and a track that
/// goes quiet at 80 % would never read more than 80 %. Not a chunk count: a chunk is 1 to 28 s.
struct Reached {
    tracks: Vec<TrackChunks>,
    total: f64,
}

/// One track's chunks for [`Reached`]: their starts in time order, whether each is done, and the
/// first not done.
struct TrackChunks {
    track: usize,
    chunks: Vec<(f64, bool)>,
    first: usize,
}

impl TrackChunks {
    fn position(&self, total: f64) -> f64 {
        self.chunks.get(self.first).map_or(total, |c| c.0)
    }
}

impl Reached {
    fn new(sources: &[Source], work: &[(usize, Chunk)], done: &[Segment], total: f64) -> Self {
        let tracks = sources
            .iter()
            .map(|s| {
                let mut chunks: Vec<(f64, bool)> = work
                    .iter()
                    .filter(|(t, _)| *t == s.track)
                    .map(|(_, c)| {
                        let finished = done
                            .iter()
                            .any(|d| d.track == s.track && d.start == c.start);
                        (c.start, finished)
                    })
                    .collect();
                chunks.sort_by(|a, b| a.0.total_cmp(&b.0));
                let first = chunks.iter().take_while(|c| c.1).count();
                TrackChunks {
                    track: s.track,
                    chunks,
                    first,
                }
            })
            .collect();
        Self { tracks, total }
    }

    /// Every track and how far it reaches.
    fn all(&self) -> Vec<(usize, f64)> {
        self.tracks
            .iter()
            .map(|t| (t.track, t.position(self.total)))
            .collect()
    }

    /// Marks `track`'s chunk at `start` done; how far the track reaches now, when it moved.
    fn finish(&mut self, track: usize, start: f64) -> Option<f64> {
        let total = self.total;
        let t = self.tracks.iter_mut().find(|t| t.track == track)?;
        let before = t.position(total);
        if let Some(c) = t.chunks.iter_mut().find(|c| c.0 == start) {
            c.1 = true;
        }
        while t.chunks.get(t.first).is_some_and(|c| c.1) {
            t.first += 1;
        }
        let now = t.position(total);
        (now != before).then_some(now)
    }
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

/// Write the partial file afresh (`header`, then the chunks already done) and open it for
/// appending. Rewritten rather than appended to: a killed run leaves a torn last line, and the next
/// chunk appended to it shared its line, so both were lost at the next resume (1d).
fn open_partial(path: &Path, header: &Header, done: &[Segment]) -> Result<std::fs::File> {
    let mut body = serde_json::to_string(header)? + "\n";
    for seg in done {
        body += &serde_json::to_string(seg)?;
        body.push('\n');
    }
    // Through a temp file, so a kill now still leaves the old partial to resume from.
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(std::fs::OpenOptions::new().append(true).open(path)?)
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
        assert_eq!((got.clone(), segs), (h.clone(), vec![seg.clone()]));
        assert!(got.same_run(&Header {
            device: Device::Cpu,
            ..h.clone()
        }));
        assert!(!got.same_run(&header("en", "large-v3-turbo")));
        assert!(!got.same_run(&header("es+en", "large-v3")));
        // Resuming rewrites the torn partial, so the next chunk gets a line of its own; so does a
        // last line that is whole but lost its newline.
        let next = Segment {
            start: 3.0,
            end: 4.0,
            ..seg.clone()
        };
        let append = |segs: &[Segment]| {
            let mut out = open_partial(&p, &h, segs).unwrap();
            out.write_all(format!("{}\n", serde_json::to_string(&next).unwrap()).as_bytes())
                .unwrap();
        };
        append(std::slice::from_ref(&seg));
        let (_, segs) = read_partial(&p).unwrap();
        assert_eq!(segs, vec![seg.clone(), next.clone()]);
        let unterminated = format!(
            "{}\n{}",
            serde_json::to_string(&h).unwrap(),
            serde_json::to_string(&seg).unwrap()
        );
        std::fs::write(&p, unterminated).unwrap();
        let (_, segs) = read_partial(&p).unwrap();
        append(&segs);
        assert_eq!(read_partial(&p).unwrap().1, vec![seg.clone(), next]);
        // A 1b partial (version 1, no model) is not resumed.
        std::fs::write(&p, "{\"version\":1,\"languages\":\"es+en\"}\n").unwrap();
        assert!(read_partial(&p).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_track_reaches_its_first_chunk_not_done_and_its_end_when_all_are() {
        let chunk = |start: f64| Chunk {
            start,
            end: start + 10.0,
        };
        let seg = |track, start| Segment {
            track,
            start,
            end: start + 10.0,
            lang: "es".into(),
            model: "m".into(),
            words: vec![],
        };
        let source = |track| Source {
            track,
            regions: vec![],
        };
        // Track 0: chunks at 0, 50 and 200; the one at 50 done by an interrupted run. Track 1: one
        // chunk. Track 2: no speech, so nothing to wait for.
        let work = [
            (0, chunk(0.0)),
            (0, chunk(50.0)),
            (0, chunk(200.0)),
            (1, chunk(30.0)),
        ];
        let mut r = Reached::new(
            &[source(0), source(1), source(2)],
            &work,
            &[seg(0, 50.0)],
            300.0,
        );
        assert_eq!(r.all(), vec![(0, 0.0), (1, 30.0), (2, 300.0)]);
        // The first chunk done: the silence after it and the resumed chunk are behind us too.
        assert_eq!(r.finish(0, 0.0), Some(200.0));
        // The last one: the whole track, though its speech ended at 210 s.
        assert_eq!(r.finish(0, 200.0), Some(300.0));
        assert_eq!(r.finish(1, 30.0), Some(300.0));
        // A chunk of a track not asked for, or done twice, moves nothing.
        assert_eq!(r.finish(5, 0.0), None);
        assert_eq!(r.finish(1, 30.0), None);
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
