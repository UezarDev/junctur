//! The transcript as the editor sees it: every word of every track in one list, sorted by start, so
//! the frontend and the editor name a word by the same index. Words are kept as heard, which names
//! them in a fix or retime; where time places a word, the retimed words are read through [`Times`],
//! so the page and the captions place every word the same way.

use std::path::Path;

use junctur_core::edit::{
    same_millisecond, set_time, word_span, CutSettings, Envelope, Fix, Fixed, Fixes, Retime,
    Speakers, Times, WordAt,
};
use junctur_core::transcribe::{self, Word};
use serde::Serialize;

use crate::cuts::Fps;

/// How far from a moved edge the word it moves may end (or start) and still be the word beside it:
/// the pads leave an edge about half a second from its word, and a range kept by hand over a long
/// pause should not stretch its last word over the whole pause.
const REACH: f64 = 1.0;

/// Which end of a kept range an edge is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side {
    Start,
    End,
}

/// Every word's start and end with the list's retimed words in place, the latest end up to each
/// word, and the earliest start from each word on: a retimed word may start before the word heard
/// before it, so the next word in list order is not always the earliest to come.
struct Timing {
    start: Vec<f64>,
    end: Vec<f64>,
    ends_by: Vec<f64>,
    starts_from: Vec<f64>,
}

/// The latest of `ends` so far, at each.
fn ends_by(ends: impl Iterator<Item = f64>) -> Vec<f64> {
    ends.scan(f64::NEG_INFINITY, |m, e| {
        *m = m.max(e);
        Some(*m)
    })
    .collect()
}

#[derive(Debug, Default)]
pub struct Words {
    pub list: Vec<Word>,
    /// The audio track (0-based ordinal) each word was heard on.
    pub tracks: Vec<usize>,
    /// The latest end among words `0..=i`. With several tracks a long word can outlast the words
    /// that start after it, so speech up to word `i` ends here, not at word `i`'s own end.
    ends_by: Vec<f64>,
    /// The words so far of a recording still transcribing: every word before this many seconds is
    /// here, and words not yet transcribed start at or after it.
    until: Option<f64>,
}

impl Words {
    /// The project's `transcript.json`, or `None` when there is none (or it does not read).
    pub fn load(project_dir: &Path) -> Option<Self> {
        let t = transcribe::load(project_dir)?;
        Some(Self::from_segments(&t.segments))
    }

    /// The words so far of a recording still transcribing: those of the run's chunks that start
    /// before `through`, so the next read only adds words at the end and the page's indices stay
    /// valid. With no chunks yet, none: never an earlier run's `transcript.json`.
    #[cfg(test)]
    pub fn load_so_far(project_dir: &Path, through: f64) -> Self {
        Self::so_far(
            &transcribe::so_far(project_dir).unwrap_or_default(),
            through,
        )
    }

    /// [`Self::load_so_far`] from the partial's chunks as read (`transcribe::SoFar`).
    pub fn so_far(segs: &[transcribe::Segment], through: f64) -> Self {
        let mut w = Self::from_segments(segs);
        let n = w.list.partition_point(|x| x.start < through);
        w.list.truncate(n);
        w.tracks.truncate(n);
        w.ends_by.truncate(n);
        w.until = Some(through);
        w
    }

    fn from_segments(segs: &[transcribe::Segment]) -> Self {
        let words = segs
            .iter()
            .flat_map(|s| s.words.iter().map(|w| (s.track, w.clone())))
            .collect();
        Self::from_tracked(words)
    }

    /// Words all heard on track 0.
    #[cfg(test)]
    pub fn from_words(list: Vec<Word>) -> Self {
        Self::from_tracked(list.into_iter().map(|w| (0, w)).collect())
    }

    pub fn from_tracked(mut words: Vec<(usize, Word)>) -> Self {
        // Stable: words of one chunk keep their order when two start together.
        words.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
        let (tracks, list): (Vec<usize>, Vec<Word>) = words.into_iter().unzip();
        let ends_by = ends_by(list.iter().map(|w| w.end));
        Self {
            list,
            tracks,
            ends_by,
            until: None,
        }
    }

    /// Every word with its track, in the list's order: the order `edit::Fixes` and `edit::spread`
    /// read a transcript in.
    pub fn in_order(&self) -> Vec<(usize, &Word)> {
        self.tracks.iter().copied().zip(self.list.iter()).collect()
    }

