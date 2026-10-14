//! Speech to text with word timestamps over speech regions. Each finished chunk is appended to
//! `transcript.partial.jsonl`, so an interrupted run loses one chunk at most, and the recording is
//! heard in [`PIECE`]s so the editor can take the transcript a piece at a time ([`so_far`]).

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
    /// Hear the recording again without resuming or reusing, but unlike `force` keep the old
    /// transcript until the new one replaces it, so a run stopped early still leaves its words.
    pub again: bool,
    /// The user's word list, told to Whisper before every chunk. Not in the partial's header nor in
    /// [`answers`], so a changed list applies to the next run and never restarts or outdates one.
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

/// A version-1 partial names no model, so it is not resumed.
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

/// The chunks a run in progress (or an interrupted one) has finished, from its partial file, sorted
/// as the finished transcript sorts them, so a word keeps its place when the whole one replaces
/// this; `None` when there is no readable partial. A torn last line is dropped, as a resume drops
/// it. For the editor while the rest is transcribed.
pub fn so_far(project: &Path) -> Option<Vec<Segment>> {
    let (_, mut segs) = read_partial(&partial_path(project))?;
    sort(&mut segs);
    Some(segs)
}

/// [`so_far`] for a reader that asks again as the run appends: only the lines added since its last
/// read are parsed. What it parsed is checked against the file's start each time (length and hash),
/// so a partial written afresh (a new run, [`open_partial`]) is read again whole.
#[derive(Debug, Default)]
pub struct SoFar {
    path: std::path::PathBuf,
    /// Bytes parsed: whole lines, through the last newline read.
    parsed: usize,
    hash: u64,
    /// The parsed chunks in [`sort`]'s order.
    segs: Vec<Segment>,
}

impl SoFar {
    /// What [`so_far`] gives for `project` now.
    pub fn read(&mut self, project: &Path) -> Option<std::borrow::Cow<'_, [Segment]>> {
        use std::borrow::Cow;
        let path = partial_path(project);
        let Ok(body) = std::fs::read_to_string(&path) else {
            *self = Self::default();
            return None;
        };
        let same = self.path == path
            && self.parsed > 0
            && body.len() >= self.parsed
            && hash(&body.as_bytes()[..self.parsed]) == self.hash;
        if !same {
            *self = Self {
                path,
                ..Self::default()
            };
            // The header is the first line, and there is no partial without one that reads.
            let end = body.find('\n').map_or(body.len(), |i| i + 1);
            serde_json::from_str::<Header>(body[..end].lines().next()?).ok()?;
            if body[..end].ends_with('\n') {
                self.parsed = end;
            }
        }
        // Whole lines are kept; a last line without its newline is parsed but read again next time,
        // as it may still be being written.
        let rest = &body[self.parsed..];
        let whole = rest.rfind('\n').map_or(0, |i| i + 1);
        let new: Vec<Segment> = rest[..whole]
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if !new.is_empty() {
            // Stable: the parsed ones, then the new, is the file's order among equal keys.
            self.segs.extend(new);
            sort(&mut self.segs);
        }
        self.parsed += whole;
        self.hash = hash(&body.as_bytes()[..self.parsed]);
        let torn = rest[whole..]
            .lines()
            .next()
            .and_then(|l| serde_json::from_str::<Segment>(l).ok());
        Some(match torn {
            Some(seg) => {
                let mut all = self.segs.clone();
                all.push(seg);
                sort(&mut all);
                Cow::Owned(all)
            }
            None => Cow::Borrowed(&self.segs),
        })
    }
}

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// The finished transcript's order: by start, then track.
fn sort(segs: &mut [Segment]) {
    segs.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.track.cmp(&b.track)));
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

/// Every chunk of `sources` in the order Whisper hears them: piece by piece ([`PIECE`]), within a
/// piece track by track (in `sources`' order), within a track in time. Track by track inside a
/// piece keeps one decoder per track per piece (`audio::read` starts one per run of a track's
/// groups); chunk by chunk across tracks would seek for every chunk.
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

/// What the log says about the word list in Whisper's prompt; nothing without one.
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

/// How far each track's transcript reaches: the start of its earliest chunk not done yet, or
/// [`f64::INFINITY`] (the track's end, whose length only the caller knows) once all are.
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

    /// Where the transcript is whole on every track: the least of their positions. Every word
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
/// appending. Rewritten rather than appended to: a killed run leaves a torn last line, which the
/// next chunk would share and be lost with.
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
