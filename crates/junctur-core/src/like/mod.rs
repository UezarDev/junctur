//! "Show me one" (phase 7b, a prototype): instead of describing what to keep, the user points at it.
//! A moment or two of a track are the examples, every moment that sounds like them is found
//! ([`search`] over the track's [`mel`] spectrogram), and what to do with all of them is one choice:
//! keep a little around each and cut the rest, cut each, or send each to the moments list. A module
//! of its own, pure but for its files, so 7c's recipes can call [`find`] as their "like this" tool.
//!
//! The search, its examples and what was done with it are kept in [`FILE`] in the project folder,
//! so a reopened recording shows them again. Edits are proposals first and accepted like smart keep's
//! suggestions; what the list kept where an accepted one edited is stored with it, so its undo puts
//! back exactly that (as 7a's moments do).

pub mod mel;
pub mod search;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::edit::{kept_in, Range};
use crate::moments::{Marker, Source};
use crate::{Error, Result};

pub use mel::Spectrogram;

/// The project file.
pub const FILE: &str = "like.json";
/// A dragged example is at most this long; a click is this long, around the click.
pub const MAX_EXAMPLE_S: f64 = 3.0;
pub const CLICK_S: f64 = 0.5;
/// The search keeps every peak at or above this; the slider only filters them.
pub const FLOOR: f32 = 0.3;
/// At most this many peaks are kept, the highest (the view says when a search reached it).
pub const MAX_MATCHES: usize = 20_000;
/// Where the slider starts.
pub const THRESHOLD: f64 = 0.6;
/// Kept (or cut) around a match's point: the research's quarter second for an ore.
pub const BEFORE_S: f64 = 0.1;
pub const AFTER_S: f64 = 0.15;
/// What a sent match is called in the moments list.
const LABEL: &str = "Like this";

/// A marked example: `start..end` of the track, and its point (the click, or the start of a
/// dragged span), which is where each match's point falls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub start: f64,
    pub end: f64,
    pub point: f64,
    /// `false`: a "not this".
    pub positive: bool,
}

impl Example {
    /// [`CLICK_S`] around a click at `at` (never before 0).
    pub fn click(at: f64, positive: bool) -> Self {
        let start = (at - CLICK_S / 2.0).max(0.0);
        Self {
            start: ms(start),
            end: ms(start + CLICK_S),
            point: ms(at.max(0.0)),
            positive,
        }
    }

    /// A dragged `a..b`, its first [`MAX_EXAMPLE_S`] when longer.
    pub fn span(a: f64, b: f64, positive: bool) -> Self {
        let start = a.min(b).max(0.0);
        let end = a.max(b).min(start + MAX_EXAMPLE_S);
        Self {
            start: ms(start),
            end: ms(end),
            point: ms(start),
            positive,
        }
    }
}

/// A moment that sounds like an example: `start..end` the window it matched in, `point` where
/// the example's point falls in it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    pub start: f64,
    pub end: f64,
    pub point: f64,
    pub score: f64,
}

/// The matches of `examples` in `spec`, within `range` (seconds; the whole track when `None`),
/// in time order, every peak at or above [`FLOOR`]. Refused without a positive example that has
/// sound in it.
pub fn find(
    spec: &Spectrogram,
    examples: &[Example],
    range: Option<(f64, f64)>,
) -> Result<Vec<Match>> {
    let frame = |s: f64| ((s / mel::HOP).round().max(0.0) as usize).min(spec.frames());
    let patch = |e: &Example| {
        let (a, b) = (frame(e.start), frame(e.end));
        search::Patch::of(spec, a, b.saturating_sub(a))
    };
    let mut pos = Vec::new();
    let mut owners = Vec::new();
    for e in examples.iter().filter(|e| e.positive) {
        if let Some(p) = patch(e) {
            pos.push(p);
            owners.push(e);
        }
    }
    if pos.is_empty() {
        return Err(Error::Like(if examples.iter().any(|e| e.positive) {
            "the example has no sound in it to look for: mark a moment where something is heard"
                .into()
        } else {
            "mark a moment first: the selection, or a click at the playhead".into()
        }));
    }
    let neg: Vec<search::Patch> = examples
        .iter()
        .filter(|e| !e.positive)
        .filter_map(patch)
        .collect();
    let (from, to) = match range {
        Some((a, b)) => (frame(a.min(b)), frame(a.max(b))),
        None => (0, spec.frames()),
    };
    let s = search::scores(spec, from, to, &pos, &neg);
    let gap = pos.iter().map(|p| p.len).min().unwrap_or(1).max(1);
    Ok(search::peaks(&s.score, FLOOR, gap, MAX_MATCHES)
        .into_iter()
        .map(|(i, score)| {
            let k = s.best[i] as usize;
            let e = owners[k];
            let start = (from + i) as f64 * mel::HOP;
            Match {
                start: ms(start),
                end: ms(start + pos[k].len as f64 * mel::HOP),
                point: ms(start + (e.point - e.start)),
                score: ((score as f64) * 1000.0).round() / 1000.0,
            }
        })
        .collect())
}