    /// The words' times with the list's retimed words in place; `None` when no word is retimed.
    /// Read in list order, as the fixes and captions read the words, which counts each `nth`.
    fn timing(&self, times: &[Retime]) -> Option<Timing> {
        if times.is_empty() {
            return None;
        }
        let mut t = Times::new(times);
        let (start, end): (Vec<f64>, Vec<f64>) = self
            .list
            .iter()
            .zip(&self.tracks)
            .map(|(w, &k)| t.of(k, w))
            .unzip();
        let ends_by = ends_by(end.iter().copied());
        let mut starts_from = start.clone();
        for i in (0..starts_from.len().saturating_sub(1)).rev() {
            starts_from[i] = starts_from[i].min(starts_from[i + 1]);
        }
        Some(Timing {
            start,
            end,
            ends_by,
            starts_from,
        })
    }

    /// The list's retimed words after moving `side` of the kept range of frames `a..b` to frame
    /// `to`, so every word keeps the side of the cut it was on (`beyond`: the cut's far end). The
    /// edge word follows a growing edge within [`REACH`]; words it runs into are trimmed to it, one
    /// left under a frame kept is cut whole. Every new span is at least a frame long.
    pub fn moved_edge(
        &self,
        times: &[Retime],
        (a, b): (u64, u64),
        side: Side,
        to: u64,
        beyond: u64,
        fps: Fps,
    ) -> Vec<Retime> {
        let n = self.list.len();
        let t = self.timing(times);
        let s = |i: usize| t.as_ref().map_or(self.list[i].start, |t| t.start[i]);
        let e = |i: usize| t.as_ref().map_or(self.list[i].end, |t| t.end[i]);
        let per = fps.num as f64 / fps.den as f64;
        let frame = fps.seconds(1);
        // A word is where the frame at its middle is, as the page and the captions place it.
        let mid = |i: usize| (s(i) + e(i)) / 2.0 * per;
        let inside = |i: usize| (a as f64..b as f64).contains(&mid(i));
        let edge = fps.seconds(to);
        let mut moved: Vec<(usize, f64, f64)> = Vec::new();
        match side {
            Side::End if to > b => {
                let joined = to >= beyond;
                let gap = |i: usize| (b as f64..beyond as f64).contains(&mid(i));
                for i in (0..n).filter(|&i| !joined && gap(i)) {
                    if s(i) < edge && e(i) - frame >= edge {
                        moved.push((i, edge, e(i)));
                    }
                }
                let last = (0..n)
                    .filter(|&i| inside(i))
                    .max_by(|&i, &j| e(i).total_cmp(&e(j)).then(i.cmp(&j)));
                if let Some(w) = last.filter(|&w| e(w) >= fps.seconds(b) - REACH) {
                    let mut end = edge.min(fps.seconds(beyond));
                    // The next word on its track, passed whole: kept now, and this one ends where
                    // it starts.
                    let next = (w + 1..n).find(|&j| self.tracks[j] == self.tracks[w]);
                    if let Some(j) = next.filter(|&j| s(j) < edge && e(j) - frame < edge) {
                        end = end.min(s(j));
                    }
                    moved.push((w, s(w), end.max(s(w) + frame)));
                }
            }
            Side::End => {
                for i in (0..n).filter(|&i| inside(i) && s(i) < edge && e(i) > edge) {
                    if s(i) + frame <= edge {
                        moved.push((i, s(i), edge));
                    } else if e(i) - frame >= edge {
                        moved.push((i, edge, e(i)));
                    }
                }
            }
            Side::Start if to < a => {
                let joined = to <= beyond;
                let gap = |i: usize| (beyond as f64..a as f64).contains(&mid(i));
                for i in (0..n).filter(|&i| !joined && gap(i)) {
                    if e(i) > edge && s(i) + frame <= edge {
                        moved.push((i, s(i), edge));
                    }
                }
                let first = (0..n)
                    .filter(|&i| inside(i))
                    .min_by(|&i, &j| s(i).total_cmp(&s(j)).then(i.cmp(&j)));
                if let Some(w) = first.filter(|&w| s(w) <= fps.seconds(a) + REACH) {
                    let mut start = edge.max(fps.seconds(beyond));
                    let prev = (0..w).rev().find(|&j| self.tracks[j] == self.tracks[w]);
                    if let Some(j) = prev.filter(|&j| e(j) > edge && s(j) + frame > edge) {
                        start = start.max(e(j));
                    }
                    moved.push((w, start.min(e(w) - frame), e(w)));
                }
            }
            Side::Start => {
                for i in (0..n).filter(|&i| inside(i) && s(i) < edge && e(i) > edge) {
                    if e(i) - frame >= edge {
                        moved.push((i, edge, e(i)));
                    } else if s(i) + frame <= edge {
                        moved.push((i, s(i), edge));
                    }
                }
            }
        }
        let mut out = times.to_vec();
        for (i, from, to) in moved {
            let w = &self.list[i];
            // A span the millisecond grid leaves empty is refused: that word stays as it was.
            let _ = set_time(
                &mut out,
                self.tracks[i],
                w.start,
                self.nth(i),
                (w.start, w.end),
                (from, to),
            );
        }
        out
    }

