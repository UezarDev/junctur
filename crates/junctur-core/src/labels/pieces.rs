//! Where the labels go: the stretches where nobody talks, cut into pieces of one decision each, and
//! the moments each piece is looked at. From the speech stage's regions, not the edit list, so the
//! user's cuts never move a label.

use serde::{Deserialize, Serialize};

use crate::vad::Region;

/// The shortest silent stretch that gets a label: shorter ones are pauses between sentences.
pub const SHORTEST: f64 = 2.0;
/// The longest piece: a longer stretch is cut into equal pieces, so a teamfight after a minute of
/// walking is a piece of its own. 3b cuts speech at the same length, one decision each.
pub const LONGEST: f64 = 30.0;
/// One look every this many seconds of a piece (OBS's keyframe interval at its usual settings).
pub const EVERY: f64 = 2.0;
/// How far from a look a keyframe may be and still stand for it.
pub const NEAR: f64 = 1.0;

/// A stretch of a silent part, `start..end` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub start: f64,
    pub end: f64,
}

impl Piece {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    /// The moments looked at: one every [`EVERY`] seconds, centred in the piece, at least one.
    pub fn looks(&self) -> Vec<f64> {
        let n = ((self.len() / EVERY).floor() as usize).max(1);
        let step = self.len() / n as f64;
        (0..n)
            .map(|k| self.start + (k as f64 + 0.5) * step)
            .collect()
    }

    fn holds(&self, t: f64) -> bool {
        t >= self.start && t < self.end
    }
}

/// The silent stretches of a recording `duration` seconds long whose speech is `regions` (any tracks,
/// any order, overlapping): every gap of at least [`SHORTEST`] between them, the start and end of the
/// recording included. Whole, as the speech leaves them: smart keep keeps a stretch whole, and joining
/// its [`pieces`] back together missed where two shared ends were computed an ulp apart (3e).
pub fn gaps(regions: &[Region], duration: f64) -> Vec<(f64, f64)> {
    let mut spans: Vec<(f64, f64)> = regions.iter().map(|r| (r.start, r.end)).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut at = 0.0f64;
    for (a, b) in spans {
        out.push((at, a.min(duration)));
        at = at.max(b);
    }
    out.push((at, duration));
    out.retain(|(a, b)| b - a >= SHORTEST);
    out
}

/// The silent pieces: each of [`gaps`] cut into equal pieces of at most [`LONGEST`].
pub fn pieces(regions: &[Region], duration: f64) -> Vec<Piece> {
    let mut out = Vec::new();
    for (a, b) in gaps(regions, duration) {
        let n = ((b - a) / LONGEST).ceil().max(1.0) as usize;
        let step = (b - a) / n as f64;
        for k in 0..n {
            let start = a + k as f64 * step;
            let end = if k + 1 == n { b } else { start + step };
            out.push(Piece { start, end });
        }
    }
    out
}

/// For each piece, the keyframes that stand for its looks, and the looks no keyframe stands for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Per piece, indices into the keyframe times given to [`plan`].
    pub keyframes: Vec<Vec<usize>>,
    /// Per piece, the looks to decode by seeking.
    pub seek: Vec<Vec<f64>>,
}

/// Matches each look of `pieces` to the nearest keyframe of `times` (sorted) within [`NEAR`] inside
/// its piece, each keyframe at most once; looks with none are left to seek.
pub fn plan(pieces: &[Piece], times: &[f64]) -> Plan {
    let mut p = Plan::default();
    let mut used = vec![false; times.len()];
    for piece in pieces {
        let mut keys = Vec::new();
        let mut seek = Vec::new();
        for look in piece.looks() {
            let from = times.partition_point(|&t| t < look - NEAR);
            let best = (from..times.len())
                .take_while(|&i| times[i] <= look + NEAR)
                .filter(|&i| !used[i] && piece.holds(times[i]))
                .min_by(|&a, &b| (times[a] - look).abs().total_cmp(&(times[b] - look).abs()));
            match best {
                Some(i) => {
                    used[i] = true;
                    keys.push(i);
                }
                None => seek.push(look),
            }
        }
        p.keyframes.push(keys);
        p.seek.push(seek);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Region {
        Region { start, end }
    }

    #[test]
    fn gaps_of_two_seconds_or_more_between_any_tracks_speech() {
        // Two tracks, out of order and overlapping; a 1.5 s pause is no piece.
        let regions = [r(10.0, 20.0), r(3.0, 5.0), r(15.0, 22.0), r(23.5, 30.0)];
        let p = pieces(&regions, 40.0);
        assert_eq!(
            p,
            vec![
                Piece {
                    start: 0.0,
                    end: 3.0
                },
                Piece {
                    start: 5.0,
                    end: 10.0
                },
                Piece {
                    start: 30.0,
                    end: 40.0
                },
            ]
        );
    }

    #[test]
    fn a_long_stretch_is_cut_into_equal_pieces_of_at_most_thirty_seconds() {
        let p = pieces(&[r(0.0, 1.0)], 71.0);
        assert_eq!(p.len(), 3);
        assert!(p.iter().all(|x| (x.len() - 70.0 / 3.0).abs() < 1e-9));
        assert_eq!(p[2].end, 71.0);
        // Exactly thirty is one piece.
        assert_eq!(pieces(&[], 30.0).len(), 1);
    }

    #[test]
    fn no_speech_is_all_silent_and_no_length_is_nothing() {
        assert_eq!(
            pieces(&[], 5.0),
            vec![Piece {
                start: 0.0,
                end: 5.0
            }]
        );
        assert!(pieces(&[], 0.0).is_empty());
        assert!(pieces(&[r(0.0, 10.0)], 10.0).is_empty());
    }

    #[test]
    fn looks_every_two_seconds_centred_and_at_least_one() {
        let p = Piece {
            start: 10.0,
            end: 16.0,
        };
        assert_eq!(p.looks(), vec![11.0, 13.0, 15.0]);
        assert_eq!(
            Piece {
                start: 0.0,
                end: 2.5
            }
            .looks(),
            vec![1.25]
        );
    }

    #[test]
    fn keyframes_stand_for_near_looks_once_and_the_rest_are_sought() {
        let p = [
            Piece {
                start: 10.0,
                end: 16.0,
            },
            Piece {
                start: 16.0,
                end: 18.5,
            },
        ];
        // Keyframes every 2 s at the even seconds, and none near 15.
        let times = [8.0, 10.0, 12.0, 14.0, 20.0];
        let plan = plan(&p, &times);
        // Looks 11, 13, 15: 11 takes 10 (12 is as near, the first wins), 13 takes 12 or 14, 15 takes 14.
        assert_eq!(plan.keyframes[0], vec![1, 2, 3]);
        assert!(plan.seek[0].is_empty());
        // Look 17.25: 16..18.5 holds no keyframe (14 is outside the piece).
        assert!(plan.keyframes[1].is_empty());
        assert_eq!(plan.seek[1], vec![17.25]);
    }
}
