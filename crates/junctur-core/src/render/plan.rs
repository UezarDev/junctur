//! Which frames get re-encoded and which get copied. Pure, so the cut arithmetic is tested without
//! a video.
//!
//! A kept range `[a, b)` is copied between the first and last usable keyframes inside it; only the
//! frames from `a` up to the first keyframe and from the last keyframe up to `b` are encoded.

/// A run of source frames `[start, end)`; `at` is its first frame's place in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub start: u64,
    pub end: u64,
    pub at: u64,
    pub copy: bool,
}

impl Piece {
    pub fn frames(&self) -> u64 {
        self.end - self.start
    }
}

/// Pieces for the frame ranges, in output order. `keyframes` sorted; empty = encode everything.
pub fn pieces(ranges: &[(u64, u64)], keyframes: &[u64]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut push = |start: u64, end: u64, copy: bool| {
        if end > start {
            out.push(Piece {
                start,
                end,
                at,
                copy,
            });
            at += end - start;
        }
    };
    for &(a, b) in ranges {
        let lo = keyframes.partition_point(|&k| k < a);
        let hi = keyframes.partition_point(|&k| k <= b);
        match keyframes[lo..hi] {
            [first, .., last] => {
                push(a, first, false);
                push(first, last, true);
                push(last, b, false);
            }
            _ => push(a, b, false),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(start: u64, end: u64, at: u64, copy: bool) -> Piece {
        Piece {
            start,
            end,
            at,
            copy,
        }
    }

    #[test]
    fn copies_between_keyframes_and_encodes_the_edges() {
        let kf = [0, 60, 120, 180, 240];
        assert_eq!(
            pieces(&[(100, 200)], &kf),
            vec![
                p(100, 120, 0, false),
                p(120, 180, 20, true),
                p(180, 200, 80, false)
            ]
        );
    }

    #[test]
    fn keyframe_on_a_boundary_needs_no_encode() {
        let kf = [0, 60, 120];
        assert_eq!(pieces(&[(0, 120)], &kf), vec![p(0, 120, 0, true)]);
    }

    #[test]
    fn fewer_than_two_keyframes_inside_encodes_the_range() {
        let kf = [0, 250, 500];
        assert_eq!(
            pieces(&[(10, 240), (260, 400)], &kf),
            vec![p(10, 240, 0, false), p(260, 400, 230, false)]
        );
        // One keyframe inside: nothing to copy between, the whole range is encoded.
        assert_eq!(pieces(&[(200, 300)], &kf), vec![p(200, 300, 0, false)]);
    }

    #[test]
    fn no_keyframes_is_a_full_encode() {
        assert_eq!(
            pieces(&[(0, 10), (20, 25)], &[]),
            vec![p(0, 10, 0, false), p(20, 25, 10, false)]
        );
    }
}
