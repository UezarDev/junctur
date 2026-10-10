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
}