    /// Which of the words heard on its track starting in its millisecond word `i` is, 0 for the
    /// first: what names it in a fix, with its track and start.
    pub fn nth(&self, i: usize) -> usize {
        let Some(w) = self.list.get(i) else {
            return 0;
        };
        (0..i)
            .rev()
            .take_while(|&j| same_millisecond(self.list[j].start, w.start))
            .filter(|&j| self.tracks[j] == self.tracks[i])
            .count()
    }

    /// What fixing words `first..=last` as one text names: the track, the first and last words as
    /// a fix names them, and what that track's words were heard as (other tracks' words are not in
    /// it). Refused when the ends are on two tracks, or a new text covers two people's words, as a
    /// caption gives each their own part. Clearing is never refused, so an old span can go.
    pub fn retyped(
        &self,
        first: usize,
        last: usize,
        text: &str,
        speakers: &Speakers,
        times: &[Retime],
    ) -> Result<Retyped, String> {
        let (first, last) = (first.min(last), first.max(last));
        if last >= self.list.len() {
            return Err("no such word in the transcript".into());
        }
        let track = self.tracks[first];
        if self.tracks[last] != track {
            return Err("These words start and end on two different voice tracks: fix each track's words on their own.".into());
        }
        let label = speakers.labeller();
        let name = |id: u32| speakers.name(id).unwrap_or("someone").to_string();
        let mut who: Option<u32> = None;
        let mut two: Option<(u32, u32)> = None;
        let mut heard: Vec<&str> = Vec::new();
        // Whose words they are by where they are now, as the page shows them.
        let t = self.timing(times);
        for i in (first..=last).filter(|&i| self.tracks[i] == track) {
            let w = &self.list[i];
            heard.push(w.text.trim());
            let (start, end) = t
                .as_ref()
                .map_or((w.start, w.end), |t| (t.start[i], t.end[i]));
            match (who, label.of_word(track, start, end)) {
                (Some(a), Some(b)) if a != b && two.is_none() => two = Some((a, b)),
                (None, Some(b)) => who = Some(b),
                _ => {}
            }
        }
        let heard = heard.join(" ");
        let new = junctur_core::edit::check_span(text).unwrap_or_default();
        let clearing =
            new.is_empty() || new == junctur_core::edit::check_span(&heard).unwrap_or_default();
        if let (Some((a, b)), false) = (two, clearing) {
            return Err(format!(
                "These words are said by two people ({} and {}): fix each one's words on their own.",
                name(a),
                name(b)
            ));
        }
        let at = |i: usize| WordAt {
            start: self.list[i].start,
            nth: self.nth(i),
        };
        Ok(Retyped {
            track,
            first: at(first),
            last: at(last),
            heard,
        })
    }

    /// The source seconds words `first..=last` are said in: the first one's start to the latest
    /// end up to the last (a voice is enrolled from this), retimed words where they now are. `None`
    /// for an index out of range.
    pub fn stretch(&self, first: usize, last: usize, times: &[Retime]) -> Option<(f64, f64)> {
        let (first, last) = (first.min(last), first.max(last));
        self.list.get(last)?;
        Some(match self.timing(times) {
            Some(t) => (t.start[first], t.ends_by[last]),
            None => (self.list[first].start, self.ends_by[last]),
        })
    }

    /// The source span that cutting (`keep` false) or keeping words `first..=last` changes, placed
    /// by [`word_span`] between the latest end of any earlier word and the earliest start after the
    /// run (or where a transcript still in progress stops). Words are where the retimed words put
    /// them. `None` for an index out of range.
    #[allow(clippy::too_many_arguments)]
    pub fn span(
        &self,
        first: usize,
        last: usize,
        keep: bool,
        env: &Envelope,
        s: &CutSettings,
        duration: f64,
        times: &[Retime],
    ) -> Option<(f64, f64)> {
        let (first, last) = (first.min(last), first.max(last));
        self.list.get(last)?;
        let t = self.timing(times);
        let start_of = |i: usize| t.as_ref().map_or(self.list[i].start, |t| t.start[i]);
        let ends_by = |i: usize| t.as_ref().map_or(self.ends_by[i], |t| t.ends_by[i]);
        // The earliest start of the words after `i`: the next one's while the list is sorted.
        let first_from = |i: usize| t.as_ref().map_or(self.list[i].start, |t| t.starts_from[i]);
        let start = start_of(first);
        let end = ends_by(last);
        let before = first.checked_sub(1).map(ends_by);
        // A word started before the transcript stops may end after it.
        let after = (last + 1 < self.list.len()).then(|| first_from(last + 1));
        let after = after.or(self.until.map(|u| u.max(end)));
        Some(word_span(
            before,
            (start, end),
            after,
            keep,
            env,
            s,
            duration,
        ))
    }
}

