//! An edit list on the frame grid: its kept ranges as whole frames, rounded exactly as FCPXML and
//! the renderer round them (`edit::frame_ranges`), and the mapping between source time and output
//! (edited) time. Every lookup is a binary search, since a 30 h recording has thousands of ranges
//! and the mapping runs on every position update.
//!
//! Output *seconds* are mpv's EDL timeline (see `edl`): each segment as the EDL spells it, its
//! start and length rounded to the EDL's decimals, and segment starts summed from those lengths in
//! order, as mpv sums them. Computing them from exact frame times instead drifts from mpv by the
//! rounding of every earlier length (milliseconds after thousands of ranges, enough to name the
//! wrong frame). Within a segment, mpv shows frame `a + k` at `segment start + k/fps + β`, where
//! β is 0 to ~0.1 ms depending on how the source's time base rounds the EDL's biased start (0 on
//! OBS's 1/30 files, measured; ~0.13 ms on a 1/15360 one), so offsets count from the range's
//! first frame and any β in that band names the same frame. Output *frames* are exact integers,
//! and they are what the clocks show.

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

/// How far before its first frame an EDL segment starts. mpv keeps frames with `start <= pts <
/// start + length`; starting exactly on the frame lets float rounding drop it (or let the frame
/// after the range in), so every boundary sits a hair before its frame. Far below a frame, and
/// ~5 audio samples at 48 kHz.
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
mod tests {
    use super::*;

    const FPS: Fps = Fps { num: 30, den: 1 };

    fn r(start: f64, end: f64) -> Range {
        Range { start, end }
    }

    /// Kept 1-2 s and 5-7 s of a 10 s source: frames [30, 60) and [150, 210).
    fn sample() -> Timeline {
        Timeline::new(&[r(1.0, 2.0), r(5.0, 7.0)], 10.0, FPS)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn frames_match_frame_ranges() {
        let t = sample();
        assert_eq!(t.ranges, vec![(30, 60), (150, 210)]);
        assert_eq!((t.total, t.output_frames), (300, 90));
        assert!(close(t.output_duration(), 3.0));
        assert!(close(t.source_duration(), 10.0));
        assert_eq!(t.boundaries().len(), 1);
        // Same rounding as the render: a range off the frame grid rounds outward.
        let t = Timeline::new(&[r(1.01, 1.99)], 10.0, FPS);
        assert_eq!(t.ranges, vec![(30, 60)]);
    }

    #[test]
    fn source_to_output() {
        let t = sample();
        let at = |s: f64| t.to_output(s);
        assert!(close(at(1.5).0, 0.5) && !at(1.5).1);
        assert!(close(at(5.0).0, 1.0) && !at(5.0).1);
        assert!(close(at(6.5).0, 2.5));
        // Before the first range, between ranges, and after the last.
        assert!(close(at(0.2).0, 0.0) && at(0.2).1);
        assert!(close(at(3.0).0, 1.0) && at(3.0).1);
        assert!(at(2.0).1, "a range's end is cut");
        assert!(close(at(9.0).0, 3.0) && at(9.0).1);
        assert_eq!(t.output_frame(5.0), 30);
        assert_eq!(t.output_frame(3.0), 30, "a cut shows the next kept frame");
        assert_eq!(t.output_frame(9.0), 90);
    }

    #[test]
    fn output_to_source_stays_on_kept_frames() {
        let t = sample();
        assert!(close(t.to_source(0.5), 1.5));
        assert!(close(t.to_source(1.0), 5.0));
        assert!(close(t.to_source(2.9), 6.9));
        assert_eq!(
            FPS.frame_at(t.to_source(99.0)),
            209,
            "past the end: the last kept frame"
        );
        // In mpv's timeline a segment starts a bias before its first frame: until that frame
        // shows, the previous segment's last frame is on screen, then the new first frame.
        assert_eq!(FPS.frame_at(t.to_source(1.0 - EDL_BIAS / 2.0)), 59);
        assert_eq!(FPS.frame_at(t.to_source(1.0 + EDL_BIAS / 2.0)), 150);
        for f in (30..60).chain(150..210) {
            let (o, cut) = t.to_output(FPS.seconds(f));
            assert!(!cut);
            assert_eq!(FPS.frame_at(t.to_source(o)), f);
        }
    }

    #[test]
    fn seeks_land_on_kept_material() {
        let t = sample();
        assert_eq!(t.snap(1.5), Some(1.5));
        assert_eq!(t.snap(3.3), Some(5.0));
        assert_eq!(t.snap(7.0), None);
        let (o, s) = t.seek_target(3.3);
        assert_eq!(s, 5.0);
        assert_eq!(FPS.frame_at(t.to_source(o)), 150);
        // Past the last range: its last frame.
        let (o, s) = t.seek_target(9.0);
        assert_eq!(FPS.frame_at(s), 209);
        assert_eq!(FPS.frame_at(t.to_source(o)), 209);
    }

    #[test]
    fn empty_keeps_nothing() {
        let t = Timeline::new(&[], 10.0, FPS);
        assert!(t.is_empty());
        assert_eq!(t.to_output(4.0), (0.0, true));
        assert_eq!(t.to_source(1.0), 0.0);
        assert_eq!(t.snap(1.0), None);
        assert!(t.boundaries().is_empty());
    }

    #[test]
    fn ntsc_rate_frames() {
        let fps = Fps {
            num: 30000,
            den: 1001,
        };
        let t = Timeline::new(&[r(1.0, 2.0), r(2.01, 3.0)], 10.0, fps);
        // As frame_ranges' own test: the two touch after rounding and merge.
        assert_eq!(t.ranges, vec![(29, 90)]);
        assert_eq!(fps.frame_at(fps.seconds(1234)), 1234);
    }

    /// What mpv does with the EDL: segment `i` starts in its timeline at the sum of the rounded
    /// lengths before it, and frame `a + k` shows at `start_i + k/fps + β` with β from 0 to the
    /// bias. Over 10 000 ranges of 10 frames at 30000/1001 (every length rounded), every range's
    /// first and last frame must map back to the same source frame and the exact output frame.
    #[test]
    fn agrees_with_mpv_timeline_over_ten_thousand_ranges() {
        let fps = Fps {
            num: 30000,
            den: 1001,
        };
        let keep: Vec<Range> = (0..10_000)
            .map(|i| {
                let a = fps.seconds(i * 25);
                r(a, a + fps.seconds(10))
            })
            .collect();
        let t = Timeline::new(&keep, fps.seconds(25 * 10_000 + 10), fps);
        assert_eq!(t.ranges.len(), 10_000);
        let mut mpv_start = 0.0;
        for (i, (&(a, b), &(_, len))) in t.ranges.iter().zip(t.segments()).enumerate() {
            for f in [a, b - 1] {
                for beta in [0.0, EDL_BIAS / 2.0, EDL_BIAS] {
                    let shown = mpv_start + fps.seconds(f - a) + beta;
                    assert_eq!(fps.frame_at(t.to_source(shown)), f, "range {i}");
                    assert_eq!(t.output_frame(t.to_source(shown)), t.out_before[i] + f - a);
                }
            }
            // A seek to the range's start targets no later than its first frame shows.
            let (o, _) = t.seek_target(fps.seconds(a));
            assert!(o <= mpv_start + 1e-9, "range {i}");
            mpv_start += len;
        }
        assert_eq!(t.output_duration(), mpv_start);
    }
}
