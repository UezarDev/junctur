//! Joining kept audio segments with a short crossfade that does not change the length.
//!
//! Each join is an equal-power crossfade of `2h` samples centred on the cut: the outgoing segment
//! runs `h` samples past its end and the incoming one starts `h` samples early, overlapped. The
//! output is exactly the sum of the segment lengths, so audio stays frame-locked to the video.

use crate::Result;

/// Interleaved samples `[from, to)` (sample frames, not values) of one source track; zeros past
/// its end.
pub trait Samples {
    fn read(&mut self, from: u64, to: u64) -> Result<Vec<f32>>;
}

/// Samples per write while copying the middle of a segment.
const CHUNK: u64 = 48_000;

/// Write `segs` (sample frames `[start, end)`) end to end, crossfading `fade` samples on each
/// side of every join (less where a segment is too short to give them).
pub fn splice(
    segs: &[(u64, u64)],
    fade: u64,
    channels: usize,
    src: &mut dyn Samples,
    write: &mut dyn FnMut(&[f32]) -> Result<()>,
) -> Result<()> {
    let joins: Vec<u64> = segs
        .windows(2)
        .map(|w| fade.min((w[0].1 - w[0].0) / 2).min((w[1].1 - w[1].0) / 2))
        .collect();
    // The previous segment's last `h` samples (not yet written) and the `h` that follow it.
    let mut tail = Vec::new();
    let mut after = Vec::new();
    for (i, &(s, e)) in segs.iter().enumerate() {
        let hin = if i > 0 { joins[i - 1] } else { 0 };
        let hout = joins.get(i).copied().unwrap_or(0);
        if hin > 0 {
            let before = src.read(s - hin, s)?;
            let head = src.read(s, s + hin)?;
            write(&blend(&tail, &before, channels, 0, hin))?;
            write(&blend(&after, &head, channels, hin, hin))?;
        }
        let mut p = s + hin;
        while p < e - hout {
            let q = (p + CHUNK).min(e - hout);
            write(&src.read(p, q)?)?;
            p = q;
        }
        if hout > 0 {
            tail = src.read(e - hout, e)?;
            after = src.read(e, e + hout)?;
        }
    }
    Ok(())
}

/// Half of a `2h`-sample crossfade: `out` fades from `a` to `b`, `offset` samples into the window.
fn blend(a: &[f32], b: &[f32], channels: usize, offset: u64, h: u64) -> Vec<f32> {
    let width = 2.0 * h as f32;
    a.iter()
        .zip(b)
        .enumerate()
        .map(|(i, (&x, &y))| {
            let t = ((i / channels) as f32 + offset as f32 + 0.5) / width;
            let (sin, cos) = (t * std::f32::consts::FRAC_PI_2).sin_cos();
            x * cos + y * sin
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mono source whose sample `n` is `n`.
    struct Ramp;
    impl Samples for Ramp {
        fn read(&mut self, from: u64, to: u64) -> Result<Vec<f32>> {
            Ok((from..to).map(|n| n as f32).collect())
        }
    }

    fn run(segs: &[(u64, u64)], fade: u64) -> Vec<f32> {
        let mut out = Vec::new();
        splice(segs, fade, 1, &mut Ramp, &mut |s| {
            out.extend_from_slice(s);
            Ok(())
        })
        .unwrap();
        out
    }

    #[test]
    fn length_is_the_sum_of_segments() {
        let out = run(&[(0, 100_000), (150_000, 150_300), (200_000, 200_001)], 720);
        assert_eq!(out.len(), 100_000 + 300 + 1);
    }

    #[test]
    fn untouched_away_from_joins() {
        let out = run(&[(10, 100), (500, 600)], 4);
        assert_eq!(out[0], 10.0);
        assert_eq!(out[85], 95.0, "before the fade");
        assert_eq!(out[90 + 4], 504.0, "after the fade");
        assert_eq!(*out.last().unwrap(), 599.0);
    }

    #[test]
    fn crossfade_moves_from_outgoing_to_incoming() {
        let out = run(&[(0, 100), (1000, 1100)], 4);
        // Around the join (output 96..104) the output moves from ~96..103 (outgoing, continued
        // past its end) to ~996..1003 (incoming, started early), monotonically.
        let w = &out[96..104];
        assert!(w[0] < 300.0 && w[7] > 900.0, "{w:?}");
        assert!(w.windows(2).all(|p| p[1] > p[0]), "{w:?}");
    }

    #[test]
    fn short_segments_shrink_the_fade_and_none_is_no_fade() {
        assert_eq!(run(&[(0, 4), (10, 14)], 100).len(), 8);
        assert_eq!(
            run(&[(0, 4), (10, 14)], 0),
            vec![0., 1., 2., 3., 10., 11., 12., 13.]
        );
    }
}