/// A run of words to fix as one text ([`Words::retyped`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Retyped {
    pub track: usize,
    pub first: WordAt,
    pub last: WordAt,
    /// The words on `track` as heard, joined by spaces.
    pub heard: String,
}

/// The words for the frontend, compact for a 30 h transcript (hundreds of thousands of words):
/// times in whole milliseconds (where the retimed words put them), each word's speaker id (0: none)
/// from the edit list's turns, and its text as the user fixed it, else as heard.
#[derive(Debug, Serialize)]
pub struct TranscriptView {
    pub text: Vec<String>,
    pub start: Vec<u64>,
    pub end: Vec<u64>,
    pub speaker: Vec<u32>,
    /// Someone else who says the word too (two people talking at once), 0 for none.
    pub also: Vec<u32>,
    /// The word's lowest token probability in whole percent, rounded down, for the page to flag
    /// under its threshold. [`NOT_FLAGGED`] with no probability, or for a word fixed, confirmed or
    /// inside a retyped span.
    pub sure: Vec<u8>,
    /// The audio track each word was heard on.
    pub track: Vec<u32>,
    /// The span fixed as one text each word is in, 1-based into `spans`; 0 for none. Such a
    /// word's `text` is what was heard: the span's text stands for all its words.
    pub span: Vec<u32>,
    /// Each span's text, in the order their first words come.
    pub spans: Vec<String>,
    /// Each word's start as heard, in whole milliseconds, when some word is retimed: what names a
    /// word in a fix, so words starting together are grouped by it, not by `start`. Empty (and not
    /// sent) when no word is retimed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub heard: Vec<u64>,
}

/// [`TranscriptView::sure`] of a word that is never flagged, above every percentage. The page has
/// the same number as `NOT_FLAGGED` (`ui/src/lib/transcript.ts`).
pub const NOT_FLAGGED: u8 = u8::MAX;

/// `p` (0..1) in whole percent, rounded down. In f64 with a hair added: `0.35_f32` is 0.34999999
/// and must read as 35, while a real 34.99 % stays 34.
fn percent(p: f32) -> u8 {
    (f64::from(p.clamp(0.0, 1.0)) * 100.0 + 1e-4).floor() as u8
}

impl TranscriptView {
    pub fn new(words: &Words, speakers: &Speakers, fixes: &[Fix], times: &[Retime]) -> Self {
        let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
        let mut fixed = Fixes::new(fixes);
        let w = &words.list;
        let heard = if times.is_empty() {
            Vec::new()
        } else {
            w.iter().map(|w| ms(w.start)).collect()
        };
        let (starts, ends): (Vec<f64>, Vec<f64>) = match words.timing(times) {
            Some(t) => (t.start, t.end),
            None => w.iter().map(|w| (w.start, w.end)).unzip(),
        };
        let label = speakers.labeller();
        let (speaker, also) = (0..w.len())
            .zip(&words.tracks)
            .map(|(i, &t)| {
                let all = label.speakers_of_word(t, starts[i], ends[i]);
                (
                    all.first().copied().unwrap_or(0),
                    all.get(1).copied().unwrap_or(0),
                )
            })
            .unzip();
        // One pass, each word read once: `Fixes` counts the words of a millisecond as it reads
        // them, so a word read twice, or one skipped, would give the next one another's fix.
        let mut text = Vec::with_capacity(w.len());
        let mut sure = Vec::with_capacity(w.len());
        let mut span = Vec::with_capacity(w.len());
        // Each span's id here (1-based), by its place in `fixes`.
        let mut ids: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
        let mut spans: Vec<String> = Vec::new();
        for (w, &t) in w.iter().zip(&words.tracks) {
            let fix = fixed.of(t, w);
            // A word fixed, confirmed or in a retyped span is vouched for: never flagged.
            sure.push(match (w.p_min, fix) {
                (Some(p), None) => percent(p),
                _ => NOT_FLAGGED,
            });
            let (shown, id) = match fix {
                Some(Fixed::Word(fix)) => (fix, 0),
                Some(Fixed::Span { index, text }) => {
                    let id = *ids.entry(index).or_insert_with(|| {
                        spans.push(text.to_string());
                        spans.len() as u32
                    });
                    (w.text.as_str(), id)
                }
                None => (w.text.as_str(), 0),
            };
            text.push(shown.trim().to_string());
            span.push(id);
        }
        Self {
            text,
            start: starts.iter().map(|&s| ms(s)).collect(),
            end: ends.iter().map(|&e| ms(e)).collect(),
            speaker,
            also,
            sure,
            track: words.tracks.iter().map(|&t| t as u32).collect(),
            span,
            spans,
            heard,
        }
    }
}

#[cfg(test)]
mod tests;
