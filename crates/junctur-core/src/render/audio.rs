//! Audio tracks are re-encoded whole, not copied: AAC packets (21.3 ms at 48 kHz) never line up
//! with 33.3 ms video frames, every copied join would carry encoder priming, and the crossfades
//! need new samples anyway. Each track is decoded once, front to back, spliced sample-exactly in
//! [`splice`](super::splice), and encoded once, so priming exists only at the start of the file,
//! where the MP4 edit list hides it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::source::AudioTrack;
use super::splice::{splice, Samples};
use crate::ffmpeg::{PcmReader, PcmWriter, Span};
use crate::{Error, Result};

/// AAC bitrate when the source does not say, and the floor (a near-silent track reports ~2 kb/s).
const DEFAULT_BITRATE: u64 = 160_000;
const MIN_BITRATE: u64 = 64_000;

/// Interleaved samples per decode.
const BLOCK: usize = 48_000;

/// Where a [`Cursor`] gets its samples: the decoder, or a stand-in in tests.
trait Decode {
    /// Up to `n` interleaved samples, empty at the end.
    fn read(&mut self, n: usize) -> Result<Vec<f32>>;
}

impl Decode for PcmReader {
    fn read(&mut self, n: usize) -> Result<Vec<f32>> {
        PcmReader::read(self, n)
    }
}

/// A forward-only reader that keeps a little history, because a crossfade reads a few ms before
/// the point it last read up to.
struct Cursor<D> {
    reader: D,
    channels: usize,
    buf: Vec<f32>,
    /// Sample frame number of `buf[0]`.
    base: u64,
    eof: bool,
    keep: u64,
    /// The most samples ever buffered, for the memory test.
    #[cfg(test)]
    peak: usize,
}

impl<D> Cursor<D> {
    fn new(reader: D, channels: usize, keep: u64) -> Self {
        Self {
            reader,
            channels,
            buf: Vec::new(),
            base: 0,
            eof: false,
            keep,
            #[cfg(test)]
            peak: 0,
        }
    }

    /// Forget samples before frame `upto`.
    fn forget(&mut self, upto: u64) {
        let have = (self.buf.len() / self.channels) as u64;
        let drop = upto.saturating_sub(self.base).min(have);
        self.buf.drain(..drop as usize * self.channels);
        self.base += drop;
    }
}

impl<D: Decode> Samples for Cursor<D> {
    fn read(&mut self, from: u64, to: u64) -> Result<Vec<f32>> {
        let ch = self.channels;
        // Nothing before `from` is read now, nor before `to - keep` later, so it goes while the
        // buffer fills: the first read after a cut decodes the whole cut gap (2 h of stereo is
        // 2.8 GB), and holding it was the 1d memory bug.
        let dead = from.min(to.saturating_sub(self.keep));
        while !self.eof && self.base + ((self.buf.len() / ch) as u64) < to {
            self.forget(dead);
            let got = self.reader.read(ch * BLOCK)?;
            self.eof = got.is_empty();
            self.buf.extend_from_slice(&got);
            #[cfg(test)]
            {
                self.peak = self.peak.max(self.buf.len());
            }
        }
        assert!(
            from >= self.base,
            "read {from} behind the kept history at {}",
            self.base
        );
        let mut out = vec![0.0; (to - from) as usize * ch];
        let have = self.buf.len() as u64 / ch as u64;
        let (lo, hi) = (from - self.base, (to - self.base).min(have));
        if hi > lo {
            out[..((hi - lo) as usize * ch)]
                .copy_from_slice(&self.buf[lo as usize * ch..hi as usize * ch]);
        }
        self.forget(to.saturating_sub(self.keep));
        Ok(out)
    }
}

/// Render audio track `index` of `src` as the sample-frame segments `segs`, crossfading `fade`
/// samples each side of every join, into the AAC file `out`. Gives up (with an error) once `stop`
/// is set, e.g. because a video piece failed.
pub fn render_track(
    src: &Path,
    index: usize,
    track: &AudioTrack,
    segs: &[(u64, u64)],
    fade: u64,
    out: &Path,
    stop: &AtomicBool,
) -> Result<()> {
    let ch = track.channels.max(1);
    let reader = PcmReader::open(src, index, track.rate, ch, Span::default())?;
    let mut cursor = Cursor::new(reader, ch as usize, 2 * fade + track.rate as u64);
    let bitrate = track
        .bit_rate
        .unwrap_or(DEFAULT_BITRATE)
        .max(MIN_BITRATE)
        .to_string();
    let opts = ["-c:a", "aac", "-b:a", &bitrate].map(String::from);
    let mut writer = PcmWriter::create_with(out, track.rate, ch, &opts)?;
    splice(segs, fade, ch as usize, &mut cursor, &mut |s| {
        if stop.load(Ordering::Relaxed) {
            return Err(Error::Render("stopped".into()));
        }
        writer.write(s)
    })?;
    writer.finish()?;
    // A decoder that died early would otherwise pass as silence. A normal track ends a few ms
    // short of the video; the margin is a second because no real recording has measured how short
    // one can end, and failing a long render on a healthy file costs more than a second of silence.
    let read_to = cursor.base + (cursor.buf.len() / ch as usize) as u64;
    let last = segs.last().map_or(0, |s| s.1);
    if cursor.eof && read_to + u64::from(track.rate) < last {
        // The decoder has exited; its own message says why it stopped.
        let why = match cursor.reader.finish() {
            Err(e) => format!(" ({e})"),
            Ok(()) => String::new(),
        };
        return Err(Error::Render(format!(
            "audio track {} ended at {:.1} s, before the last kept range{why}",
            index + 1,
            read_to as f64 / f64::from(track.rate)
        )));
    }
    // Not `finish`: the decoder is still running past the last kept range; dropping stops it.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mono samples numbered from 0, `len` in all, counting the most the cursor ever held.
    struct Counter {
        next: u64,
        len: u64,
    }

    impl Decode for Counter {
        fn read(&mut self, n: usize) -> Result<Vec<f32>> {
            let end = (self.next + n as u64).min(self.len);
            let out = (self.next..end).map(|x| x as f32).collect();
            self.next = end;
            Ok(out)
        }
    }

    #[test]
    fn a_long_cut_gap_is_not_held_in_memory() {
        // Two kept seconds either side of a 10-minute cut, at 48 kHz mono.
        let rate = 48_000;
        let segs = [(0, 2 * rate), (602 * rate, 604 * rate)];
        let reader = Counter {
            next: 0,
            len: 700 * rate,
        };
        let mut c = Cursor::new(reader, 1, 2 * 720 + rate);
        let mut out = Vec::new();
        for &(a, b) in &segs {
            for p in (a..b).step_by(rate as usize) {
                out.extend(c.read(p, (p + rate).min(b)).unwrap());
            }
        }
        assert_eq!(out.len() as u64, 4 * rate);
        assert_eq!(out[2 * rate as usize], (602 * rate) as f32);
        // One read, the kept history and a decode block, not the 600 s between.
        assert!(c.peak <= 4 * rate as usize, "held {} samples", c.peak);
    }
}
