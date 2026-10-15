//! Chunk audio for Whisper: each track decoded once, front to back, on a thread of its own that
//! hands Whisper one group of chunks at a time. One seek per track keeps the GPU from waiting on an
//! ffmpeg start per chunk, and every chunk's sample `i` exactly at `start + i / 16000`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;

use super::Chunk;
use crate::ffmpeg::{PcmReader, Span};
use crate::vad;
use crate::{Error, Result};

/// Chunks that share one language detection: consecutive chunks of one track, together at most
/// Whisper's 30 s window and within [`GROUP_SPAN`] of each other.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub track: usize,
    pub chunks: Vec<Chunk>,
}

/// Audio a language is detected on: Whisper's whole window. A short "sí" alone is a coin toss
/// between Spanish and English; 30 s is not.
pub const GROUP_AUDIO: f64 = 30.0;
/// A group never reaches further than this, so a recording that switches language is followed.
pub const GROUP_SPAN: f64 = 120.0;

/// Group each track's chunks (in time order) for language detection. A group never crosses into
/// another piece of the recording ([`super::piece`]), so a piece is whole when its last group is.
pub fn groups(work: &[(usize, Chunk)]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for &(track, c) in work {
        if let Some(g) = out.last_mut().filter(|g| g.track == track) {
            let audio: f64 = g.chunks.iter().map(|x| x.end - x.start).sum();
            let first = g.chunks[0].start;
            if audio + (c.end - c.start) <= GROUP_AUDIO
                && c.end - first <= GROUP_SPAN
                && super::piece(c.start) == super::piece(first)
            {
                g.chunks.push(c);
                continue;
            }
        }
        out.push(Group {
            track,
            chunks: vec![c],
        });
    }
    out
}

/// A group's audio, one sample vector per chunk (16 kHz mono).
pub struct Heard {
    /// Index into the groups given to [`read`].
    pub group: usize,
    pub samples: Vec<Vec<f32>>,
}

/// Decode the groups whose index `wanted` accepts, track by track, sending each as it is read.
/// Each track gets one ffmpeg, started at its first wanted group. Stops once `stop` is set or the
/// receiver is gone (Whisper failed): `stop` is checked between decoder reads, so a sparse track does
/// not decode on for hours to the next send.
pub fn read(
    file: &Path,
    groups: &[Group],
    wanted: &dyn Fn(usize) -> bool,
    stop: &AtomicBool,
    tx: &SyncSender<Result<Heard>>,
) {
    let mut i = 0;
    while i < groups.len() {
        let track = groups[i].track;
        let end = (i..groups.len())
            .find(|&k| groups[k].track != track)
            .unwrap_or(groups.len());
        let r = read_track(file, track, &groups[..end], i, wanted, stop, tx);
        if let Err(e) = r {
            let _ = tx.send(Err(e));
            return;
        }
        i = end;
    }
}

/// `groups[from..]` (all on `track`). Err only for a decode failure; a gone receiver is Ok.
fn read_track(
    file: &Path,
    track: usize,
    groups: &[Group],
    from: usize,
    wanted: &dyn Fn(usize) -> bool,
    stop: &AtomicBool,
    tx: &SyncSender<Result<Heard>>,
) -> Result<()> {
    let Some(first) = (from..groups.len()).find(|&k| wanted(k)) else {
        return Ok(());
    };
    // The decoder is started at whole milliseconds (`PcmReader` writes the seek with `{:.3}`); the
    // stream counts from the same instant.
    let origin = (groups[first].chunks[0].start * 1000.0).round() / 1000.0;
    let span = Span {
        start: origin,
        len: None,
    };
    let reader = PcmReader::open(file, track, vad::RATE, 1, span)?;
    let mut stream = Stream::new(reader, origin, stop);
    for (k, g) in groups.iter().enumerate().skip(first) {
        if !wanted(k) {
            continue;
        }
        let mut samples = Vec::with_capacity(g.chunks.len());
        for c in &g.chunks {
            samples.push(stream.take(c.start, c.end)?);
        }
        if tx.send(Ok(Heard { group: k, samples })).is_err() {
            return Ok(());
        }
    }
    // Not `finish`: the decoder may still be running past the last chunk; dropping stops it. A
    // decoder that ended early was checked when it ended.
    Ok(())
}

/// Where samples come from: the decoder, or a stand-in in tests.
pub trait Decode {
    /// Fill `buf` with up to `n` samples; empty at the end.
    fn read_into(&mut self, buf: &mut Vec<f32>, n: usize) -> Result<()>;
    /// The stream ended: Err when it ended because the decode failed.
    fn ended(&mut self) -> Result<()>;
}

impl Decode for PcmReader {
    fn read_into(&mut self, buf: &mut Vec<f32>, n: usize) -> Result<()> {
        PcmReader::read_into(self, buf, n)
    }
    fn ended(&mut self) -> Result<()> {
        self.wait()
    }
}

/// A forward reader over one decoded track, addressed in seconds from `origin`.
pub struct Stream<'a, S> {
    src: S,
    origin: f64,
    /// Set when nobody wants the samples any more.
    stop: &'a AtomicBool,
    /// Samples from `base` on, read but not yet taken.
    buf: Vec<f32>,
    base: u64,
    block: Vec<f32>,
    eof: bool,
}

/// Samples per read from the decoder.
const BLOCK: usize = vad::RATE as usize;

impl<'a, S: Decode> Stream<'a, S> {
    pub fn new(src: S, origin: f64, stop: &'a AtomicBool) -> Self {
        Self {
            src,
            origin,
            stop,
            buf: Vec::new(),
            base: 0,
            block: Vec::with_capacity(BLOCK),
            eof: false,
        }
    }

