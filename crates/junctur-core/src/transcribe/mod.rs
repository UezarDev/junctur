//! Speech to text with word timestamps, only over speech regions, resumable chunk by chunk.
//!
//! Every finished chunk is appended to `transcript.partial.jsonl` in the project folder before the
//! next starts, so an interrupted run (a 30h stream is hours of Whisper) loses one chunk at most.
//! The finished transcript is `transcript.json`, plus a plain `transcript.txt` to read.
//!
//! The recording is heard in 5-minute pieces (2j, [`PIECE`]): every track's chunks in one piece
//! before any of the next, so the editor can take the transcript a piece at a time ([`so_far`])
//! while the rest is heard.

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
    /// Hear the recording again (5aa: the Correct button): an interrupted run is not resumed and the
    /// finished transcript not reused, but it stays until the new one replaces it, unlike `force`,
    /// which deletes it first: a run stopped early still leaves the words it had.
    pub again: bool,
    /// The user's word list (5h), told to Whisper before every chunk. Neither in the partial's
    /// header nor in [`answers`]: a changed list applies to the next run and never restarts or
    /// outdates one.
    pub words: Vec<String>,
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

fn partial_path(project: &Path) -> std::path::PathBuf {
    project.join("transcript.partial.jsonl")
}

/// A run of `project`'s transcription was started and has not finished: its partial file is there,
/// and the next run resumes it.
pub fn interrupted(project: &Path) -> bool {
    partial_path(project).is_file()
}

/// Seconds of the recording heard as one piece (2j): Uezar's "5 minutes at a time". Every track's
/// chunks that start in a piece are transcribed before any chunk of the next one, so the first
/// piece is whole minutes in, not once track 1 is done hours later.
pub const PIECE: f64 = 300.0;

/// The piece a chunk starting at `t` seconds belongs to.
pub(crate) fn piece(t: f64) -> u64 {
    (t.max(0.0) / PIECE) as u64
}

/// The chunks a run in progress (or an interrupted one) has finished, from its partial file, sorted
/// as the finished transcript sorts them, so a word keeps its place when the whole one replaces
/// this; `None` when there is no readable partial. A torn last line is dropped, as a resume drops
/// it. For the editor while the rest is transcribed (2j).
pub fn so_far(project: &Path) -> Option<Vec<Segment>> {
    let (_, mut segs) = read_partial(&partial_path(project))?;
    sort(&mut segs);
    Some(segs)
}

/// The finished transcript's order: by start, then track.
fn sort(segs: &mut [Segment]) {
    segs.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.track.cmp(&b.track)));
}

/// What a run tells its caller as it goes besides the log: how far each track's transcript reaches
/// ([`Reached`]; every track at the start and a chunk's track after each chunk, 2h), and where the
/// transcript is whole on every track (2j): `transcribed(t)` says every word before `t` seconds is
/// in, at the start of a run with work to do and each time `t` enters a new [`PIECE`]. Not at the
/// end: the finished transcript says that. `between` is called before Whisper loads and after each
/// chunk while chunks remain, the only points a run can stop at (a chunk is one Whisper call); it
/// may block, and the run goes on when it returns (the app's pause, 2k).
pub struct Reports<'a> {
    pub reach: &'a mut dyn FnMut(usize, f64),
    pub transcribed: &'a mut dyn FnMut(f64),
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
    // One transcription of a recording at a time across processes (2j repair): the app resumes an
    // unfinished one on open, which must not start a second Whisper on the partial a `junctur
    // analyze` is writing. Held until this returns; the OS lets go if the process dies.
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
/// its lock is held. For the app, which resumes an unfinished transcription on open (2j repair).
pub fn transcribing_elsewhere(project: &Path) -> bool {
    match Transcribing::claim(project) {
        Ok(_) => false,
        Err(Error::Model(_)) => true,
        Err(_) => false,
    }
}

