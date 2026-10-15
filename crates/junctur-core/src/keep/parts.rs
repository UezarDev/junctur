//! The parts Laya judges, one decision each: the silent pieces the labels have (3a) and the talk
//! between them, cut at its widest pauses into pieces of at most 30 s. From the speech regions, not
//! the edit list, so the user's cuts never move a part.

use serde::{Deserialize, Serialize};

use crate::labels::pieces::{self, LONGEST};
use crate::spans::{joined, Join};
use crate::vad::Region;

/// Shorter talk between two silent pieces is not a part (a cough on one track).
const SHORTEST_TALK: f64 = 0.3;
/// A talk piece is not cut shorter than this, so a cut lands in a pause, not after a word.
const SHORTEST_CUT: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Talk,
    Silent,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub start: f64,
    pub end: f64,
    pub kind: Kind,
}

impl Part {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    /// How much of it `ranges` (sorted, apart) cover, in seconds.
    pub fn covered(&self, ranges: &[crate::edit::Range]) -> f64 {
        let from = ranges.partition_point(|r| r.end <= self.start);
        ranges[from..]
            .iter()
            .take_while(|r| r.start < self.end)
            .map(|r| (r.end.min(self.end) - r.start.max(self.start)).max(0.0))
            .sum()
    }

    /// Whether `ranges` keep it: half of it or more.
    pub fn kept_by(&self, ranges: &[crate::edit::Range]) -> bool {
        self.covered(ranges) * 2.0 >= self.len()
    }
}

/// The parts of a recording `duration` seconds long whose speech is `regions`, in time order and
/// end to end: every silent piece ([`pieces::pieces`]) and the talk between.
pub fn parts(regions: &[Region], duration: f64) -> Vec<Part> {
    let silent = pieces::pieces(regions, duration);
    let mut spans: Vec<(f64, f64)> = regions.iter().map(|r| (r.start, r.end)).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let speech = joined(spans, Join::Touching);
    let mut out = Vec::with_capacity(silent.len() * 2 + 1);
    let mut at = 0.0f64;
    for p in silent.iter().chain(std::iter::once(&pieces::Piece {
        start: duration,
        end: duration,
    })) {
        if p.start - at >= SHORTEST_TALK {
            talk(at, p.start, &speech, &mut out);
        }
        if p.end > p.start {
            out.push(Part {
                start: p.start,
                end: p.end,
                kind: Kind::Silent,
            });
        }
        at = p.end;
    }
    out
}

/// The talk from `a` to `b` as pieces of at most [`LONGEST`]: each cut at the widest pause between
/// `speech` (joined, sorted) that leaves the piece before it at least [`SHORTEST_CUT`] long, else at
/// [`LONGEST`] itself.
fn talk(a: f64, b: f64, speech: &[(f64, f64)], out: &mut Vec<Part>) {
    let from = speech.partition_point(|s| s.1 <= a);
    let inside: Vec<(f64, f64)> = speech[from..]
        .iter()
        .take_while(|s| s.0 < b)
        .map(|s| (s.0.max(a), s.1.min(b)))
        .collect();
    // The pauses inside the stretch, as (middle, width).
    let pauses: Vec<(f64, f64)> = inside
        .windows(2)
        .filter(|w| w[1].0 > w[0].1)
        .map(|w| ((w[0].1 + w[1].0) / 2.0, w[1].0 - w[0].1))
        .collect();
    let mut start = a;
    while b - start > LONGEST {
        let cut = pauses
            .iter()
            .filter(|(m, _)| *m >= start + SHORTEST_CUT && *m <= start + LONGEST)
            .max_by(|x, y| x.1.total_cmp(&y.1).then(y.0.total_cmp(&x.0)))
            .map_or(start + LONGEST, |(m, _)| *m);
        out.push(Part {
            start,
            end: cut,
            kind: Kind::Talk,
        });
        start = cut;
    }
    out.push(Part {
        start,
        end: b,
        kind: Kind::Talk,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Region {
        Region { start, end }
    }

    #[test]
    fn silence_and_talk_cover_the_recording_end_to_end() {
        let p = parts(&[r(3.0, 10.0), r(10.5, 12.0), r(40.0, 41.0)], 50.0);
        let kinds: Vec<Kind> = p.iter().map(|x| x.kind).collect();
        use Kind::*;
        assert_eq!(kinds, [Silent, Talk, Silent, Talk, Silent]);
        assert_eq!(p[0].start, 0.0);
        assert_eq!(p.last().unwrap().end, 50.0);
        for w in p.windows(2) {
            assert!((w[0].end - w[1].start).abs() < 1e-9, "{w:?}");
        }
        // The half-second pause stays inside the talk.
        assert_eq!((p[1].start, p[1].end), (3.0, 12.0));
    }

    #[test]
    fn long_talk_is_cut_at_its_widest_pause_and_never_past_30_s() {
        // 70 s of talk with 0.4 s pauses, and one of 1.5 s after the fifth sentence (19.1 to 20.6 s):
        // the widest in the first 30 s.
        let mut regions = Vec::new();
        let mut t = 0.0;
        for i in 0..18 {
            regions.push(r(t, t + 3.5));
            t += 3.5 + if i == 4 { 1.5 } else { 0.4 };
        }
        let p = parts(&regions, regions.last().unwrap().end);
        assert!(p.iter().all(|x| x.kind == Kind::Talk));
        assert!(p.iter().all(|x| x.len() <= LONGEST + 1e-9), "{p:?}");
        assert!((p[0].end - 19.85).abs() < 1e-9, "{p:?}");
    }

    #[test]
    fn talk_with_no_pause_is_cut_at_30_s_and_nothing_is_lost() {
        let p = parts(&[r(0.0, 65.0)], 65.0);
        let ends: Vec<f64> = p.iter().map(|x| x.end).collect();
        assert_eq!(ends, [30.0, 60.0, 65.0]);
    }

    #[test]
    fn a_silent_recording_is_its_silent_pieces_and_an_empty_one_has_none() {
        let p = parts(&[], 45.0);
        assert!(p.iter().all(|x| x.kind == Kind::Silent));
        assert_eq!(p.len(), 2);
        assert!(parts(&[], 0.0).is_empty());
    }

    #[test]
    fn a_part_is_kept_when_half_of_it_is() {
        use crate::edit::Range;
        let part = Part {
            start: 10.0,
            end: 20.0,
            kind: Kind::Talk,
        };
        let keep = |v: &[(f64, f64)]| -> Vec<Range> {
            v.iter().map(|&(start, end)| Range { start, end }).collect()
        };
        assert!(part.kept_by(&keep(&[(0.0, 12.0), (14.0, 17.0)])));
        assert!(!part.kept_by(&keep(&[(0.0, 11.0), (19.0, 30.0)])));
        assert!(!part.kept_by(&[]));
    }
}
