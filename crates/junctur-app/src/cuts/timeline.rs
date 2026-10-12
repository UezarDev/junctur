//! An edit list on the frame grid: its kept ranges as whole frames, rounded exactly as FCPXML and
//! the renderer round them (`edit::frame_ranges`), and the mapping between source time and output
//! (edited) time. Every lookup is a binary search, since a 30 h recording has thousands of ranges
//! and the mapping runs on every position update.

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

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub fps: Fps,
    /// Kept `[start, end)` source frames, sorted and disjoint.
    pub ranges: Vec<(u64, u64)>,
    /// Source length in frames.
    pub total: u64,
    /// Output frames before each range (`out_before[i]` is where range `i` starts in the output).
    out_before: Vec<u64>,
    output_frames: u64,
}

impl Timeline {
    pub fn new(keep: &[Range], duration: f64, fps: Fps) -> Self {
        let total = frames_ceil(duration, fps.num, fps.den);
        let ranges = frame_ranges(keep, fps.num, fps.den, total);
        let mut out_before = Vec::with_capacity(ranges.len());
        let mut acc = 0;
        for (a, b) in &ranges {
            out_before.push(acc);
            acc += b - a;
        }
        Self {
            fps,
            ranges,
            total,
            out_before,
            output_frames: acc,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn source_duration(&self) -> f64 {
        self.fps.seconds(self.total)
    }

    pub fn output_duration(&self) -> f64 {
        self.fps.seconds(self.output_frames)
    }

    /// Index of the first range that ends after source frame `f`.
    fn range_after(&self, f: u64) -> usize {
        self.ranges.partition_point(|&(_, b)| b <= f)
    }

    /// Output time for source time `s`, and whether `s` is inside a cut. A cut maps to the output
    /// time where the next kept range starts (the end of the output past the last one).
    pub fn to_output(&self, s: f64) -> (f64, bool) {
        let f = self.fps.frame_at(s);
        let i = self.range_after(f);
        let Some(&(a, _)) = self.ranges.get(i) else {
            return (self.output_duration(), true);
        };
        if f < a {
            return (self.fps.seconds(self.out_before[i]), true);
        }
        // Same offset into the frame as in the source, so the clock moves smoothly.
        let shift = self.fps.seconds(a) - self.fps.seconds(self.out_before[i]);
        ((s - shift).max(0.0), false)
    }

    /// Source time for output time `o` (clamped to the last kept frame's end).
    pub fn to_source(&self, o: f64) -> f64 {
        if self.ranges.is_empty() {
            return 0.0;
        }
        let of = self.fps.frame_at(o);
        let i = self
            .out_before
            .partition_point(|&x| x <= of)
            .saturating_sub(1);
        let (a, b) = self.ranges[i];
        let shift = self.fps.seconds(a) - self.fps.seconds(self.out_before[i]);
        (o + shift).min(self.fps.seconds(b))
    }

    /// `s` itself when it is kept, else the first frame of the next kept range; `None` past the
    /// last one.
    pub fn snap(&self, s: f64) -> Option<f64> {
        let f = self.fps.frame_at(s);
        let &(a, _) = self.ranges.get(self.range_after(f))?;
        Some(if f < a { self.fps.seconds(a) } else { s })
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

    #[test]
    fn frames_match_frame_ranges() {
        let t = sample();
        assert_eq!(t.ranges, vec![(30, 60), (150, 210)]);
        assert_eq!(t.total, 300);
        assert!((t.output_duration() - 3.0).abs() < 1e-9);
        assert!((t.source_duration() - 10.0).abs() < 1e-9);
        // Same rounding as the render: a range off the frame grid rounds outward.
        let t = Timeline::new(&[r(1.01, 1.99)], 10.0, FPS);
        assert_eq!(t.ranges, vec![(30, 60)]);
    }

    #[test]
    fn source_to_output() {
        let t = sample();
        let close = |(a, cut): (f64, bool), b: f64, c: bool| (a - b).abs() < 1e-9 && cut == c;
        assert!(close(t.to_output(1.5), 0.5, false));
        assert!(close(t.to_output(5.0), 1.0, false));
        assert!(close(t.to_output(6.5), 2.5, false));
        // Before the first range, between ranges, and after the last.
        assert!(close(t.to_output(0.2), 0.0, true));
        assert!(close(t.to_output(3.0), 1.0, true));
        assert!(close(t.to_output(2.0), 1.0, true), "a range's end is cut");
        assert!(close(t.to_output(9.0), 3.0, true));
    }

    #[test]
    fn output_to_source() {
        let t = sample();
        assert!((t.to_source(0.0) - 1.0).abs() < 1e-9);
        assert!((t.to_source(0.5) - 1.5).abs() < 1e-9);
        assert!((t.to_source(1.0) - 5.0).abs() < 1e-9);
        assert!((t.to_source(2.9) - 6.9).abs() < 1e-9);
        assert!((t.to_source(99.0) - 7.0).abs() < 1e-9, "clamped to the end");
        // Round trip over every kept frame.
        for f in (30..60).chain(150..210) {
            let s = FPS.seconds(f);
            let (o, cut) = t.to_output(s);
            assert!(!cut);
            assert_eq!(FPS.frame_at(t.to_source(o)), f);
        }
    }

    #[test]
    fn snaps_into_kept_material() {
        let t = sample();
        assert_eq!(t.snap(1.5), Some(1.5));
        assert_eq!(t.snap(0.0), Some(1.0));
        assert_eq!(t.snap(3.3), Some(5.0));
        assert_eq!(t.snap(2.0), Some(5.0));
        assert_eq!(t.snap(7.0), None);
    }

    #[test]
    fn empty_keeps_nothing() {
        let t = Timeline::new(&[], 10.0, FPS);
        assert!(t.is_empty());
        assert_eq!(t.to_output(4.0), (0.0, true));
        assert_eq!(t.to_source(1.0), 0.0);
        assert_eq!(t.snap(1.0), None);
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

    #[test]
    fn thousands_of_ranges_stay_consistent() {
        // 30 h at 30 fps, 10 000 ranges of 3 s every 10.8 s.
        let keep: Vec<Range> = (0..10_000)
            .map(|i| r(i as f64 * 10.8, i as f64 * 10.8 + 3.0))
            .collect();
        let t = Timeline::new(&keep, 30.0 * 3600.0, FPS);
        assert_eq!(t.ranges.len(), 10_000);
        let s = 9_999.0 * 10.8 + 1.0;
        let (o, cut) = t.to_output(s);
        assert!(!cut);
        assert!((o - (9_999.0 * 3.0 + 1.0)).abs() < 1e-6);
        assert!((t.to_source(o) - s).abs() < 1e-6);
    }
}