    fn at(&self, t: f64) -> u64 {
        ((t - self.origin).max(0.0) * vad::RATE as f64).round() as u64
    }

    /// The samples of `[start, end)` seconds; shorter (or empty) past the end of the track. Calls
    /// must move forward.
    pub fn take(&mut self, start: f64, end: f64) -> Result<Vec<f32>> {
        let (from, to) = (self.at(start), self.at(end));
        while !self.eof && self.base + (self.buf.len() as u64) < to {
            // Nothing before `from` is wanted again: drop it before reading more, so a long
            // silence between chunks is never held.
            if self.stop.load(Ordering::Relaxed) {
                return Err(Error::Model("transcription stopped".into()));
            }
            let dead = from.saturating_sub(self.base).min(self.buf.len() as u64) as usize;
            self.buf.drain(..dead);
            self.base += dead as u64;
            self.src.read_into(&mut self.block, BLOCK)?;
            if self.block.is_empty() {
                self.eof = true;
                self.src.ended()?;
            }
            // Wholly before `from` (a gap): skip without keeping.
            let end = self.base + (self.buf.len() + self.block.len()) as u64;
            if self.buf.is_empty() && end <= from {
                self.base = end;
            } else {
                self.buf.extend_from_slice(&self.block);
            }
        }
        assert!(from >= self.base, "chunk {start} s read out of order");
        let have = self.base + self.buf.len() as u64;
        let lo = (from.min(have) - self.base) as usize;
        let hi = (to.min(have) - self.base) as usize;
        let out = self.buf[lo..hi.max(lo)].to_vec();
        self.buf.drain(..lo);
        self.base += lo as u64;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(start: f64, end: f64) -> Chunk {
        Chunk { start, end }
    }

    #[test]
    fn groups_fill_whispers_window_per_track() {
        let work = [
            (0, c(0.0, 10.0)),
            (0, c(12.0, 22.0)),
            (0, c(25.0, 35.0)),
            (0, c(40.0, 45.0)),
            (0, c(300.0, 302.0)),
            (1, c(1.0, 3.0)),
        ];
        let g = groups(&work);
        let spans: Vec<(usize, usize)> = g.iter().map(|g| (g.track, g.chunks.len())).collect();
        // 30 s of audio fill the first group; the 2-minute reach splits off 300 s; tracks never mix.
        assert_eq!(spans, vec![(0, 3), (0, 1), (0, 1), (1, 1)]);
        assert_eq!(g[1].chunks[0], c(40.0, 45.0));
        // A chunk alone over half the window still gets its own group.
        assert_eq!(groups(&[(0, c(0.0, 28.0)), (0, c(29.0, 57.0))]).len(), 2);
        assert!(groups(&[]).is_empty());
        // Close in time and short, but on either side of 5:00: two pieces, two groups.
        assert_eq!(
            groups(&[(0, c(290.0, 299.0)), (0, c(300.5, 305.0))]).len(),
            2
        );
    }

    /// Samples numbered from 0 at 16 kHz, `len` in all; `fail` makes the end a decode failure.
    struct Counter {
        next: u64,
        len: u64,
        fail: bool,
    }

    impl Decode for Counter {
        fn read_into(&mut self, buf: &mut Vec<f32>, n: usize) -> Result<()> {
            let end = (self.next + n as u64).min(self.len);
            buf.clear();
            buf.extend((self.next..end).map(|x| x as f32));
            self.next = end;
            Ok(())
        }
        fn ended(&mut self) -> Result<()> {
            if self.fail {
                return Err(crate::Error::Render("decode failed".into()));
            }
            Ok(())
        }
    }

    fn counter(len: u64, fail: bool) -> Counter {
        Counter { next: 0, len, fail }
    }

    #[test]
    fn chunks_are_cut_from_one_forward_stream() {
        let rate = vad::RATE as u64;
        // The stream starts at 100 s in the recording.
        let go = AtomicBool::new(false);
        let mut s = Stream::new(counter(700 * rate, false), 100.0, &go);
        let a = s.take(100.5, 101.0).unwrap();
        assert_eq!(a.len(), rate as usize / 2);
        assert_eq!(a[0], (rate / 2) as f32);
        // Ten minutes later: the gap is read past, not kept.
        let b = s.take(700.0, 700.25).unwrap();
        assert_eq!(b[0], (600 * rate) as f32);
        assert!(s.buf.len() < 3 * rate as usize, "held {}", s.buf.len());
        // Past the end of the track: short, then empty.
        let tail = s.take(799.5, 801.0).unwrap();
        assert_eq!(tail.len(), rate as usize / 2);
        assert!(s.take(802.0, 803.0).unwrap().is_empty());
    }

    #[test]
    fn a_failed_decode_is_an_error() {
        let go = AtomicBool::new(false);
        let mut s = Stream::new(counter(16_000, true), 0.0, &go);
        assert!(s.take(0.0, 0.5).is_ok());
        assert!(s.take(0.5, 2.0).is_err());
    }

    #[test]
    fn a_stopped_stream_reads_no_further() {
        let stop = AtomicBool::new(false);
        let mut s = Stream::new(counter(u64::MAX, false), 0.0, &stop);
        assert!(s.take(0.0, 1.0).is_ok());
        stop.store(true, Ordering::Relaxed);
        // Ten hours ahead: refused at the first read, not decoded through.
        assert!(s.take(36_000.0, 36_001.0).is_err());
        assert!(s.src.next < 3 * vad::RATE as u64, "read {}", s.src.next);
    }
}
