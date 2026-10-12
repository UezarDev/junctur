//! Seconds to whole frames. FCPXML and the renderer both need a range to land on the frame grid the
//! same way (start down, end up), so a kept range means the same frames in Resolve and in the render.

use super::Range;

pub fn frames_ceil(s: f64, num: u64, den: u64) -> u64 {
    (s * num as f64 / den as f64 - 1e-6).ceil().max(0.0) as u64
}

pub fn frames_floor(s: f64, num: u64, den: u64) -> u64 {
    (s * num as f64 / den as f64 + 1e-6).floor().max(0.0) as u64
}

/// Kept ranges as `[start, end)` frames within `[0, total]`, merged where rounding made them touch.
pub fn frame_ranges(keep: &[Range], num: u64, den: u64, total: u64) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for r in keep {
        let a = frames_floor(r.start, num, den).min(total);
        let b = frames_ceil(r.end, num, den).min(total);
        if b <= a {
            continue;
        }
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Seconds for source frames `[start, end)` that survive the edit file's millisecond times and
/// name exactly those frames again through [`frame_ranges`] (starts floored, ends ceiled). `k/fps`
/// itself does not: 31/30 s writes as 1.033, which floors to frame 30. A kept span (`keep`) starts
/// at the first millisecond inside its first frame and ends at the last one before its end; a cut
/// span is the other way round, because the ranges either side of it take its ends.
pub fn frame_span(start: u64, end: u64, keep: bool, num: u64, den: u64) -> (f64, f64) {
    let secs = |f: u64| f as f64 * den as f64 / num as f64;
    let up = |f: u64| (secs(f) * 1000.0 - 1e-6).ceil() / 1000.0;
    let down = |f: u64| (secs(f) * 1000.0 + 1e-6).floor() / 1000.0;
    if keep {
        (up(start), down(end))
    } else {
        (down(start), up(end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Range {
        Range { start, end }
    }

    #[test]
    fn frames_round_outward_and_merge() {
        // 29.97 fps: 1.0 s is frame 29.97 -> 29, 2.0 s -> 60, and 2.01 s floors to 60, so the
        // first two touch and merge; the last starts past the end and is dropped.
        let f = frame_ranges(
            &[r(1.0, 2.0), r(2.01, 3.0), r(20.0, 30.0)],
            30000,
            1001,
            299,
        );
        assert_eq!(f, vec![(29, 90)]);
    }

    #[test]
    fn frame_spans_name_their_frames_after_millisecond_rounding() {
        for (num, den) in [(30, 1), (30000, 1001), (60, 1), (24000, 1001)] {
            for (a, b) in [(31, 97), (1, 2), (0, 5), (12345, 12346)] {
                let (s, e) = frame_span(a, b, true, num, den);
                let ms = |x: f64| format!("{x:.3}").parse::<f64>().unwrap();
                assert_eq!((ms(s), ms(e)), (s, e), "already on the millisecond grid");
                assert_eq!(frame_ranges(&[r(s, e)], num, den, 1 << 20), vec![(a, b)]);
                // A cut's ends are the ends of the kept ranges either side.
                let (cs, ce) = frame_span(a, b, false, num, den);
                let left = frame_ranges(&[r(0.0, cs)], num, den, 1 << 20);
                let right = frame_ranges(&[r(ce, 1e6)], num, den, 1 << 20);
                assert_eq!(left.last().map_or(0, |x| x.1), a);
                assert_eq!(right[0].0, b);
            }
        }
    }
}
