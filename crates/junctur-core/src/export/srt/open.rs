//! The cue being filled: a part per speaker, lines wrapped greedily as words arrive.

use super::{width, Cue, Part, Spoken, PAUSE_S};
use crate::edit::Captions;

/// A kept word on the output timeline.
pub(super) struct Said {
    pub(super) track: usize,
    pub(super) start: f64,
    pub(super) end: f64,
    /// Everyone saying it, its one speaker first; empty for nobody (speaker 0).
    pub(super) speakers: Vec<u32>,
}

impl Said {
    fn speaker(&self) -> u32 {
        self.speakers.first().copied().unwrap_or(0)
    }

    /// The others an overlap stretch says are talking with its speaker.
    fn others(&self) -> &[u32] {
        self.speakers.get(1..).unwrap_or(&[])
    }

    fn spoken(&self, text: &str) -> Spoken {
        Spoken {
            text: text.to_string(),
            start: self.start,
            end: self.end,
        }
    }
}

/// A speaker's part of the open cue; its lines are wrapped as words arrive, so a word costs O(1).
struct OpenPart {
    speaker: u32,
    lines: Vec<String>,
    /// The units on the lines with their times.
    spoken: Vec<Spoken>,
    /// Units taken for the word limit, one per word or replaced phrase, whatever spaces its text
    /// holds; `last_words` of them on the last line.
    words: u32,
    last_words: u32,
    /// The track and end of its last word.
    track: usize,
    end: f64,
    /// Other speakers heard at once with any of its words (from overlap stretches).
    with: Vec<u32>,
}

impl OpenPart {
    fn new(w: &Said, text: String) -> Self {
        let mut p = Self {
            speaker: w.speaker(),
            spoken: vec![w.spoken(&text)],
            lines: vec![text],
            words: 1,
            last_words: 1,
            track: w.track,
            end: w.end,
            with: Vec::new(),
        };
        p.add_with(w);
        p
    }

    fn add_with(&mut self, w: &Said) {
        for &s in w.others() {
            if !self.with.contains(&s) {
                self.with.push(s);
            }
        }
    }
}

/// The cue being filled.
pub(super) struct Open {
    start: f64,
    end: f64,
    parts: Vec<OpenPart>,
}

impl Open {
    pub(super) fn new(w: &Said, text: String) -> Self {
        Self {
            start: w.start,
            end: w.end,
            parts: vec![OpenPart::new(w, text)],
        }
    }

    /// The cue that follows this one, opened by `w` (showing as `text`), which this one refused.
    /// Cues cannot overlap, so this one ends where the next starts; a speaker whose word on
    /// another track is still running at `w`'s start (talking at once when a part ran out of
    /// lines or the cue out of seconds) would vanish mid-word, so their last line is carried into
    /// the new cue, and their next words go on after it. Parts keep their order.
    pub(super) fn after(&self, w: &Said, text: String) -> Open {
        let speaker = w.speaker();
        let mut fresh = Some(OpenPart::new(w, text));
        let mut parts: Vec<OpenPart> = Vec::new();
        for p in &self.parts {
            if p.speaker == speaker {
                parts.extend(fresh.take());
            } else if p.track != w.track && w.start < p.end {
                parts.push(OpenPart {
                    speaker: p.speaker,
                    lines: p.lines.last().cloned().into_iter().collect(),
                    // The last line's units go with it.
                    spoken: p.spoken[p.spoken.len().saturating_sub(p.last_words as usize)..]
                        .to_vec(),
                    words: p.last_words,
                    last_words: p.last_words,
                    track: p.track,
                    end: p.end,
                    with: p.with.clone(),
                });
            }
        }
        parts.extend(fresh);
        Open {
            start: w.start,
            end: parts.iter().map(|p| p.end).fold(w.end, f64::max),
            parts,
        }
    }

    /// Whether `w`, by a speaker with no part here yet, is said at once with someone in the cue:
    /// it starts before their last word ends on another track (separate voice tracks overlap in
    /// time; on one track Whisper's word edges touch, which is no overlap), or an overlap stretch
    /// puts the two together (one mixed track: its words come one after another, alternating
    /// between the two people, so only the stretch says they overlap).
    fn at_once(&self, w: &Said) -> bool {
        let speaker = w.speaker();
        self.parts.iter().any(|p| {
            (p.track != w.track && w.start < p.end)
                || w.others().contains(&p.speaker)
                || p.with.contains(&speaker)
        })
    }

    /// Adds `w` (showing as `text`) if it belongs in this cue within the caption limits.
    pub(super) fn take(&mut self, w: &Said, text: &str, c: &Captions) -> bool {
        if w.end - self.start > c.seconds {
            return false;
        }
        let chars = c.chars as usize;
        let speaker = w.speaker();
        match self.parts.iter().position(|p| p.speaker == speaker) {
            Some(k) => {
                let p = &mut self.parts[k];
                if w.start - p.end >= PAUSE_S
                    || lines_with(&p.lines, text, chars) > c.lines as usize
                    || (c.words != 0 && p.words >= c.words)
                {
                    return false;
                }
                let had = p.lines.len();
                p.lines = wrap_onto(std::mem::take(&mut p.lines), text, chars);
                p.spoken.push(w.spoken(text));
                p.words += 1;
                p.last_words = if p.lines.len() > had {
                    1
                } else {
                    p.last_words + 1
                };
                p.track = w.track;
                p.end = p.end.max(w.end);
                p.add_with(w);
            }
            None => {
                if w.start - self.end >= PAUSE_S || !self.at_once(w) {
                    return false;
                }
                self.parts.push(OpenPart::new(w, text.to_string()));
            }
        }
        self.end = self.end.max(w.end);
        true
    }

    pub(super) fn close(self) -> Cue {
        Cue {
            start: self.start,
            end: self.end,
            parts: self
                .parts
                .into_iter()
                .map(|p| Part {
                    speaker: p.speaker,
                    lines: p.lines,
                    words: p.spoken,
                })
                .collect(),
        }
    }
}

/// Lines `lines` would have with `w` added (the greedy wrap of [`wrap_onto`]).
fn lines_with(lines: &[String], w: &str, chars: usize) -> usize {
    match lines.last() {
        Some(l) if width(l) + 1 + width(w) <= chars => lines.len(),
        _ => lines.len() + 1,
    }
}

/// `lines` with `w` added by the greedy wrap; the layout wraps a side-by-side column with it too,
/// so the preview and Resolve break lines alike.
pub(in crate::export) fn wrap_onto(mut lines: Vec<String>, w: &str, chars: usize) -> Vec<String> {
    match lines.last_mut() {
        Some(l) if width(l) + 1 + width(w) <= chars => {
            l.push(' ');
            l.push_str(w);
        }
        _ => lines.push(w.to_string()),
    }
    lines
}
