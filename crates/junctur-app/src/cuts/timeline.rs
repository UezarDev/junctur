//! An edit list on the frame grid (rounded as FCPXML and the renderer round it) and the mapping
//! between source and output time, by binary search since it runs on every position update.
//! Output seconds are summed from the EDL's rounded lengths as mpv sums them, or they drift from
//! mpv by a frame after thousands of ranges; output frames are exact and are what the clocks show.

use junctur_core::edit::{frame_ranges, frames_ceil, Range};
use serde::Serialize;

/// Frames per second as a fraction (30000/1001 and friends), as ffprobe reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Fps {
    pub num: u64,
    pub den: u64,
}

impl Fps {
    /// What FCPXML export assumes for a file without video.
    pub const AUDIO_ONLY: Fps = Fps { num: 30, den: 1 };

    pub fn seconds(self, frame: u64) -> f64 {
        frame as f64 * self.den as f64 / self.num as f64
    }

    /// The frame on screen at `s` seconds. The small bias absorbs float error at frame starts, as
    /// `frames_floor` does.
    pub fn frame_at(self, s: f64) -> u64 {
        (s * self.num as f64 / self.den as f64 + 1e-6)
            .floor()
            .max(0.0) as u64
    }
}

/// How far before its first frame an EDL segment starts: starting exactly on the frame lets float
/// rounding drop it, or let the frame after the range in. About 5 audio samples at 48 kHz.
pub const EDL_BIAS: f64 = 1e-4;

/// Decimals the EDL writes seconds with.
pub const EDL_DECIMALS: usize = 6;

/// `x` as mpv reads it back from the EDL.
fn edl_value(x: f64) -> f64 {
    format!("{x:.EDL_DECIMALS$}")
        .parse()
        .expect("a formatted float parses")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub fps: Fps,
    /// Kept `[start, end)` source frames, sorted and disjoint.
    pub ranges: Vec<(u64, u64)>,
    /// Source length in frames.
    pub total: u64,
    /// Output frames before each range.
    out_before: Vec<u64>,
    pub output_frames: u64,
    /// Each segment's source start and length, as the EDL spells them.
    segments: Vec<(f64, f64)>,
    /// Where each segment starts in mpv's timeline, and where the last one ends.
    out_start: Vec<f64>,
    out_end: f64,
}

impl Timeline {
    pub fn new(keep: &[Range], duration: f64, fps: Fps) -> Self {
        let total = frames_ceil(duration, fps.num, fps.den);
        let ranges = frame_ranges(keep, fps.num, fps.den, total);
        let n = ranges.len();
        let mut out_before = Vec::with_capacity(n);
        let mut segments = Vec::with_capacity(n);
        let mut out_start = Vec::with_capacity(n);
        let (mut frames, mut secs) = (0, 0.0);
        for &(a, b) in &ranges {
            out_before.push(frames);
            frames += b - a;
            let start = edl_value((fps.seconds(a) - EDL_BIAS).max(0.0));
            let length = edl_value(fps.seconds(b - a));
            segments.push((start, length));
            out_start.push(secs);
            secs += length;
        }
        Self {
            fps,
            ranges,
            total,
            out_before,
            output_frames: frames,
            segments,
            out_start,
            out_end: secs,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The EDL's segments: source start and length in seconds, already rounded as written.
    pub fn segments(&self) -> &[(f64, f64)] {
        &self.segments
    }

    pub fn source_duration(&self) -> f64 {
        self.fps.seconds(self.total)
    }

    /// Length of mpv's EDL timeline.
    pub fn output_duration(&self) -> f64 {
        self.out_end
    }

    /// Where each boundary between ranges sits in mpv's timeline (the start of ranges 1..).
    pub fn boundaries(&self) -> &[f64] {
        self.out_start.get(1..).unwrap_or_default()
    }

    /// Index of the first range that ends after source frame `f`.
    fn range_after(&self, f: u64) -> usize {
        self.ranges.partition_point(|&(_, b)| b <= f)
    }

    /// The segment playing at EDL time `o`.
    fn segment_at(&self, o: f64) -> usize {
        self.out_start
            .partition_point(|&x| x <= o)
            .saturating_sub(1)
    }

    /// Where the segment playing at EDL time `o` starts in the timeline.
    pub fn segment_start(&self, o: f64) -> f64 {
        self.out_start
            .get(self.segment_at(o))
            .copied()
            .unwrap_or(0.0)
    }

    /// EDL time for source time `s`, and whether `s` is inside a cut. A cut maps to where the next
    /// kept range starts (the end of the timeline past the last one).
    pub fn to_output(&self, s: f64) -> (f64, bool) {
        let f = self.fps.frame_at(s);
        let i = self.range_after(f);
        let Some(&(a, _)) = self.ranges.get(i) else {
            return (self.out_end, true);
        };
        let cut = f < a;
        let s = if cut { self.fps.seconds(a) } else { s };
        (self.out_start[i] + (s - self.fps.seconds(a)), cut)
    }

    /// Source time for EDL time `o`, within the kept frames of the segment playing (its first
    /// frame during the bias before it, its last frame past the timeline's end).
    pub fn to_source(&self, o: f64) -> f64 {
        if self.ranges.is_empty() {
            return 0.0;
        }
        let i = self.segment_at(o);
        let (a, b) = self.ranges[i];
        let s = self.fps.seconds(a) + (o - self.out_start[i]);
        let last = self.fps.seconds(b - 1) + self.fps.seconds(1) / 2.0;
        s.clamp(self.fps.seconds(a), last)
    }

    /// The output frame showing at source time `s` (in a cut, the next kept frame's).
    pub fn output_frame(&self, s: f64) -> u64 {
        let f = self.fps.frame_at(s);
        let i = self.range_after(f);
        match self.ranges.get(i) {
            None => self.output_frames,
            Some(&(a, _)) => self.out_before[i] + f.saturating_sub(a),
        }
    }

    /// `s` itself when it is kept, else the first frame of the next kept range; `None` past the
    /// last one.
    pub fn snap(&self, s: f64) -> Option<f64> {
        let f = self.fps.frame_at(s);
        let &(a, _) = self.ranges.get(self.range_after(f))?;
        Some(if f < a { self.fps.seconds(a) } else { s })
    }

    /// Where a seek to source time `s` lands while the kept ranges play: `(EDL time, source
    /// time)`. A cut lands on the next kept range, past the last one on the last kept frame.
    pub fn seek_target(&self, s: f64) -> (f64, f64) {
        let s = self.snap(s).unwrap_or_else(|| {
            let last = self.ranges.last().map_or(0, |r| r.1.saturating_sub(1));
            self.fps.seconds(last)
        });
        (self.to_output(s).0, s)
    }
}

#[cfg(test)]
mod tests;