/// What a proposal does to the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Keep a little around each match and cut the rest of the searched range.
    KeepAround,
    /// Cut a little around each match.
    CutEach,
}

/// Edits waiting for the user's Accept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub kind: Kind,
    /// The seconds it edits.
    pub range: (f64, f64),
    /// What it keeps (keep around) or cuts (cut each), merged, in order.
    pub spans: Vec<(f64, f64)>,
    pub matches: usize,
}

impl Proposal {
    /// The edits (start, end, keep), in order: keep-around cuts the range and keeps its spans.
    pub fn edits(&self) -> Vec<(f64, f64, bool)> {
        match self.kind {
            Kind::KeepAround => std::iter::once((self.range.0, self.range.1, false))
                .chain(self.spans.iter().map(|&(a, b)| (a, b, true)))
                .collect(),
            Kind::CutEach => self.spans.iter().map(|&(a, b)| (a, b, false)).collect(),
        }
    }
}

/// An accepted proposal, with what the list kept of its range before, for its undo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub id: u64,
    #[serde(flatten)]
    pub proposal: Proposal,
    pub before: Vec<Range>,
}

/// How matches go to the moments list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendKind {
    /// Each a marker: the half minute before it, as a hotkey would.
    Mark,
    /// Each a clip of its own: `point - before .. point + after`.
    Clip,
}

/// Matches sent to the moments list, as one batch the Like this tab can take back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub id: u64,
    pub kind: SendKind,
    pub points: Vec<f64>,
    /// Clip only: each clip's seconds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<(f64, f64)>,
}

/// The last search, how long it took and on what.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Searched {
    pub track: usize,
    pub range: Option<(f64, f64)>,
    pub examples: usize,
    /// Seconds the search took, and whether the track's spectrogram was built for it.
    pub seconds: f64,
    pub built: bool,
    /// It found [`MAX_MATCHES`]: the least alike past those were left out.
    #[serde(default)]
    pub capped: bool,
}

/// [`FILE`]: the user's examples and settings, the last search's matches, and what was done.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Like {
    pub version: u32,
    /// The 0-based audio track searched; `None`: the game track, else the mix.
    pub track: Option<usize>,
    pub examples: Vec<Example>,
    /// Search only these seconds; `None`: the whole recording.
    pub range: Option<(f64, f64)>,
    pub threshold: f64,
    pub before: f64,
    pub after: f64,
    /// Points of matches the user ticked: shown whatever the threshold.
    pub confirmed: Vec<f64>,
    pub matches: Vec<Match>,
    pub searched: Option<Searched>,
    pub proposal: Option<Proposal>,
    pub applied: Vec<Applied>,
    pub sent: Vec<Sent>,
    pub next_id: u64,
}

impl Default for Like {
    fn default() -> Self {
        Self {
            version: 1,
            track: None,
            examples: Vec::new(),
            range: None,
            threshold: THRESHOLD,
            before: BEFORE_S,
            after: AFTER_S,
            confirmed: Vec::new(),
            matches: Vec::new(),
            searched: None,
            proposal: None,
            applied: Vec::new(),
            sent: Vec::new(),
            next_id: 1,
        }
    }
}

impl Like {
    fn confirmed(&self, m: &Match) -> bool {
        self.confirmed.iter().any(|&p| (p - m.point).abs() < 1e-3)
    }

    /// The matches shown: at or above the threshold, or ticked.
    pub fn shown(&self) -> Vec<&Match> {
        self.matches
            .iter()
            .filter(|m| m.score >= self.threshold || self.confirmed(m))
            .collect()
    }