/// Every chunk of `sources` in the order Whisper hears them: piece by piece ([`PIECE`]), within a
/// piece track by track (in `sources`' order), within a track in time. Track by track inside a
/// piece keeps one decoder per track per piece (`audio::read` starts one per run of a track's
/// groups); chunk by chunk across tracks would seek for every chunk, the cost 1e removed.
fn order(sources: &[Source], total: f64) -> Vec<(usize, Chunk)> {
    let mut work: Vec<(usize, usize, Chunk)> = sources
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            plan(&s.regions, total)
                .into_iter()
                .map(move |c| (i, s.track, c))
        })
        .collect();
    // Stable: each track's chunks stay in time order.
    work.sort_by_key(|&(i, _, c)| (piece(c.start), i));
    work.into_iter().map(|(_, track, c)| (track, c)).collect()
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

/// What the log says about the word list in Whisper's prompt (5h); nothing without one.
fn prompt_line(p: &crate::wordlist::Prompt) -> Option<String> {
    let entries = |n: usize| format!("{n} entr{}", if n == 1 { "y" } else { "ies" });
    match (p.kept, p.left_out.len()) {
        (0, 0) => None,
        (k, 0) => Some(format!("word list: {} in Whisper's prompt", entries(k))),
        (k, n) => Some(format!(
            "word list: {} in Whisper's prompt, {} left out (the prompt is full; entries higher in the list win): {}",
            entries(k),
            entries(n),
            p.left_out.join(", ")
        )),
    }
}

/// Where a run calls [`Reports::between`]: once before Whisper loads, then after each chunk while
/// any remain, never after the last (a pause there would only hold the finishing back).
struct Stops<'a> {
    left: usize,
    between: &'a mut dyn FnMut(),
}

impl<'a> Stops<'a> {
    /// `left` chunks to do, at least one; the first stop is now.
    fn new(left: usize, between: &'a mut dyn FnMut()) -> Self {
        between();
        Self { left, between }
    }

