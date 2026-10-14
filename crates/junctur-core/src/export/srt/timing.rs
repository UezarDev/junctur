//! Cue times once every cue is cut: in order, never overlapping, each held long enough to read.

use super::{Cue, Part, MIN_S};

/// Cues in order and never overlapping (a Resolve subtitle track holds one at a time), all within
/// the edited timeline's `total` seconds. Each cue lasts at least `floor`, then ends by the next
/// one's start, held to [`MIN_S`] where there is room, never past `longest`. A cue pushed past the
/// end joins the one before. Under a word limit nothing is pushed, and cues said at one instant
/// share it ([`share_instants`]).
pub(super) fn settle(cues: &mut Vec<Cue>, longest: f64, floor: f64, total: f64) {
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    if floor == 0.0 {
        share_instants(cues, total);
    }
    for i in 0..cues.len() {
        let floor = cues[i].start + floor;
        if let Some(n) = cues.get_mut(i + 1) {
            if n.start < floor {
                n.end += floor - n.start;
                n.start = floor;
            }
        }
        let next = cues.get(i + 1).map_or(f64::INFINITY, |n| n.start);
        let c = &mut cues[i];
        let want = c.end.max(c.start + MIN_S).min(c.start + longest);
        c.end = want.min(next).min(total);
    }
    // A cue pushed past the end of the timeline has no time left; its words go into the cue
    // before it rather than being lost, or into the next when it is the first.
    let mut kept: Vec<Cue> = Vec::with_capacity(cues.len());
    let mut early: Vec<Part> = Vec::new();
    for mut c in cues.drain(..) {
        if c.end <= c.start {
            match kept.last_mut() {
                Some(prev) => join(&mut prev.parts, c.parts),
                None => join(&mut early, c.parts),
            }
        } else {
            if !early.is_empty() {
                let own = std::mem::replace(&mut c.parts, std::mem::take(&mut early));
                join(&mut c.parts, own);
            }
            kept.push(c);
        }
    }
    *cues = kept;
}

/// `more` added to `parts`, a speaker's lines after their own lines there.
fn join(parts: &mut Vec<Part>, more: Vec<Part>) {
    for part in more {
        match parts.iter_mut().find(|p| p.speaker == part.speaker) {
            Some(p) => {
                p.lines.extend(part.lines);
                p.words.extend(part.words);
            }
            None => parts.push(part),
        }
    }
}

/// Cues (in order) said at one instant, spread over it in turn, under a word limit: Whisper gives
/// a word no length now and then, and a long translation puts several words where a spoken word
/// ends. The cues split the time their words take (the minimum hold when none has a length), never
/// past the next cue's start nor the timeline's end, so each keeps its words within the limit.
fn share_instants(cues: &mut [Cue], total: f64) {
    let mut i = 0;
    while i < cues.len() {
        let t = cues[i].start;
        let k = cues[i..].iter().take_while(|c| c.start <= t).count();
        if k > 1 {
            let next = cues.get(i + k).map_or(f64::INFINITY, |n| n.start);
            let said = cues[i..i + k].iter().map(|c| c.end).fold(t, f64::max);
            let end = if said > t { said } else { t + MIN_S };
            let step = (end.min(next).min(total) - t).max(0.0) / k as f64;
            for (m, c) in cues[i..i + k].iter_mut().enumerate() {
                c.start = t + m as f64 * step;
            }
        }
        i += k;
    }
}
