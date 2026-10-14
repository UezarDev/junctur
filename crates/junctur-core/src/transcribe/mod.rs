//! Speech to text with word timestamps over speech regions. Each finished chunk is appended to
//! `transcript.partial.jsonl`, so an interrupted run loses one chunk at most, and the recording is
//! heard in [`PIECE`]s so the editor can take the transcript a piece at a time ([`so_far`]).

mod audio;
mod chunks;
mod languages;
mod partial;
mod run;
mod whisper;
mod words;

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub use chunks::{plan, Chunk};
pub use languages::Languages;
pub use partial::{so_far, SoFar};
pub use whisper::{choose, Choice, ModelPref, Whisper};
pub use words::Word;

use partial::{open_partial, resume_point, sort, Header, Resume};
use run::{order, prompt_line, Progress, Reached, Run, Stops};

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
    /// Hear the recording again without resuming or reusing, but unlike `force` keep the old
    /// transcript until the new one replaces it, so a run stopped early still leaves its words.
    pub again: bool,
    /// The user's word list, told to Whisper before every chunk. Not in the partial's header nor in
    /// [`answers`], so a changed list applies to the next run and never restarts or outdates one.
    pub words: Vec<String>,
}

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
/// is out of date.
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

fn partial_path(project: &Path) -> std::path::PathBuf {
    project.join("transcript.partial.jsonl")
}

/// A run of `project`'s transcription was started and has not finished: its partial file is there,
/// and the next run resumes it.
pub fn interrupted(project: &Path) -> bool {
    partial_path(project).is_file()
}

/// Seconds of the recording heard as one piece: every track's chunks that start in a piece are
/// transcribed before any of the next, so the first piece is whole minutes in, not hours.
pub const PIECE: f64 = 300.0;

/// The piece a chunk starting at `t` seconds belongs to.
pub(crate) fn piece(t: f64) -> u64 {
    (t.max(0.0) / PIECE) as u64
}

/// What a run tells its caller as it goes, besides the log.
pub struct Reports<'a> {
    /// How far a track's transcript reaches ([`Reached`]): every track at the start, then a chunk's.
    pub reach: &'a mut dyn FnMut(usize, f64),
    /// Every word before `t` seconds is in: at the start, and each time `t` enters a new [`PIECE`].
    pub transcribed: &'a mut dyn FnMut(f64),
    /// Called before Whisper loads and between chunks, the only points a run can stop at; it may
    /// block (the app's pause), and the run goes on when it returns.
    pub between: &'a mut dyn FnMut(),
}

/// Transcribe the speech regions of `sources`, piece by piece ([`PIECE`]), resuming an interrupted
/// run and reusing a finished one that answers `opts`. `log` gets the model note and progress every
/// 5%; `reports` how far it has got ([`Reports`]).
#[allow(clippy::too_many_arguments)]
pub fn transcribe(
    file: &Path,
    sources: &[Source],
    total: f64,
    project: &Path,
    cache_dir: &Path,
    opts: &TranscribeOptions,
    log: &mut dyn FnMut(&str),
    reports: Reports,
) -> Result<Transcript> {
    let Reports {
        reach,
        transcribed,
        between,
    } = reports;
    let partial = partial_path(project);
    // One transcription of a recording at a time across processes, so the app resuming on open never
    // runs a second Whisper on a partial the CLI is writing. The OS lets go if the process dies.
    let _mine = Transcribing::claim(project)?;
    if opts.force {
        let _ = std::fs::remove_file(&partial);
        let _ = std::fs::remove_file(path(project));
    } else if opts.again {
        let _ = std::fs::remove_file(&partial);
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

    let work = order(sources, total);
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
    let mut reached = Reached::new(sources, &groups, &todo);
    for (track, at) in reached.all() {
        reach(track, at);
    }
    // Where the transcript is whole on every track, and the piece that was last reported.
    let mut whole = reached.whole();
    if left > 0 {
        transcribed(whole.min(total));
    }
    // Nothing left to hear: no GPU query, no model load. The choice is the one that did the work.
    let mut choice = Choice {
        model: want.model.clone(),
        device: started.as_ref().map_or(Device::Cpu, |h| h.device),
        note: None,
    };
    if left > 0 {
        // A pause that came first holds here, before the model takes the card.
        let mut stops = Stops::new(left, &mut *between);
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
        if let Some(line) = prompt_line(&w.set_prompt(&opts.words)) {
            log(&line);
        }
        let mut out = open_partial(&partial, &header, &done)?;
        let mut progress = Progress::new(work.len(), log);
        progress.at(resumed_chunks);
        let mut finished = |seg: Segment| -> Result<()> {
            // One write for the line and its newline, so a kill tears at most this line.
            out.write_all(format!("{}\n", serde_json::to_string(&seg)?).as_bytes())?;
            out.flush()?;
            if let Some(at) = reached.finish(seg.track, seg.start) {
                reach(seg.track, at);
                let now = reached.whole();
                // A new piece is whole (silence may have skipped several); the very end is the
                // finished transcript's to tell.
                if now.is_finite() && piece(now) > piece(whole) {
                    transcribed(now.min(total));
                }
                whole = now;
            }
            done.push(seg);
            progress.at(done.len());
            stops.chunk_done();
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
    sort(&mut done);
    let t = Transcript {
        choice,
        languages: opts.languages.allowed.clone(),
        segments: done,
        audio_s,
        elapsed_s: t0.elapsed().as_secs_f64(),
        resumed_chunks,
    };
    crate::output::write_replacing(&path(project), serde_json::to_vec_pretty(&t)?)?;
    std::fs::write(project.join("transcript.txt"), t.render())?;
    // Left behind, the next run would take it for an interrupted one and redo the finishing (no
    // Whisper: every chunk is in it), and the editor would read it as words so far.
    if let Err(e) = crate::output::remove_retrying(&partial) {
        log(&format!("could not remove {}: {e}", partial.display()));
    }
    Ok(t)
}

/// The lock file a transcription holds in a project folder ([`Transcribing`]).
fn lock_path(project: &Path) -> std::path::PathBuf {
    project.join("transcript.lock")
}

/// A transcription of a project in progress, in this process: the OS lock on its lock file.
struct Transcribing {
    /// Held, never read: the lock goes with the handle.
    _file: std::fs::File,
}

impl Transcribing {
    fn claim(project: &Path) -> Result<Self> {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(project))?;
        match f.try_lock() {
            Ok(()) => Ok(Self { _file: f }),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::Model(
                "another Junctur is transcribing this recording".into(),
            )),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}

/// Whether another process (or another run here) is transcribing project folder `project` now:
/// its lock is held. For the app, which resumes an unfinished transcription on open.
pub fn transcribing_elsewhere(project: &Path) -> bool {
    match Transcribing::claim(project) {
        Ok(_) => false,
        Err(Error::Model(_)) => true,
        Err(_) => false,
    }
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

    /// Every word with its track in time order, ties in transcript order (a stable sort by start):
    /// the order the editor's words are in and `edit::Fixes` reads words in.
    pub fn in_order(&self) -> Vec<(usize, &Word)> {
        let mut out: Vec<(usize, &Word)> = self
            .segments
            .iter()
            .flat_map(|s| s.words.iter().map(move |w| (s.track, w)))
            .collect();
        out.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
        out
    }
}

#[cfg(test)]
mod tests;
