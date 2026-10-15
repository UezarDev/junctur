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

use audio::Group;
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
    /// Hear again only the chunks this stretch (source seconds) touches, the finished transcript's
    /// other segments kept as they are ([`outside`]). Refused with no finished transcript, one that
    /// does not answer these options, or an unfinished run to resume.
    pub within: Option<(f64, f64)>,
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

/// What a transcription hears and where it keeps what it heard.
pub struct Job<'a> {
    /// The recording.
    pub file: &'a Path,
    pub sources: &'a [Source],
    /// The recording's length in seconds.
    pub total: f64,
    /// The project folder: the transcript, its partial and the lock live here.
    pub project: &'a Path,
    /// Where the models are kept.
    pub cache_dir: &'a Path,
    pub opts: &'a TranscribeOptions,
}

/// Transcribe the speech regions of `job.sources`, piece by piece ([`PIECE`]), resuming an
/// interrupted run and reusing a finished one that answers `job.opts`. `log` gets the model note
/// and progress every 5%; `reports` how far it has got ([`Reports`]).
pub fn transcribe(
    job: &Job,
    log: &mut dyn FnMut(&str),
    mut reports: Reports,
) -> Result<Transcript> {
    let Job {
        sources,
        total,
        project,
        opts,
        ..
    } = *job;
    let partial = partial_path(project);
    // One transcription of a recording at a time across processes, so the app resuming on open never
    // runs a second Whisper on a partial the CLI is writing. The OS lets go if the process dies.
    let _mine = Transcribing::claim(project)?;
    // A stretch heard again: the finished transcript's segments it does not touch go in as done.
    let mut kept: Option<Transcript> = None;
    if let (Some(_), false) = (opts.within, opts.force) {
        if partial.is_file() {
            return Err(Error::Model(UNFINISHED.into()));
        }
        let old = load(project).filter(|t| reusable(t, opts)).ok_or_else(|| {
            Error::Model(
                "no transcript answering the settings to transcribe a part of again".into(),
            )
        })?;
        kept = Some(old);
    } else if opts.force {
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
    let mut resume = resume_point(&partial, opts);
    let work = order(sources, total);
    let mut resumed_chunks = resume.done.len();
    let mut audio_s = 0.0 + work.iter().map(|(_, c)| c.end - c.start).sum::<f64>();
    if let (Some(old), Some((a, b))) = (kept, opts.within) {
        // Heard by the model that made the rest, whatever `Auto` would pick now: one transcript, one
        // model, as its `choice` says.
        let model = stretch_model(&old)?;
        resume.pref = model;
        resume.want.model = model.name();
        resume.done = outside(old.segments, &work, a, b);
        // Kept from the finished transcript, not resumed from an interrupted run.
        resumed_chunks = 0;
        let have = starts(&resume.done);
        let again: Vec<&Chunk> = work
            .iter()
            .filter(|(t, c)| !have.contains(&(*t, at(c.start))))
            .map(|(_, c)| c)
            .collect();
        // What Whisper is given this time, so the speed the CLI reports is this run's.
        audio_s = 0.0 + again.iter().map(|c| c.end - c.start).sum::<f64>();
        log(&format!(
            "transcribing again {} of the {} chunks ({:.0} s of speech) between {} and {}, with {}",
            again.len(),
            work.len(),
            audio_s,
            clock(a),
            clock(b),
            resume.want.model
        ));
    }
    let done = std::mem::take(&mut resume.done);
    let carried = done.len();

    let t0 = Instant::now();
    let groups = audio::groups(&work);
    // Per group, which of its chunks still need Whisper: those with no finished segment.
    let have = starts(&done);
    let todo: Vec<Vec<bool>> = groups
        .iter()
        .map(|g| {
            g.chunks
                .iter()
                .map(|c| !have.contains(&(g.track, at(c.start))))
                .collect()
        })
        .collect();
    let left = todo.iter().flatten().filter(|&&t| t).count();
    let reached = Reached::new(sources, &groups, &todo);
    for (track, at) in reached.all() {
        (reports.reach)(track, at);
    }
    // Where the transcript is whole on every track, and the piece that was last reported.
    let whole = reached.whole();
    if left > 0 {
        (reports.transcribed)(whole.min(total));
    }
    // Nothing left to hear: no GPU query, no model load. The choice is the one that did the work.
    let mut choice = Choice {
        model: resume.want.model.clone(),
        device: resume.started.as_ref().map_or(Device::Cpu, |h| h.device),
        note: None,
    };
    let mut tally = Tally {
        done,
        reached,
        whole,
    };
    if left > 0 {
        let plan = Plan {
            partial: &partial,
            groups: &groups,
            todo: &todo,
            chunks: work.len(),
            left,
            resumed: carried,
        };
        choice = run_chunks(job, &plan, &resume, &mut tally, log, &mut reports)?;
    }
    let mut done = tally.done;
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

/// Why a stretch is not transcribed again while an earlier transcription is unfinished, and how to
/// finish it: a run that redoes nothing resumes it (a whole re-transcription would hear everything
/// again instead).
pub const UNFINISHED: &str = "an earlier transcription of this recording is unfinished: finish it first (Re-analyse whole timeline with nothing ticked to redo resumes it, as reopening the recording does)";

/// The model a stretch of `old` is heard with: the one that made it, so the spliced transcript is one
/// model's (`Auto` picks turbo whatever made the rest). Refused for a name this version cannot load.
pub(crate) fn stretch_model(old: &Transcript) -> Result<ModelPref> {
    old.choice.model.parse().map_err(|_| {
        Error::Model(format!(
            "the transcript was made with {}, which this version cannot run: transcribe the whole timeline again",
            old.choice.model
        ))
    })
}

/// A chunk's or segment's start as a key: a segment's start is its chunk's, bit for bit (`-0.0`
/// read as `0.0`).
fn at(t: f64) -> u64 {
    (t + 0.0).to_bits()
}

/// The (track, start) of every segment in `segs`.
fn starts(segs: &[Segment]) -> std::collections::HashSet<(usize, u64)> {
    segs.iter().map(|s| (s.track, at(s.start))).collect()
}

/// One track's spans (chunks or segments) by start, for finding those that overlap a stretch
/// without reading them all: a span overlapping `[s, e)` starts before `e` and no earlier than `s`
/// less the longest span.
struct ByStart {
    /// Indices into the caller's list, sorted by start.
    idx: Vec<usize>,
    starts: Vec<f64>,
    ends: Vec<f64>,
    longest: f64,
}

impl ByStart {
    fn new(spans: impl Iterator<Item = (usize, f64, f64)>) -> Self {
        let mut v: Vec<(usize, f64, f64)> = spans.collect();
        v.sort_by(|x, y| x.1.total_cmp(&y.1));
        ByStart {
            idx: v.iter().map(|x| x.0).collect(),
            starts: v.iter().map(|x| x.1).collect(),
            ends: v.iter().map(|x| x.2).collect(),
            longest: v.iter().map(|x| x.2 - x.1).fold(0.0, f64::max),
        }
    }

    /// The caller's indices of the spans overlapping `[s, e)`.
    fn overlapping(&self, s: f64, e: f64) -> impl Iterator<Item = usize> + '_ {
        let upto = self.starts.partition_point(|&x| x < e);
        (0..upto)
            .rev()
            .take_while(move |&i| self.starts[i] >= s - self.longest)
            .filter(move |&i| self.ends[i] > s)
            .map(|i| self.idx[i])
    }
}

/// Each track's [`ByStart`] of `spans` given as (track, start, end).
fn by_track(
    spans: impl Iterator<Item = (usize, f64, f64)>,
) -> std::collections::HashMap<usize, ByStart> {
    let mut per: std::collections::HashMap<usize, Vec<(usize, f64, f64)>> = Default::default();
    for (i, (track, s, e)) in spans.enumerate() {
        per.entry(track).or_default().push((i, s, e));
    }
    per.into_iter()
        .map(|(t, v)| (t, ByStart::new(v.into_iter())))
        .collect()
}

/// The finished segments `old` a transcription of stretch `[a, b]` keeps: all but those on the chunks
/// of `work` it hears again. Those are the chunks the stretch touches, and any chunk with no segment
/// of its own in `old` (the old transcript was planned otherwise, or lost it); every old segment
/// overlapping one of them goes too, and every chunk that segment overlaps is heard again, until
/// nothing more moves. So the stretch widens to whole chunks (no word heard cut in half) and the
/// spliced transcript never holds a stretch's words twice. The caller's run hears the chunks with no
/// kept segment, which are exactly these. Indexed by track and start: a 30 h transcript has
/// thousands of each.
pub(crate) fn outside(old: Vec<Segment>, work: &[(usize, Chunk)], a: f64, b: f64) -> Vec<Segment> {
    let chunks = by_track(work.iter().map(|&(t, c)| (t, c.start, c.end)));
    let segs = by_track(old.iter().map(|s| (s.track, s.start, s.end)));
    let have = starts(&old);
    let mut redo = vec![false; work.len()];
    let mut queue: Vec<usize> = Vec::new();
    for (k, &(t, c)) in work.iter().enumerate() {
        if (c.start < b && a < c.end) || !have.contains(&(t, at(c.start))) {
            redo[k] = true;
            queue.push(k);
        }
    }
    let mut gone = vec![false; old.len()];
    while let Some(k) = queue.pop() {
        let (t, c) = work[k];
        let Some(on) = segs.get(&t) else { continue };
        for i in on.overlapping(c.start, c.end) {
            if gone[i] {
                continue;
            }
            gone[i] = true;
            let s = &old[i];
            for k2 in chunks[&t].overlapping(s.start, s.end) {
                if !redo[k2] {
                    redo[k2] = true;
                    queue.push(k2);
                }
            }
        }
    }
    old.into_iter()
        .zip(gone)
        .filter(|(_, g)| !g)
        .map(|(s, _)| s)
        .collect()
}

/// `h:mm:ss` of source seconds, for the log.
fn clock(s: f64) -> String {
    let t = s.max(0.0) as u64;
    format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

/// The chunks of a run with some left to hear.
struct Plan<'a> {
    partial: &'a Path,
    groups: &'a [Group],
    /// Per group, which of its chunks still need Whisper.
    todo: &'a [Vec<bool>],
    /// Every chunk of the run, done or not.
    chunks: usize,
    /// The chunks still to hear, at least one.
    left: usize,
    /// The chunks an interrupted run had finished.
    resumed: usize,
}

/// What a run has so far: its finished chunks and how far each track's transcript reaches.
struct Tally {
    done: Vec<Segment>,
    reached: Reached,
    /// Where the transcript is whole on every track.
    whole: f64,
}

/// Whisper over the chunks `plan` has left: the model chosen and loaded, the partial written afresh,
/// then every finished chunk appended to it, added to `tally` and reported. The choice it ran with.
fn run_chunks(
    job: &Job,
    plan: &Plan,
    resume: &Resume,
    tally: &mut Tally,
    log: &mut dyn FnMut(&str),
    reports: &mut Reports,
) -> Result<Choice> {
    let Reports {
        reach,
        transcribed,
        between,
    } = reports;
    let total = job.total;
    // A pause that came first holds here, before the model takes the card.
    let mut stops = Stops::new(plan.left, &mut **between);
    let (choice, model) = choose(resume.pref, job.opts.device);
    if let Some(note) = &choice.note {
        log(note);
    }
    let header = resume.started.clone().unwrap_or(Header {
        device: choice.device,
        ..resume.want.clone()
    });
    let mut w = Whisper::open(&models::ensure(model, job.cache_dir)?, choice.device)?;
    if let Some(line) = prompt_line(&w.set_prompt(&job.opts.words)) {
        log(&line);
    }
    let mut out = open_partial(plan.partial, &header, &tally.done)?;
    let mut progress = Progress::new(plan.chunks, log);
    progress.at(plan.resumed);
    let mut finished = |seg: Segment| -> Result<()> {
        // One write for the line and its newline, so a kill tears at most this line.
        out.write_all(format!("{}\n", serde_json::to_string(&seg)?).as_bytes())?;
        out.flush()?;
        if let Some(at) = tally.reached.finish(seg.track, seg.start) {
            reach(seg.track, at);
            let now = tally.reached.whole();
            // A new piece is whole (silence may have skipped several); the very end is the
            // finished transcript's to tell.
            if now.is_finite() && piece(now) > piece(tally.whole) {
                transcribed(now.min(total));
            }
            tally.whole = now;
        }
        tally.done.push(seg);
        progress.at(tally.done.len());
        stops.chunk_done();
        Ok(())
    };
    let run = Run {
        groups: plan.groups,
        todo: plan.todo,
        model: &choice.model,
        languages: &job.opts.languages,
    };
    run.chunks(job.file, &mut w, &mut finished)?;
    Ok(choice)
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
