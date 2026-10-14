//! Whisper over the chunks still to do: the order they are heard in, one decoding thread feeding
//! it, and what the run reports as chunks finish.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use super::audio::{self, Group, Heard};
use super::{piece, plan, Chunk, Languages, Segment, Source, Whisper, Word};
use crate::{Error, Result};

/// Every chunk of `sources` in the order Whisper hears them: piece by piece ([`PIECE`]), within a
/// piece track by track (in `sources`' order), within a track in time. Track by track inside a
/// piece keeps one decoder per track per piece (`audio::read` starts one per run of a track's
/// groups); chunk by chunk across tracks would seek for every chunk.
pub(super) fn order(sources: &[Source], total: f64) -> Vec<(usize, Chunk)> {
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

/// The chunks still to transcribe, grouped for language detection.
pub(super) struct Run<'a> {
    pub(super) groups: &'a [Group],
    pub(super) todo: &'a [Vec<bool>],
    pub(super) model: &'a str,
    pub(super) languages: &'a Languages,
}

/// Groups decoded ahead of Whisper: enough to keep the GPU fed, little memory (30 s each).
const AHEAD: usize = 4;

impl Run<'_> {
    /// Whisper over every group with a chunk to do, fed by one decoding thread; each finished
    /// chunk goes to `finished` as it is done.
    pub(super) fn chunks(
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
pub(super) fn prompt_line(p: &crate::wordlist::Prompt) -> Option<String> {
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
pub(super) struct Stops<'a> {
    left: usize,
    between: &'a mut dyn FnMut(),
}

impl<'a> Stops<'a> {
    /// `left` chunks to do, at least one; the first stop is now.
    pub(super) fn new(left: usize, between: &'a mut dyn FnMut()) -> Self {
        between();
        Self { left, between }
    }

    pub(super) fn chunk_done(&mut self) {
        self.left = self.left.saturating_sub(1);
        if self.left > 0 {
            (self.between)();
        }
    }
}

/// "transcribing n/total chunks (p%)" through `log`, every 5% and at the end.
pub(super) struct Progress<'a> {
    total: usize,
    last: usize,
    log: &'a mut dyn FnMut(&str),
}

impl<'a> Progress<'a> {
    pub(super) fn new(total: usize, log: &'a mut dyn FnMut(&str)) -> Self {
        Self {
            total,
            last: 0,
            log,
        }
    }

    pub(super) fn at(&mut self, done: usize) {
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
pub(super) struct Reached {
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
    pub(super) fn new(sources: &[Source], groups: &[Group], todo: &[Vec<bool>]) -> Self {
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
    pub(super) fn all(&self) -> Vec<(usize, f64)> {
        self.tracks
            .iter()
            .map(|t| (t.track, t.position()))
            .collect()
    }

    /// Where the transcript is whole on every track: the least of their positions. Every word
    /// still to come starts at or after it, as its chunk does. Infinite when every track is done.
    pub(super) fn whole(&self) -> f64 {
        self.tracks
            .iter()
            .map(TrackChunks::position)
            .fold(f64::INFINITY, f64::min)
    }

    /// Marks `track`'s chunk at `start` done; how far the track reaches now, when it moved.
    pub(super) fn finish(&mut self, track: usize, start: f64) -> Option<f64> {
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
