//! Which frames get re-encoded and which get copied. Pure, so the cut arithmetic is tested without
//! a video.
//!
//! A kept range `[a, b)` is copied between the first and last usable keyframes inside it; only the
//! frames from `a` up to the first keyframe and from the last keyframe up to `b` are encoded. Every
//! piece costs an ffmpeg start (~0.3 s to open a 5-track MP4, plus NVENC's), which is what a render
//! of many short ranges spends its time on, so a copy shorter than it is worth is encoded instead,
//! and encode pieces close together in the source share one ffmpeg ([`batches`]).

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

/// Pieces for the frame ranges, in output order. `keyframes` sorted; empty = encode everything. A
/// range is copied between its keyframes only when that copy is longer than `min_copy` frames;
/// otherwise the whole range is one encode (copying it would cost a copy piece and an encode on
/// each side, three processes, for a few frames).
pub fn pieces(ranges: &[(u64, u64)], keyframes: &[u64], min_copy: u64) -> Vec<Piece> {
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
            [first, .., last] if last - first > min_copy => {
                push(a, first, false);
                push(first, last, true);
                push(last, b, false);
            }
            _ => push(a, b, false),
        }
    }
    out
}

/// The usual distance between usable keyframes, in frames (the median gap; 0 without two).
pub fn gop(keyframes: &[u64]) -> u64 {
    let mut gaps: Vec<u64> = keyframes.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_unstable();
    gaps.get(gaps.len() / 2).copied().unwrap_or(0)
}

/// Pieces written by one ffmpeg each: a copy piece alone, and runs of encode pieces next to each
/// other in the output whose source frames are at most `max_gap` apart (the encoder decodes
/// through the gap and drops it, cheaper than starting another ffmpeg) and span at most
/// `max_span` frames from first to last. Ranges of piece indices, in order.
pub fn batches(pieces: &[Piece], max_gap: u64, max_span: u64) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, p) in pieces.iter().enumerate() {
        if let Some(r) = out.last_mut() {
            let (first, prev) = (&pieces[r.start], &pieces[i - 1]);
            let joins = !p.copy
                && !prev.copy
                && p.start >= prev.end
                && p.start - prev.end <= max_gap
                && p.end - first.start <= max_span;
            if joins {
                r.end = i + 1;
                continue;
            }
        }
        out.push(i..i + 1);
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
            pieces(&[(100, 200)], &kf, 0),
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
        assert_eq!(pieces(&[(0, 120)], &kf, 0), vec![p(0, 120, 0, true)]);
    }

    #[test]
    fn fewer_than_two_keyframes_inside_encodes_the_range() {
        let kf = [0, 250, 500];
        assert_eq!(
            pieces(&[(10, 240), (260, 400)], &kf, 0),
            vec![p(10, 240, 0, false), p(260, 400, 230, false)]
        );
        // One keyframe inside: nothing to copy between, the whole range is encoded.
        assert_eq!(pieces(&[(200, 300)], &kf, 0), vec![p(200, 300, 0, false)]);
    }

    #[test]
    fn no_keyframes_is_a_full_encode() {
        assert_eq!(
            pieces(&[(0, 10), (20, 25)], &[], 0),
            vec![p(0, 10, 0, false), p(20, 25, 10, false)]
        );
    }

    #[test]
    fn a_copy_of_one_gop_or_less_is_encoded_whole() {
        let kf = [0, 60, 120, 180, 240];
        assert_eq!(gop(&kf), 60);
        assert_eq!(gop(&[0, 60, 61, 200]), 60, "the median, not a scene cut");
        assert_eq!(gop(&[5]), 0);
        // 60 copied frames between two keyframes: one GOP, not worth three processes.
        assert_eq!(pieces(&[(50, 130)], &kf, 60), vec![p(50, 130, 0, false)]);
        // Two GOPs are copied.
        assert_eq!(
            pieces(&[(50, 190)], &kf, 60),
            vec![
                p(50, 60, 0, false),
                p(60, 180, 10, true),
                p(180, 190, 130, false)
            ]
        );
    }

    #[test]
    fn close_encode_pieces_share_an_ffmpeg() {
        let ps = [
            p(0, 10, 0, false),
            p(10, 70, 10, true),
            p(70, 80, 70, false),
            // 20 frames of cut, then a whole short range, then a far one.
            p(100, 130, 80, false),
            p(140, 150, 110, false),
            p(900, 910, 120, false),
        ];
        assert_eq!(batches(&ps, 30, 1000), vec![0..1, 1..2, 2..5, 5..6]);
        assert_eq!(batches(&ps, 15, 1000), vec![0..1, 1..2, 2..3, 3..5, 5..6]);
        // The span limit bounds what one ffmpeg decodes.
        assert_eq!(batches(&ps, 30, 60), vec![0..1, 1..2, 2..4, 4..5, 5..6]);
        // Backwards in the source (a hand-edited list may reorder): never joined.
        let back = [p(100, 110, 0, false), p(50, 60, 10, false)];
        assert_eq!(batches(&back, 1000, 1000), vec![0..1, 1..2]);
        assert!(batches(&[], 30, 60).is_empty());
    }
}
