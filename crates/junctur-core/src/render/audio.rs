//! Audio tracks are re-encoded whole, not copied: AAC packets (21.3 ms at 48 kHz) never line up
//! with 33.3 ms video frames, every copied join would carry encoder priming, and the crossfades
//! need new samples anyway. Each track is decoded once, front to back, spliced sample-exactly in
//! [`splice`](super::splice), and encoded once, so priming exists only at the start of the file,
//! where the MP4 edit list hides it.

use std::path::Path;

use super::source::AudioTrack;
use super::splice::{splice, Samples};
use crate::ffmpeg::{PcmReader, PcmWriter, Span};
use crate::{Error, Result};

/// AAC bitrate when the source does not say, and the floor (a near-silent track reports ~2 kb/s).
const DEFAULT_BITRATE: u64 = 160_000;
const MIN_BITRATE: u64 = 64_000;

/// A forward-only reader that keeps a little history, because a crossfade reads a few ms before
/// the point it last read up to.
struct Cursor {
    reader: PcmReader,
    channels: usize,
    buf: Vec<f32>,
    /// Sample frame number of `buf[0]`.
    base: u64,
    eof: bool,
    keep: u64,
}

impl Samples for Cursor {
    fn read(&mut self, from: u64, to: u64) -> Result<Vec<f32>> {
        let ch = self.channels;
        while !self.eof && self.base + ((self.buf.len() / ch) as u64) < to {
            let got = self.reader.read(ch * 48_000)?;
            self.eof = got.is_empty();
            self.buf.extend_from_slice(&got);
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
        let drop = to
            .saturating_sub(self.keep)
            .saturating_sub(self.base)
            .min(have);
        self.buf.drain(..drop as usize * ch);
        self.base += drop;
        Ok(out)
    }
}

/// Render audio track `index` of `src` as the sample-frame segments `segs`, crossfading `fade`
/// samples each side of every join, into the AAC file `out`.
pub fn render_track(
    src: &Path,
    index: usize,
    track: &AudioTrack,
    segs: &[(u64, u64)],
    fade: u64,
    out: &Path,
) -> Result<()> {
    let ch = track.channels.max(1);
    let reader = PcmReader::open(src, index, track.rate, ch, Span::default())?;
    let mut cursor = Cursor {
        reader,
        channels: ch as usize,
        buf: Vec::new(),
        base: 0,
        eof: false,
        keep: 2 * fade + track.rate as u64,
    };
    let bitrate = track
        .bit_rate
        .unwrap_or(DEFAULT_BITRATE)
        .max(MIN_BITRATE)
        .to_string();
    let opts = ["-c:a", "aac", "-b:a", &bitrate].map(String::from);
    let mut writer = PcmWriter::create_with(out, track.rate, ch, &opts)?;
    splice(segs, fade, ch as usize, &mut cursor, &mut |s| {
        writer.write(s)
    })?;
    writer.finish()?;
    // A decoder that died early would otherwise pass as silence (a normal track ends a few ms short).
    let read_to = cursor.base + (cursor.buf.len() / ch as usize) as u64;
    let last = segs.last().map_or(0, |s| s.1);
    if cursor.eof && read_to + u64::from(track.rate) < last {
        return Err(Error::Render(format!(
            "audio track {} ended at {:.1} s, before the last kept range",
            index + 1,
            read_to as f64 / f64::from(track.rate)
        )));
    }
    // Not `finish`: the decoder is still running past the last kept range; dropping stops it.
    Ok(())
}
