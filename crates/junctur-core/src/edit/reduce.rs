//! The first automatic reduction: keep speech, drop the silences between it.
//!
//! "Silence" means nobody talking (from the VAD), not a volume threshold, so the ranges come from
//! speech regions. Each is padded, short pauses are kept so speech is not chopped, and then each
//! boundary moves to the quietest instant nearby on the speech tracks' energy, so a cut lands in a
//! real gap rather than on the tail of a breath or a word the VAD trimmed.

use super::{CutSettings, Range};
use crate::vad::Region;

/// Envelope resolution: 10 ms.
pub const ENVELOPE_HOP: f64 = 0.01;

/// RMS level per [`ENVELOPE_HOP`] of the speech tracks (the loudest track at each instant).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Envelope(pub Vec<f32>);

impl Envelope {
    /// Quietest envelope step in `[a, b]`; on a tie the one nearest `prefer`. Without data, `prefer`.
    fn quietest(&self, a: f64, b: f64, prefer: f64) -> f64 {
        // Integer steps: adding 0.01 repeatedly drifts off the grid.
        let first = (a / ENVELOPE_HOP - 1e-6).ceil().max(0.0) as usize;
        let last = ((b / ENVELOPE_HOP + 1e-6).floor() as usize).min(self.0.len().saturating_sub(1));
        if self.0.is_empty() || b <= a || first > last {
            return prefer;
        }
        let mut best = (f32::INFINITY, f64::INFINITY, prefer);
        for i in first..=last {
            let t = i as f64 * ENVELOPE_HOP;
            let key = (self.0[i], (t - prefer).abs());
            if key.0 < best.0 || (key.0 == best.0 && key.1 < best.1) {
                best = (key.0, key.1, t);
            }
        }
        best.2
    }
}

/// Where a hand-made boundary goes in the pause `gap` between two words (end of the one before,
/// start of the one after), placed as [`keep_ranges`] places its own: `pad_after` past the speech
/// when the side before is kept (`kept_before`), else `pad_before` ahead of the speech after, then
/// the quietest step within `snap`. Never outside the gap, so a cut never clips either word; a gap
/// the words overlap in gives its middle.
pub fn snap_boundary(env: &Envelope, gap: (f64, f64), kept_before: bool, s: &CutSettings) -> f64 {
    let (a, b) = gap;
    if b <= a {
        return (a + b) / 2.0;
    }
    let target = if kept_before {
        a + s.pad_after
    } else {
        b - s.pad_before
    }
    .clamp(a, b);
    env.quietest((target - s.snap).max(a), (target + s.snap).min(b), target)
}

/// Ranges to keep from speech regions (any order, any number of tracks) within `[0, total]`.
pub fn keep_ranges(speech: &[Region], env: &Envelope, total: f64, s: &CutSettings) -> Vec<Range> {
    let mut regions: Vec<Region> = speech.to_vec();
    regions.sort_by(|a, b| a.start.total_cmp(&b.start));
    // Speech spans: union of all tracks, with pauses under `merge_gap` filled in.
    let mut spans: Vec<Region> = Vec::new();
    for r in regions {
        match spans.last_mut() {
            Some(last) if r.start - last.end < s.merge_gap => last.end = last.end.max(r.end),
            _ => spans.push(r),
        }
    }
    let mut out: Vec<Range> = Vec::new();
    for sp in spans {
        let start = (sp.start - s.pad_before).max(0.0);
        let end = (sp.end + s.pad_after).min(total);
        // A boundary moves only away from speech's side of the pad: never into the words.
        let start = env.quietest(
            (start - s.snap).max(0.0),
            (start + s.snap).min(sp.start),
            start,
        );
        let end = env.quietest((end - s.snap).max(sp.end), (end + s.snap).min(total), end);
        if end <= start {
            continue;
        }
        match out.last_mut() {
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => out.push(Range { start, end }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Region {
        Region { start, end }
    }

    fn flat() -> Envelope {
        Envelope(vec![1.0; 10_000])
    }

    #[test]
    fn no_speech_keeps_nothing() {
        assert!(keep_ranges(&[], &flat(), 100.0, &CutSettings::default()).is_empty());
    }

    #[test]
    fn pads_merges_and_clamps() {
        let s = CutSettings::default();
        let k = keep_ranges(
            &[r(10.0, 12.0), r(0.1, 1.0), r(12.5, 13.0), r(30.0, 40.0)],
            &flat(),
            40.1,
            &s,
        );
        assert_eq!(k.len(), 3);
        assert_eq!(k[0].start, 0.0);
        assert!((k[1].start - 9.75).abs() < 1e-9 && (k[1].end - 13.35).abs() < 1e-9);
        assert!((k[2].end - 40.1).abs() < 1e-9);
    }

    #[test]
    fn overlapping_tracks_union() {
        let k = keep_ranges(
            &[r(5.0, 8.0), r(6.0, 7.0), r(7.5, 9.0)],
            &flat(),
            100.0,
            &CutSettings::default(),
        );
        assert_eq!(k.len(), 1);
        assert!((k[0].end - 9.35).abs() < 1e-9);
    }

    #[test]
    fn snaps_to_quiet_but_not_into_speech() {
        // Loud everywhere except a dip at 9.70 s (before the pad start of 9.75) and one at 10.05 s,
        // inside the speech: the start must take the first and ignore the second.
        let mut e = vec![1.0f32; 2000];
        e[970] = 0.0;
        e[1005] = 0.0;
        let k = keep_ranges(
            &[r(10.0, 11.0)],
            &Envelope(e),
            100.0,
            &CutSettings::default(),
        );
        assert!((k[0].start - 9.70).abs() < 1e-6, "{:?}", k[0]);
    }

    #[test]
    fn hand_boundaries_pad_snap_and_stay_in_the_gap() {
        let s = CutSettings::default();
        // Flat: just the pad. After kept speech ending at 10.0, 0.35 s later; before speech at
        // 12.0, 0.25 s earlier.
        let b = snap_boundary(&flat(), (10.0, 12.0), true, &s);
        assert!((b - 10.35).abs() < 1e-9, "{b}");
        let b = snap_boundary(&flat(), (10.0, 12.0), false, &s);
        assert!((b - 11.75).abs() < 1e-9, "{b}");
        // A pause shorter than the pad: the far edge of the gap, never into the next word.
        let b = snap_boundary(&flat(), (10.0, 10.1), true, &s);
        assert!((b - 10.1).abs() < 1e-9, "{b}");
        // A quiet step within `snap` wins, but not one past the next word's start.
        let mut e = vec![1.0f32; 2000];
        e[1040] = 0.0;
        e[1060] = 0.0;
        let b = snap_boundary(&Envelope(e.clone()), (10.0, 10.5), true, &s);
        assert!((b - 10.40).abs() < 1e-6, "{b}");
        // Before speech at 10.45: the target is 10.20, and 10.40 is beyond `snap` of it.
        let b = snap_boundary(&Envelope(e), (10.0, 10.45), false, &s);
        assert!((b - 10.20).abs() < 1e-6, "{b}");
        // Overlapping words (two tracks): the middle.
        assert_eq!(snap_boundary(&flat(), (5.0, 4.0), true, &s), 4.5);
    }
}
