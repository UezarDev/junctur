//! The first automatic reduction: keep speech (from the VAD, not a volume threshold), drop the
//! silences between it, each boundary moved to the quietest instant nearby to land in a real gap.

use super::{CutSettings, Range};
use crate::spans::{joined, push_joined, Join};
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

/// Which side of a boundary in a gap is kept speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    Before,
    After,
}

/// Where a hand-made boundary goes in the pause `gap` between speech (where the speech before ends,
/// where the speech after starts), placed as [`keep_ranges`] places its own: `pad_after` past the
/// speech when the side before is kept (`kept_before`), else `pad_before` ahead of the speech
/// after, then the quietest step within `snap`. Never outside the gap, so it clips neither side.
/// `None` when there is no pause (the two overlap, e.g. on two tracks).
pub fn snap_boundary(env: &Envelope, gap: (f64, f64), kept: Kept, s: &CutSettings) -> Option<f64> {
    let (a, b) = gap;
    if b <= a {
        return None;
    }
    let target = match kept {
        Kept::Before => a + s.pad_after,
        Kept::After => b - s.pad_before,
    }
    .clamp(a, b);
    Some(env.quietest((target - s.snap).max(a), (target + s.snap).min(b), target))
}

/// The span a hand edit of a run of words changes (cut, or kept with `keep`), from a boundary placed
/// by [`snap_boundary`] in the pause before `run` to one in the pause after. `before`/`after` are
/// where earlier speech ends and later speech starts on any track, `None` at the transcript's ends.
/// With no pause there the boundary is the run's own edge, since a cut removes every track.
pub fn word_span(
    before: Option<f64>,
    run: (f64, f64),
    after: Option<f64>,
    keep: bool,
    env: &Envelope,
    s: &CutSettings,
    duration: f64,
) -> (f64, f64) {
    let pads = s.pad_before + s.pad_after;
    let before = before.unwrap_or((run.0 - pads).max(0.0));
    let after = after.unwrap_or((run.1 + pads).min(duration));
    // A cut ends kept speech before the run and starts it after; a keep the other way round.
    let (into, out_of) = if keep {
        (Kept::After, Kept::Before)
    } else {
        (Kept::Before, Kept::After)
    };
    let start = snap_boundary(env, (before, run.0), into, s).unwrap_or(run.0);
    let end = snap_boundary(env, (run.1, after), out_of, s).unwrap_or(run.1);
    (start, end.max(start))
}

/// Ranges to keep from speech regions (any order, any number of tracks) within `[0, total]`.
pub fn keep_ranges(speech: &[Region], env: &Envelope, total: f64, s: &CutSettings) -> Vec<Range> {
    let mut regions: Vec<Region> = speech.to_vec();
    regions.sort_by(|a, b| a.start.total_cmp(&b.start));
    // Speech spans: union of all tracks, with pauses under `merge_gap` filled in.
    let spans = joined(regions, Join::Within(s.merge_gap));
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
        push_joined(&mut out, Range { start, end }, Join::Touching);
    }
    out
}

#[cfg(test)]
mod tests;