    fn chunk_done(&mut self) {
        self.left = self.left.saturating_sub(1);
        if self.left > 0 {
            (self.between)();
        }
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
/// (everything before it is done, silence included), or [`f64::INFINITY`] once its last chunk is,
/// meaning the end of the track: only the caller knows the track's own length, which can differ
/// from the recording's. Not the latest chunk's end: a resumed run can have later chunks done than
/// earlier ones, and a track that goes quiet at 80 % would never read more than 80 %. Not a chunk
/// count: a chunk is 1 to 28 s.
struct Reached {
    tracks: Vec<TrackChunks>,
}

/// One track's chunks for [`Reached`]: their starts in time order, whether each is done, and the
/// first not done.
struct TrackChunks {
    track: usize,
    chunks: Vec<(f64, bool)>,
    first: usize,
}

impl TrackChunks {
    fn position(&self) -> f64 {
        self.chunks.get(self.first).map_or(f64::INFINITY, |c| c.0)
    }
}

impl Reached {
    /// From the run's own plan: `todo` says, per group's chunk, whether it still needs Whisper.
    fn new(sources: &[Source], groups: &[Group], todo: &[Vec<bool>]) -> Self {
        let tracks = sources
            .iter()
            .map(|s| {
                let mut chunks: Vec<(f64, bool)> = groups
                    .iter()
                    .zip(todo)
                    .filter(|(g, _)| g.track == s.track)
                    .flat_map(|(g, t)| g.chunks.iter().zip(t).map(|(c, &t)| (c.start, !t)))
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
        Self { tracks }
    }

    /// Every track and how far it reaches.
    fn all(&self) -> Vec<(usize, f64)> {
        self.tracks
            .iter()
            .map(|t| (t.track, t.position()))
            .collect()
    }

    /// Where the transcript is whole on every track: the least of their positions (2j). Every word
    /// still to come starts at or after it, as its chunk does. Infinite when every track is done.
    fn whole(&self) -> f64 {
        self.tracks
            .iter()
            .map(TrackChunks::position)
            .fold(f64::INFINITY, f64::min)
    }

    /// Marks `track`'s chunk at `start` done; how far the track reaches now, when it moved.
    fn finish(&mut self, track: usize, start: f64) -> Option<f64> {
        let t = self.tracks.iter_mut().find(|t| t.track == track)?;
        let before = t.position();
        if let Some(c) = t.chunks.iter_mut().find(|c| c.0 == start) {
            c.1 = true;
        }
        while t.chunks.get(t.first).is_some_and(|c| c.1) {
            t.first += 1;
        }
        let now = t.position();
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
    crate::output::rename_over(&tmp, path)?;
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

    /// Every word with its track in time order, ties in transcript order (a stable sort by start):
    /// the order the editor's words are in and `edit::Fixes` reads words in (5w).
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
    fn the_log_names_the_word_list_and_what_was_left_out() {
        use crate::wordlist::Prompt;
        let p = |kept, left: &[&str]| Prompt {
            text: String::new(),
            kept,
            left_out: left.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(prompt_line(&p(0, &[])), None, "no list, no line");
        assert_eq!(
            prompt_line(&p(1, &[])).unwrap(),
            "word list: 1 entry in Whisper's prompt"
        );
        let cut = prompt_line(&p(40, &["Junctur", "Resolve"])).unwrap();
        assert!(
            cut.starts_with("word list: 40 entries in Whisper's prompt, 2 entries left out"),
            "{cut}"
        );
        assert!(cut.ends_with(": Junctur, Resolve"), "{cut}");
    }

    #[test]
    fn a_run_stops_before_whisper_and_between_chunks_never_after_the_last() {
        let mut calls = 0;
        let mut between = || calls += 1;
        let mut stops = Stops::new(3, &mut between);
        stops.chunk_done();
        stops.chunk_done();
        stops.chunk_done();
        // Before the first chunk, after the first and after the second.
        assert_eq!(calls, 3);
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
        let groups = audio::groups(&work);
        let todo: Vec<Vec<bool>> = groups
            .iter()
            .map(|g| g.chunks.iter().map(|c| c.start != 50.0).collect())
            .collect();
        let mut r = Reached::new(&[source(0), source(1), source(2)], &groups, &todo);
        let end = f64::INFINITY;
        assert_eq!(r.all(), vec![(0, 0.0), (1, 30.0), (2, end)]);
        // The first chunk done: the silence after it and the resumed chunk are behind us too.
        assert_eq!(r.finish(0, 0.0), Some(200.0));
        // The last one: the whole track, though its speech ended at 210 s.
        assert_eq!(r.finish(0, 200.0), Some(end));
        assert_eq!(r.finish(1, 30.0), Some(end));
        // A chunk of a track not asked for, or done twice, moves nothing.
        assert_eq!(r.finish(5, 0.0), None);
        assert_eq!(r.finish(1, 30.0), None);
        assert_eq!(r.whole(), end);
    }

    #[test]
    fn the_transcript_is_whole_up_to_the_track_furthest_behind() {
        let chunk = |start: f64| Chunk {
            start,
            end: start + 10.0,
        };
        let source = |track| Source {
            track,
            regions: vec![],
        };
        let work = [(0, chunk(0.0)), (0, chunk(320.0)), (1, chunk(100.0))];
        let groups = audio::groups(&work);
        let todo: Vec<Vec<bool>> = groups.iter().map(|g| vec![true; g.chunks.len()]).collect();
        let mut r = Reached::new(&[source(0), source(1)], &groups, &todo);
        assert_eq!(r.whole(), 0.0);
        r.finish(0, 0.0);
        // Track 0 is at 320 s, track 1 still at 100 s: words before 100 s are all in.
        assert_eq!(r.whole(), 100.0);
        r.finish(1, 100.0);
        assert_eq!(r.whole(), 320.0);
        assert_eq!(piece(r.whole()), 1);
        // No tracks: nothing to wait for.
        assert_eq!(Reached::new(&[], &[], &[]).whole(), f64::INFINITY);
    }

    #[test]
    fn the_work_goes_piece_by_piece_and_no_language_group_spans_two() {
        use crate::vad::Region;
        let speech = |spans: &[(f64, f64)]| {
            spans
                .iter()
                .map(|&(start, end)| Region { start, end })
                .collect::<Vec<_>>()
        };
        let sources = [
            Source {
                track: 2,
                regions: speech(&[(10.0, 20.0), (400.0, 410.0), (900.0, 905.0)]),
            },
            Source {
                track: 0,
                regions: speech(&[(50.0, 60.0), (299.0, 299.5), (310.0, 320.0)]),
            },
        ];
        let got: Vec<(usize, u64)> = order(&sources, 1000.0)
            .iter()
            .map(|(t, c)| (*t, c.start as u64))
            .collect();
        // Piece 0 (0-5 min): track 2 then track 0, each in time; the chunk at 4:58.8 is piece 0's
        // though it ends after 5:00. Then piece 1, then piece 3 (piece 2 has no speech).
        assert_eq!(
            got,
            vec![(2, 9), (0, 49), (0, 298), (2, 399), (0, 309), (2, 899)]
        );
        // A piece's last group ends the piece: languages are never detected across a boundary.
        let groups = audio::groups(&order(&sources, 1000.0));
        assert!(groups.iter().all(|g| g
            .chunks
            .iter()
            .all(|c| piece(c.start) == piece(g.chunks[0].start))));
    }

    #[test]
    fn one_transcription_of_a_recording_at_a_time() {
        let dir = std::env::temp_dir().join(format!("junctur-tlock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!transcribing_elsewhere(&dir));
        let held = Transcribing::claim(&dir).unwrap();
        assert!(transcribing_elsewhere(&dir), "the lock is seen");
        assert!(
            Transcribing::claim(&dir).is_err(),
            "a second run is refused"
        );
        drop(held);
        assert!(!transcribing_elsewhere(&dir), "let go when done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn so_far_is_the_partial_in_the_finished_order() {
        let dir = std::env::temp_dir().join(format!("junctur-sofar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(so_far(&dir).is_none(), "no partial: nothing");
        let seg = |track, start: f64| Segment {
            track,
            start,
            end: start + 1.0,
            lang: "es".into(),
            model: "large-v3-turbo".into(),
            words: vec![],
        };
        // Written in Whisper's order (piece, then track); a run killed mid-line.
        let mut body = serde_json::to_string(&header("es+en", "large-v3-turbo")).unwrap() + "\n";
        for s in [seg(1, 5.0), seg(1, 40.0), seg(0, 20.0), seg(0, 5.0)] {
            body += &(serde_json::to_string(&s).unwrap() + "\n");
        }
        body += "{\"track\":0,\"st";
        std::fs::write(partial_path(&dir), body).unwrap();
        let got: Vec<(usize, f64)> = so_far(&dir)
            .unwrap()
            .iter()
            .map(|s| (s.track, s.start))
            .collect();
        assert_eq!(got, vec![(0, 5.0), (1, 5.0), (0, 20.0), (1, 40.0)]);
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

    #[test]
    fn again_drops_a_partial_and_never_reuses_the_finished_transcript() {
        // 5aa: the Correct button hears the recording again. With nothing to hear (no sources) the
        // run is a finish without Whisper, which shows what it starts from.
        let dir = std::env::temp_dir().join(format!("junctur-again-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = Transcript {
            choice: Choice {
                model: "large-v3-turbo".into(),
                device: Device::Cpu,
                note: None,
            },
            languages: Languages::default().allowed,
            segments: vec![],
            audio_s: 42.0,
            elapsed_s: 0.0,
            resumed_chunks: 0,
        };
        std::fs::write(path(&dir), serde_json::to_vec(&old).unwrap()).unwrap();
        let run = |opts: &TranscribeOptions| {
            transcribe(
                &dir.join("rec.mp4"),
                &[],
                10.0,
                &dir,
                &dir,
                opts,
                &mut |_| {},
                Reports {
                    reach: &mut |_, _| {},
                    transcribed: &mut |_| {},
                    between: &mut || {},
                },
            )
            .unwrap()
        };
        // Without it the finished transcript answers.
        assert_eq!(run(&TranscribeOptions::default()).audio_s, 42.0);
        // With it a partial of an older run is not resumed and the transcript is made anew.
        std::fs::write(partial_path(&dir), "torn").unwrap();
        assert!(interrupted(&dir));
        let opts = TranscribeOptions {
            again: true,
            ..Default::default()
        };
        assert_eq!(run(&opts).audio_s, 0.0);
        assert!(!interrupted(&dir));
        assert_eq!(load(&dir).unwrap().audio_s, 0.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