    /// The seconds each shown match acts on, `point - before .. point + after`, inside `range`
    /// (the searched range, else `0..duration`), merged where they touch, in order.
    fn windows(&self, range: (f64, f64)) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        for m in self.shown() {
            let a = ms((m.point - self.before.max(0.0)).max(range.0));
            let b = ms((m.point + self.after.max(0.0)).min(range.1));
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

    /// The recording's end: `duration`, or past the last match when the length is not known (0).
    fn end(&self, duration: f64) -> f64 {
        if duration > 0.0 {
            return duration;
        }
        let last = self.matches.iter().map(|m| m.point).fold(0.0, f64::max);
        last + self.after.max(0.0) + 1.0
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    /// Proposes `kind` over the shown matches of a recording `duration` long (0: not known);
    /// refused when none is shown.
    pub fn propose(&mut self, kind: Kind, duration: f64) -> Result<&Proposal> {
        let range = self.range.unwrap_or((0.0, self.end(duration)));
        let spans = self.windows(range);
        if spans.is_empty() {
            return Err(Error::Like(
                "no match is shown: search, or lower the slider".into(),
            ));
        }
        let matches = self.shown().len();
        let range = match kind {
            Kind::KeepAround => (ms(range.0), ms(range.1)),
            Kind::CutEach => (spans[0].0, spans[spans.len() - 1].1),
        };
        Ok(self.proposal.insert(Proposal {
            kind,
            range,
            spans,
            matches,
        }))
    }

    /// Accepts the proposal against the list's kept ranges `now` (sorted): its edits, the
    /// proposal moved to [`Like::applied`] with what `now` keeps of its range; `None` without one.
    pub fn accept(&mut self, now: &[Range]) -> Option<Vec<(f64, f64, bool)>> {
        let proposal = self.proposal.take()?;
        let edits = proposal.edits();
        let before = kept_in(now, proposal.range.0, proposal.range.1);
        let id = self.take_id();
        self.applied.push(Applied {
            id,
            proposal,
            before,
        });
        Some(edits)
    }

    /// The edits that undo applied `id` (its range cut, then what was kept there kept again), and
    /// it forgotten; nothing when there is no such one.
    pub fn undo(&mut self, id: u64) -> Vec<(f64, f64, bool)> {
        let Some(i) = self.applied.iter().position(|a| a.id == id) else {
            return Vec::new();
        };
        let a = self.applied.remove(i);
        let (from, to) = a.proposal.range;
        std::iter::once((from, to, false))
            .chain(a.before.iter().map(|r| (r.start, r.end, true)))
            .collect()
    }

    /// Sends every shown match to the moments list; refused when none is shown.
    pub fn send(&mut self, kind: SendKind, duration: f64) -> Result<&Sent> {
        let points: Vec<f64> = self.shown().iter().map(|m| m.point).collect();
        if points.is_empty() {
            return Err(Error::Like(
                "no match is shown: search, or lower the slider".into(),
            ));
        }
        let spans = match kind {
            SendKind::Mark => Vec::new(),
            SendKind::Clip => points
                .iter()
                .map(|&p| {
                    let a = (p - self.before.max(0.0)).max(0.0);
                    (ms(a), ms((p + self.after.max(0.0)).min(self.end(duration))))
                })
                .collect(),
        };
        let id = self.take_id();
        self.sent.push(Sent {
            id,
            kind,
            points,
            spans,
        });
        Ok(self.sent.last().expect("just pushed"))
    }

    /// Takes sent batch `id` back out of the moments list; whether there was one.
    pub fn take_back(&mut self, id: u64) -> bool {
        let n = self.sent.len();
        self.sent.retain(|s| s.id != id);
        self.sent.len() != n
    }

    /// The moments markers of every sent batch.
    pub fn markers(&self) -> Vec<Marker> {
        self.sent
            .iter()
            .flat_map(|s| {
                s.points.iter().enumerate().map(move |(i, &at)| Marker {
                    at,
                    source: Source::Like,
                    cut: None,
                    text: LABEL.into(),
                    span: s.spans.get(i).copied(),
                })
            })
            .collect()
    }

    /// A new search's matches: ticks of matches no longer found dropped, and the proposal (made of
    /// the old matches) with them.
    pub fn found(&mut self, matches: Vec<Match>, searched: Searched) {
        self.confirmed
            .retain(|&p| matches.iter().any(|m| (m.point - p).abs() < 1e-3));
        self.matches = matches;
        self.searched = Some(searched);
        self.proposal = None;
    }
}

fn ms(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub fn load(dir: &Path) -> Result<Like> {
    match std::fs::read(dir.join(FILE)) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Like::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(dir: &Path, l: &Like) -> Result<()> {
    crate::output::write_replacing(&dir.join(FILE), serde_json::to_vec_pretty(l)?)?;
    Ok(())
}

#[cfg(test)]
mod tests;
