//! Captions as an SRT file on the edited timeline, each cue in its speaker's colour (5a).
//!
//! SRT because it is the one format checked to bring per-cue colour into DaVinci Resolve: Uezar
//! imported a test file with `<font color>` per cue and the colours came through; the same test as
//! WebVTT (`::cue` classes) did not. Premiere Pro is unverified; reports say its import gives every
//! caption its default style. Only what an SRT carries into Resolve is written: text, timing,
//! colour, and bold, italic (5d) or underline (5f). Each line gets its own `<font>` tag, since a tag spanning a line break is the case
//! importers disagree on.
//!
//! One SRT holds one Resolve subtitle track, and a track shows one caption at a time: Uezar's
//! import of 26 overlapping pairs in one file came in without overlaps (5b). 5b answered with a file
//! per speaker, one track each; Uezar wanted one track (5e). So when people talk at once, one cue
//! holds them all ([`Part`]): each speaker's words on their own line(s) in their colour, a line break
//! between speakers, the caption settings' line limit per speaker, and the cue lasting until the
//! last of them stops. Cues never overlap.
//!
//! Times are output times: a kept word is mapped through the kept ranges on the frame grid
//! ([`frame_ranges`]), exactly as the renderer and the FCPXML lay the ranges end to end, so a cue
//! lands on the same frames in the render and in Resolve's timeline. A range-by-range sum of
//! seconds would drift by up to a frame per cut, seconds over a long edit.

use std::fmt::Write;

use serde::Serialize;

use super::caption_text;
use super::translated::{self, Shown};
use crate::edit::{frame_ranges, frames_ceil, Captions, EditList, Labeller};
use crate::transcribe::Word;
use crate::translate::{Sentence, Translations};

/// A new cue after a pause this long in the edited timeline, even mid-sentence.
const PAUSE_S: f64 = 1.0;
/// A cue stays at least this long when the next one leaves room, so a lone short word is not
/// gone before it is read.
const MIN_S: f64 = 0.8;
/// No cue is shorter than this: when the next one would start sooner (quick turns, or a stacked
/// overlap cut at its limits), the next one waits. Shorter than [`MIN_S`] so a quick exchange does
/// not drift far behind. Not under a word limit (5i): there each cue is a word or a few, quicker
/// than any floor, and every wait would add to the last until the captions trail the voice by
/// seconds; a word cue starts when it is said and lasts until the next.
const READABLE_S: f64 = 0.5;

/// One speaker's lines in a cue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Part {
    /// Speaker id, 0 for none.
    pub speaker: u32,
    pub lines: Vec<String>,
}

/// A caption: one part per speaker talking in it, in the order they started, so more than one only
/// where people talk at once. Serialized for the app's preview over the picture (5d).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cue {
    /// Output seconds.
    pub start: f64,
    pub end: f64,
    pub parts: Vec<Part>,
}

/// Source seconds to output seconds through the kept frame ranges.
struct Timeline {
    ranges: Vec<(u64, u64)>,
    /// Output frames before each range.
    before: Vec<u64>,
    num: f64,
    den: f64,
}

impl Timeline {
    /// Output seconds of the whole edited timeline.
    fn total(&self) -> f64 {
        let frames =
            self.before.last().copied().unwrap_or(0) + self.ranges.last().map_or(0, |(a, b)| b - a);
        frames as f64 * self.den / self.num
    }

    fn new(edit: &EditList, num: u64, den: u64) -> Self {
        let total = frames_ceil(edit.duration, num, den);
        let ranges = frame_ranges(&edit.keep, num, den, total);
        let before = ranges
            .iter()
            .scan(0, |acc, (a, b)| {
                let at = *acc;
                *acc += b - a;
                Some(at)
            })
            .collect();
        Self {
            ranges,
            before,
            num: num as f64,
            den: den as f64,
        }
    }

    /// The range holding source second `t`, if it is kept (a word is kept when its middle is, as
    /// the transcript panel decides).
    fn range_of(&self, t: f64) -> Option<usize> {
        let f = t * self.num / self.den;
        let i = self.ranges.partition_point(|&(_, b)| (b as f64) <= f);
        self.ranges
            .get(i)
            .filter(|&&(a, _)| a as f64 <= f)
            .map(|_| i)
    }

    /// Output seconds of source second `t`, held inside range `i` (a word's edge may lie in the
    /// cut beside it).
    fn map(&self, i: usize, t: f64) -> f64 {
        let (a, b) = self.ranges[i];
        let f = (t * self.num / self.den).clamp(a as f64, b as f64);
        (self.before[i] as f64 + f - a as f64) * self.den / self.num
    }
}

/// Characters as a reader counts them.
fn width(s: &str) -> usize {
    s.chars().count()
}

/// What a word shows as: whole `<...>` and `{...}` spans removed (they would read as tags or
/// `{\an8}`-style codes; a bracket left open drops just the bracket), line breaks as spaces (a
/// blank line ends a cue), runs of space as one, trimmed.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        let close = match c {
            '<' => '>',
            '{' => '}',
            '>' | '}' => continue,
            '\r' | '\n' | '\t' => {
                out.push(' ');
                continue;
            }
            _ => {
                out.push(c);
                continue;
            }
        };
        // Skip the span and its closing bracket, if it closes.
        if chars.clone().any(|x| x == close) {
            for x in chars.by_ref() {
                if x == close {
                    break;
                }
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The cues for `edit`'s kept words. `words` is every transcript word with the track it was heard
/// on, in any order; `num/den` is the frame rate the edit is rounded to (the source's).
///
/// Each word goes under its one speaker ([`crate::edit::Labeller::of_word`], as the transcript view
/// shows it), never on two lines: where two people talk over each other on one mixed track the
/// transcript cannot tell whose word it is, and the same words twice in two colours would read as
/// a stutter. A word joins the open cue on its speaker's part while it is within [`PAUSE_S`] of
/// their last word there, fits their `lines` and `words` (5i), and the cue stays within `seconds`. A speaker not yet
/// in the cue joins it only when talking at once with someone in it ([`Open::at_once`]); otherwise
/// the cue closes, so plain turn-taking stays one speaker per cue as in 5a, and a stacked cue ends
/// when the last of the overlapping speakers stops. An overlap longer than the limits allow becomes
/// several stacked cues, each within them; a speaker still mid-word when one closes keeps their
/// last line on screen in the next ([`Open::after`]).
///
/// Before cutting, the kept words go through the text options ([`caption_text`], 5j): a word is
/// shown as they make it, and a replaced phrase is one unit, one word for the word limit. Before
/// those, with `c.translate` set, each sentence with a translation in `translated` is shown
/// translated ([`translated`], 5k); one without is shown as said, so the preview can show what is
/// translated so far, and a caller writing a file checks first that none is missing
/// ([`missing`]).
///
/// `c` is the caption settings to cut with, resolved by the caller: the list's own, else the
/// user's defaults (2i, `config::captions`). Taken apart from `edit`, whose `captions` may be
/// `None`, so a caller cannot forget the defaults and silently get the built-in ones.
pub fn cues<'a>(
    edit: &EditList,
    c: &Captions,
    words: impl IntoIterator<Item = (usize, &'a Word)>,
    num: u64,
    den: u64,
    translations: Option<&Translations>,
) -> Vec<Cue> {
    let tl = Timeline::new(edit, num, den);
    let label = edit.speakers.labeller();
    let kept = kept(&tl, words);
    let shown = match translations.filter(|_| !c.translate.is_empty()) {
        Some(t) => {
            let s = sentence_words(&kept, &tl, &label);
            translated::apply(kept, &s, |text| t.get(&c.translate, text), clean)
        }
        None => kept,
    };
    let texts: Vec<(usize, String)> = shown.iter().map(|w| (w.track, w.text.clone())).collect();

    let mut out: Vec<Cue> = Vec::new();
    let mut open: Option<Open> = None;
    for unit in caption_text::units(&texts, c) {
        let (first, last) = (&shown[unit.first], &shown[unit.last]);
        let text = unit.text;
        let said = Said {
            track: first.track,
            start: tl.map(first.start.0, first.start.1),
            end: tl.map(last.end.0, last.end.1),
            // Its speaker first, then anyone else whose overlap turn holds it (5b).
            speakers: label.speakers_of_word(first.track, first.start.1, last.end.1),
        };
        if open.as_mut().is_some_and(|o| o.take(&said, &text, c)) {
            continue;
        }
        open = Some(match open.take() {
            Some(o) => {
                let next = o.after(&said, text);
                out.push(o.close());
                next
            }
            None => Open::new(&said, text),
        });
    }
    out.extend(open.map(Open::close));
    let floor = if c.words == 0 { READABLE_S } else { 0.0 };
    settle(&mut out, c.seconds, floor, tl.total());
    out
}

/// The kept words, in time order, each in its kept range and cleaned (a word left empty is kept
/// here, so a phrase around it does not match, and dropped as a unit).
fn kept<'a>(tl: &Timeline, words: impl IntoIterator<Item = (usize, &'a Word)>) -> Vec<Shown> {
    let mut words: Vec<(usize, &Word)> = words.into_iter().collect();
    words.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
    words
        .into_iter()
        .filter_map(|(track, w)| {
            let r = tl.range_of((w.start + w.end) / 2.0)?;
            Some(Shown {
                track,
                start: (r, w.start),
                end: (r, w.end),
                text: clean(&w.text),
            })
        })
        .collect()
}

/// The sentences of the kept words ([`translated::sentences`]), cut at [`PAUSE_S`] like cues.
fn sentence_words(kept: &[Shown], tl: &Timeline, label: &Labeller) -> Vec<Vec<usize>> {
    translated::sentences(
        kept,
        |w| label.of_word(w.track, w.start.1, w.end.1).unwrap_or(0),
        |(r, t)| tl.map(r, t),
        PAUSE_S,
    )
}

/// The sentences captions would translate (5k): the kept words' sentences as said, each with its
/// track and start, which find the language it was said in. The words, frame rate and edit are as
/// for [`cues`]; the caption settings play no part, so changing them never asks for a
/// translation again, while a cut that changes a sentence's words does.
pub fn sentences<'a>(
    edit: &EditList,
    words: impl IntoIterator<Item = (usize, &'a Word)>,
    num: u64,
    den: u64,
) -> Vec<Sentence> {
    let tl = Timeline::new(edit, num, den);
    let label = edit.speakers.labeller();
    let kept = kept(&tl, words);
    sentence_words(&kept, &tl, &label)
        .iter()
        .map(|s| Sentence {
            track: kept[s[0]].track,
            start: kept[s[0]].start.1,
            text: translated::text(&kept, s),
        })
        .filter(|s| !s.text.is_empty())
        .collect()
}

/// How many of `sentences` have no translation into `c.translate` yet; 0 when captions are not
/// translated.
pub fn missing(c: &Captions, sentences: &[Sentence], translations: &Translations) -> usize {
    if c.translate.is_empty() {
        return 0;
    }
    translations
        .missing(&c.translate, sentences.iter().map(|s| s.text.as_str()))
        .len()
}

/// A kept word on the output timeline.
struct Said {
    track: usize,
    start: f64,
    end: f64,
    /// Everyone saying it, its one speaker first; empty for nobody (speaker 0).
    speakers: Vec<u32>,
}

impl Said {
    fn speaker(&self) -> u32 {
        self.speakers.first().copied().unwrap_or(0)
    }

    /// The others an overlap stretch says are talking with its speaker.
    fn others(&self) -> &[u32] {
        self.speakers.get(1..).unwrap_or(&[])
    }
}

/// A speaker's part of the open cue; its lines are wrapped as words arrive, so a word costs O(1).
struct OpenPart {
    speaker: u32,
    lines: Vec<String>,
    /// Units taken (5i's word limit), one per word or replaced phrase (5j), whatever spaces its
    /// text holds; `last_words` of them on the last line.
    words: u32,
    last_words: u32,
    /// The track and end of its last word.
    track: usize,
    end: f64,
    /// Other speakers heard at once with any of its words (5b's overlap stretches).
    with: Vec<u32>,
}

impl OpenPart {
    fn new(w: &Said, text: String) -> Self {
        let mut p = Self {
            speaker: w.speaker(),
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
struct Open {
    start: f64,
    end: f64,
    parts: Vec<OpenPart>,
}

impl Open {
    fn new(w: &Said, text: String) -> Self {
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
    fn after(&self, w: &Said, text: String) -> Open {
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
    fn take(&mut self, w: &Said, text: &str, c: &Captions) -> bool {
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

    fn close(self) -> Cue {
        Cue {
            start: self.start,
            end: self.end,
            parts: self
                .parts
                .into_iter()
                .map(|p| Part {
                    speaker: p.speaker,
                    lines: p.lines,
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

/// `lines` with `w` added by the greedy wrap.
fn wrap_onto(mut lines: Vec<String>, w: &str, chars: usize) -> Vec<String> {
    match lines.last_mut() {
        Some(l) if width(l) + 1 + width(w) <= chars => {
            l.push(' ');
            l.push_str(w);
        }
        _ => lines.push(w.to_string()),
    }
    lines
}

/// Cues in order and never overlapping (Resolve puts subtitles on one track, which cannot hold
/// two at once), all within the edited timeline's `total` seconds. Each cue lasts at least
/// `floor` ([`READABLE_S`], or nothing under a word limit): a next cue starting sooner (a quick
/// turn) waits that long. Then each ends by the next one's start, held to [`MIN_S`] where there is
/// room, never past the longest a cue may last. A cue pushed past the end of the timeline joins the
/// one before. Under a word limit nothing is pushed, and cues said at one instant share its time
/// first ([`share_instants`]), so each keeps its words within the limit.
fn settle(cues: &mut Vec<Cue>, longest: f64, floor: f64, total: f64) {
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
    // before it rather than being lost (a part per speaker, as in any cue). One with no cue
    // before it goes into the next, ahead of its words: 5i dropped it, words and all (5p).
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
            Some(p) => p.lines.extend(part.lines),
            None => parts.push(part),
        }
    }
}

/// Cues (in order) said at one instant, spread over it in turn, under a word limit (5p).
///
/// A cue the word limit closed is followed by one starting at the same instant when two units of
/// one speaker start together: Whisper gives a word no length now and then, and a long
/// translation (5k) puts several words where a spoken word ends. Cues cannot overlap, so the
/// first had no time: 5i left it to join the cue before, past the limit, or dropped it as the
/// first. Now the cues of one instant split, evenly and in turn, the time their words take (the
/// minimum hold when none has a length), never past the next cue's start nor the end of the
/// timeline, so every cue keeps its own time and nothing trails the voice: the next cue starts
/// where it did. Two speakers at once are not this case; they share one cue, a part each.
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

/// `HH:MM:SS,mmm`.
fn stamp(s: f64) -> String {
    let ms = (s.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// The SRT text: CRLF line ends (what Windows tools and Resolve's own export write), each line in
/// `<font color>` of its part's speaker; a word nobody was heard saying is left white. A cue's parts
/// follow one another line by line, so a change of speaker is a line break: no blank line between
/// them, which would end the cue.
///
/// Bold and italic (5d) and underline (5f) wrap every line outside its colour,
/// `<b><i><u><font …>line</font></u></i></b>`: the order Resolve's own SRT export writes (`<b>`
/// around the whole text), `<u>` innermost so 5d's form is unchanged without it (Uezar's import
/// found the order makes no difference), each line closed on its own like the colour. Resolve's
/// import applies all three (5d's research, Uezar's underline test); nothing else is written,
/// since the rest of what Resolve styles is its subtitle Track Style, which an SRT cannot set.
/// `c` is resolved by the caller, as for [`cues`].
pub fn write(cues: &[Cue], edit: &EditList, c: &Captions) -> String {
    let tags: Vec<&str> = [("b", c.bold), ("i", c.italic), ("u", c.underline)]
        .into_iter()
        .filter_map(|(t, on)| on.then_some(t))
        .collect();
    let open: String = tags.iter().map(|t| format!("<{t}>")).collect();
    let close: String = tags.iter().rev().map(|t| format!("</{t}>")).collect();
    let mut out = String::new();
    for (n, c) in cues.iter().enumerate() {
        let _ = write!(
            out,
            "{}\r\n{} --> {}\r\n",
            n + 1,
            stamp(c.start),
            stamp(c.end)
        );
        for part in &c.parts {
            let color = edit.speakers.color(part.speaker);
            for line in &part.lines {
                match color {
                    Some(col) => {
                        let _ = write!(out, "{open}<font color=\"{col}\">{line}</font>{close}\r\n");
                    }
                    None => {
                        let _ = write!(out, "{open}{line}{close}\r\n");
                    }
                }
            }
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{CutSettings, Range, Speakers, Turn};

    fn word(start: f64, end: f64, text: &str) -> Word {
        Word {
            start,
            end,
            text: text.into(),
            p: 1.0,
        }
    }

    /// Each part of a cue as `(speaker, lines joined by |)`.
    fn parts(c: &Cue) -> Vec<(u32, String)> {
        c.parts
            .iter()
            .map(|p| (p.speaker, p.lines.join("|")))
            .collect()
    }

    /// One speaker's text, one line per cue.
    fn solo(c: &[Cue]) -> Vec<String> {
        c.iter()
            .map(|c| {
                assert_eq!(c.parts.len(), 1, "{c:?}");
                c.parts[0].lines.join("|")
            })
            .collect()
    }

    /// The test list's own caption settings, which these tests always set.
    fn own(e: &EditList) -> Captions {
        e.captions
            .clone()
            .expect("the test lists have their own captions")
    }

    fn edit(keep: &[(f64, f64)], captions: Captions) -> EditList {
        EditList {
            source: "rec.mp4".into(),
            duration: 100.0,
            settings: CutSettings::default(),
            keep: keep
                .iter()
                .map(|&(start, end)| Range { start, end })
                .collect(),
            speakers: Speakers::new(
                vec![
                    Turn {
                        start: 0.0,
                        end: 20.0,
                        speaker: 1,
                    },
                    Turn {
                        start: 20.0,
                        end: 40.0,
                        speaker: 2,
                    },
                ],
                &[None, None],
                None,
            ),
            captions: Some(captions),
            windows: vec![],
        }
    }

    #[test]
    fn cues_are_on_the_edited_timeline_and_skip_cut_words() {
        // Kept 10-12 and 30-33 at 30 fps: output 0-2 then 2-5.
        let e = edit(&[(10.0, 12.0), (30.0, 33.0)], Captions::default());
        let w = [
            word(5.0, 5.5, "cut"),
            word(10.5, 11.0, "hola"),
            word(11.2, 11.6, "amigos"),
            word(30.2, 30.6, "qué"),
            word(30.7, 31.0, "tal"),
        ];
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(parts(&c[0]), vec![(1, "hola amigos".into())]);
        assert!((c[0].start - 0.5).abs() < 1e-9);
        assert_eq!(parts(&c[1]), vec![(2, "qué tal".into())]);
        assert!((c[1].start - 2.2).abs() < 1e-9, "{}", c[1].start);
        let text = write(&c, &e, &own(&e));
        let first = "1\r\n00:00:00,500 --> 00:00:01,600\r\n\
                     <font color=\"#F2B33D\">hola amigos</font>\r\n\r\n2\r\n00:00:02,200 --> ";
        assert!(text.starts_with(first), "{text}");
        assert!(text.contains("<font color=\"#5FB3F2\">qué tal</font>"));
    }

    #[test]
    fn frame_rounding_does_not_drift_across_many_cuts() {
        // 200 kept ranges of 0.51 s at 30 fps: each is 16 frames once rounded (start down, end
        // up), so range k starts at output frame 16k. Seconds alone would put it at 0.51k.
        let keep: Vec<(f64, f64)> = (0..200)
            .map(|k| (k as f64 * 0.9 + 0.01, k as f64 * 0.9 + 0.52))
            .collect();
        let mut e = edit(&keep, Captions::default());
        e.duration = 200.0;
        let last = word(199.0 * 0.9 + 0.1, 199.0 * 0.9 + 0.3, "fin");
        let c = cues(&e, &own(&e), [(0, &last)], 30, 1, None);
        let frames = frame_ranges(&e.keep, 30, 1, frames_ceil(200.0, 30, 1));
        let before: u64 = frames[..199].iter().map(|(a, b)| b - a).sum();
        let (a, _) = frames[199];
        let expect = (before as f64 + (last.start * 30.0 - a as f64)) / 30.0;
        assert!(
            (c[0].start - expect).abs() < 1e-9,
            "{} vs {expect}",
            c[0].start
        );
    }

    #[test]
    fn cues_break_at_speakers_pauses_length_and_lines() {
        let one_line = Captions {
            chars: 12,
            lines: 1,
            seconds: 3.0,
            ..Default::default()
        };
        let e = edit(&[(0.0, 40.0)], one_line);
        let w = [
            word(1.0, 1.2, "uno"),
            word(1.3, 1.5, "dos"),
            word(1.6, 1.8, "tres"),   // "uno dos tres" is 12: fits
            word(1.9, 2.1, "cuatro"), // would make a 2nd line: new cue
            word(5.0, 5.2, "cinco"),  // after a 2.9 s pause: new cue
            word(5.3, 5.5, "seis"),
            word(8.0, 8.2, "siete"), // after a 2.5 s pause: new cue
            word(19.0, 19.5, "a"),
            word(20.5, 20.8, "b"), // speaker 2: new cue
        ];
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None);
        assert_eq!(
            solo(&c),
            vec!["uno dos tres", "cuatro", "cinco seis", "siete", "a", "b"]
        );
        // Long talk without pauses splits at the longest cue length.
        let e = edit(
            &[(0.0, 40.0)],
            Captions {
                chars: 80,
                lines: 3,
                seconds: 2.0,
                ..Default::default()
            },
        );
        let w: Vec<Word> = (0..20)
            .map(|k| word(k as f64 * 0.3, k as f64 * 0.3 + 0.25, "la"))
            .collect();
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None);
        assert!(c.len() >= 3);
        assert!(c.iter().all(|c| c.end - c.start <= 2.0 + 1e-9), "{c:?}");
    }

    #[test]
    fn a_word_limit_cuts_cues_and_one_word_at_a_time_keeps_up() {
        // A word every 0.3 s, quicker than the 0.5 s floor of an unlimited caption.
        let w: Vec<Word> = ["a", "b", "c", "d", "e", "f", "g"]
            .iter()
            .enumerate()
            .map(|(k, t)| word(k as f64 * 0.3, k as f64 * 0.3 + 0.25, t))
            .collect();
        let with = |words: u32| {
            let e = edit(
                &[(0.0, 40.0)],
                Captions {
                    words,
                    ..Default::default()
                },
            );
            cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None)
        };
        // No limit: as before 5i, one cue.
        assert_eq!(solo(&with(0)), vec!["a b c d e f g"]);
        assert_eq!(solo(&with(3)), vec!["a b c", "d e f", "g"]);
        // One at a time: each word its own cue, starting when it is said, not pushed behind.
        let one = with(1);
        assert_eq!(solo(&one), vec!["a", "b", "c", "d", "e", "f", "g"]);
        for (c, w) in one.iter().zip(&w) {
            assert!((c.start - w.start).abs() < 1e-9, "{one:?}");
        }
        for p in one.windows(2) {
            assert!(p[0].end <= p[1].start + 1e-9, "{one:?}");
        }
    }

    #[test]
    fn translated_captions_are_cut_and_styled_as_the_translation() {
        use crate::edit::{Case, Replace};
        // Two sentences, the second after a cut (5..10 s cut out).
        let w = [
            word(0.0, 0.4, "Hola,"),
            word(0.4, 0.8, "¿qué"),
            word(0.8, 1.2, "tal?"),
            word(10.5, 11.0, "Muy"),
            word(11.0, 11.5, "bien."),
        ];
        let c = Captions {
            translate: "en".into(),
            ..Default::default()
        };
        let e = edit(&[(0.0, 5.0), (10.0, 40.0)], c.clone());
        let words = || w.iter().map(|w| (0, w));
        let s = sentences(&e, words(), 30, 1);
        let texts: Vec<&str> = s.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Hola, ¿qué tal?", "Muy bien."]);
        assert_eq!((s[1].track, s[1].start), (0, 10.5));
        let mut t = Translations::default();
        t.insert("en", "Hola, ¿qué tal?", "Hi, how are you doing?");
        assert_eq!(missing(&c, &s, &t), 1);
        assert_eq!(missing(&Captions::default(), &s, &t), 0, "not translated");
        // A sentence not translated yet is shown as said.
        let got = cues(&e, &c, words(), 30, 1, Some(&t));
        assert_eq!(solo(&got), ["Hi, how are you doing?", "Muy bien."]);
        assert!((got[0].start, got[1].start) == (0.0, 5.5), "{got:?}");
        assert!(
            (got[0].end - 1.2).abs() < 1e-9,
            "the first ends with its sentence"
        );
        t.insert("en", "Muy bien.", "Very good.");
        assert_eq!(missing(&c, &s, &t), 0);
        // Not asked for (no setting, or no translations given): the words as said.
        let plain = solo(&cues(&e, &Captions::default(), words(), 30, 1, Some(&t)));
        assert_eq!(plain, ["Hola, ¿qué tal?", "Muy bien."]);
        assert_eq!(solo(&cues(&e, &c, words(), 30, 1, None)), plain);
        // The text options and the word limit apply to the translated words.
        let styled = Captions {
            case: Case::Upper,
            words: 2,
            replace: vec![Replace {
                from: "how are you".into(),
                to: "sup".into(),
            }],
            ..c.clone()
        };
        assert_eq!(
            solo(&cues(&e, &styled, words(), 30, 1, Some(&t))),
            ["HI, SUP", "DOING?", "VERY GOOD."]
        );
    }

    #[test]
    fn text_options_change_what_is_wrapped_and_a_phrase_is_one_word() {
        use crate::edit::{Case, Replace};
        let pair = |from: &str, to: &str| Replace {
            from: from.into(),
            to: to.into(),
        };
        let w: Vec<Word> = ["you", "know,", "gonna", "win", "in", "new", "york"]
            .iter()
            .enumerate()
            .map(|(k, t)| word(k as f64 * 0.3, k as f64 * 0.3 + 0.25, t))
            .collect();
        let with = |c: Captions| {
            let e = edit(&[(0.0, 40.0)], c);
            solo(&cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None))
        };
        let short = Captions {
            chars: 12,
            lines: 1,
            ..Default::default()
        };
        assert_eq!(
            with(short.clone()),
            vec!["you know,", "gonna win in", "new york"]
        );
        // The replacement is what gets wrapped: "GOING TO" no longer fits after "YOU KNOW".
        let c = Captions {
            case: Case::Upper,
            punctuation: false,
            replace: vec![pair("gonna", "going to"), pair("new york", "NYC")],
            ..short
        };
        assert_eq!(with(c.clone()), vec!["YOU KNOW", "GOING TO WIN", "IN NYC"]);
        // One word at a time: the phrase is one caption, from when its first word is said.
        let one = {
            let e = edit(&[(0.0, 40.0)], Captions { words: 1, ..c });
            cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None)
        };
        assert_eq!(solo(&one).last().map(String::as_str), Some("NYC"));
        assert!((one.last().unwrap().start - 1.5).abs() < 1e-9, "{one:?}");
    }

    #[test]
    fn the_word_limit_holds_per_speaker_talking_at_once() {
        let e = two_tracks(Captions {
            words: 2,
            ..Default::default()
        });
        let one = [
            word(0.0, 0.3, "uno"),
            word(0.4, 0.7, "dos"),
            word(0.8, 1.1, "tres"), // a third word for speaker 1: a new cue
        ];
        // "sí" runs past the new cue's start, so it is carried on and counts as one of two.
        let two = [word(0.5, 0.9, "sí"), word(0.95, 1.2, "no")];
        let words = one.iter().map(|w| (0, w)).chain(two.iter().map(|w| (3, w)));
        let got: Vec<_> = cues(&e, &own(&e), words, 30, 1, None)
            .iter()
            .map(parts)
            .collect();
        assert_eq!(
            got,
            vec![
                vec![(1, "uno dos".into()), (2, "sí".into())],
                vec![(1, "tres".into()), (2, "sí no".into())],
            ]
        );
    }

    #[test]
    fn one_word_at_a_time_never_falls_behind_quick_words() {
        // 100 words 0.1 s apart (short words with touching edges): a floor pushing each cue
        // after the last would put the hundredth seconds behind its word.
        let w: Vec<Word> = (0..100)
            .map(|k| word(k as f64 * 0.1, k as f64 * 0.1 + 0.1, "la"))
            .collect();
        let e = edit(
            &[(0.0, 40.0)],
            Captions {
                words: 1,
                ..Default::default()
            },
        );
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None);
        assert_eq!(c.len(), 100, "{c:?}");
        for (c, w) in c.iter().zip(&w) {
            assert!((c.start - w.start).abs() < 1e-9, "{c:?} for {w:?}");
            assert!(c.end > c.start);
        }
    }

    #[test]
    fn under_a_word_limit_the_last_words_keep_their_own_cues() {
        // Kept 0-10 s. Without a limit "b" would wait for "a" past the end and join it; one word
        // at a time it starts when said, so the limit holds to the end.
        let e = edit(
            &[(0.0, 10.0)],
            Captions {
                words: 1,
                ..Default::default()
            },
        );
        let (a, b) = (word(9.90, 9.94, "a"), word(9.95, 9.99, "b"));
        let c = cues(&e, &own(&e), [(0, &a), (0, &b)], 30, 1, None);
        assert_eq!(solo(&c), vec!["a", "b"]);
        assert!(
            (c[1].start - 9.95).abs() < 1e-9 && (c[1].end - 10.0).abs() < 1e-9,
            "{c:?}"
        );
    }

    #[test]
    fn two_speakers_starting_a_word_at_once_share_one_cue_within_the_limit() {
        // 5p's report: two tracks, both 0.0-0.5 s, one word at a time. The second speaker has no
        // part yet and talks at once, so it joins the cue as its own part: no second cue at the
        // same instant, and each part holds one word.
        let e = two_tracks(Captions {
            words: 1,
            ..Default::default()
        });
        let (a, b) = (word(0.0, 0.5, "uno"), word(0.0, 0.5, "dos"));
        let c = cues(&e, &own(&e), [(0, &a), (3, &b)], 30, 1, None);
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(parts(&c[0]), vec![(1, "uno".into()), (2, "dos".into())]);
    }

    #[test]
    fn under_a_word_limit_words_said_at_one_instant_share_its_time() {
        // Whisper gives a word no length now and then ("a" at 1.0-1.0), so the next starts at the
        // same instant. Each keeps its own cue, in turn, within the limit: first in the
        // recording, where it used to be dropped, and after another, where it used to join it.
        let e = edit(
            &[(0.0, 40.0)],
            Captions {
                words: 1,
                ..Default::default()
            },
        );
        let (x, a, b, z) = (
            word(0.0, 0.3, "x"),
            word(1.0, 1.0, "a"),
            word(1.0, 1.3, "b"),
            word(3.0, 3.3, "z"),
        );
        for (words, want) in [
            (vec![&a, &b, &z], vec!["a", "b", "z"]),
            (vec![&x, &a, &b, &z], vec!["x", "a", "b", "z"]),
        ] {
            let c = cues(&e, &own(&e), words.into_iter().map(|w| (0, w)), 30, 1, None);
            assert_eq!(solo(&c), want);
            let k = want.len() - 3;
            assert!((c[k].start - 1.0).abs() < 1e-9, "{c:?}");
            // "a" and "b" split the time until "b" ends; "b" is then held to the minimum.
            assert!((c[k + 1].start - 1.15).abs() < 1e-9, "{c:?}");
            assert!((c[k + 1].end - (1.15 + MIN_S)).abs() < 1e-9, "{c:?}");
            for p in c.windows(2) {
                assert!(
                    p[0].start < p[0].end && p[0].end <= p[1].start + 1e-9,
                    "{c:?}"
                );
            }
        }
    }

    #[test]
    fn a_long_translation_keeps_the_word_limit_where_its_words_bunch_up() {
        // Sixteen translated words over two spoken ones: two of them start in the space after
        // "Sí", so at its end, the same instant. One word at a time, every cue holds one word.
        let w = [word(0.0, 0.4, "Sí"), word(0.4, 1.0, "bien.")];
        let c = Captions {
            translate: "en".into(),
            words: 1,
            ..Default::default()
        };
        let e = edit(&[(0.0, 40.0)], c.clone());
        let mut t = Translations::default();
        let many = "a b c d e f g h i j k l m n o p";
        t.insert("en", "Sí bien.", many);
        let got = cues(&e, &c, w.iter().map(|w| (0, w)), 30, 1, Some(&t));
        assert_eq!(solo(&got), many.split(' ').collect::<Vec<_>>());
        for p in got.windows(2) {
            assert!(
                p[0].start < p[0].end && p[0].end <= p[1].start + 1e-9,
                "{got:?}"
            );
        }
    }

    #[test]
    fn a_carried_line_counts_a_replaced_phrase_as_one_word() {
        use crate::edit::Replace;
        let e = two_tracks(Captions {
            words: 2,
            replace: vec![Replace {
                from: "new york".into(),
                to: "big apple".into(),
            }],
            ..Default::default()
        });
        let one = [
            word(0.0, 0.3, "uno"),
            word(0.35, 0.6, "dos"),
            word(0.8, 1.1, "tres"), // a third word for speaker 1: a new cue
        ];
        // "big apple" runs past the new cue's start: carried on as one word of two, so "ya" fits.
        let two = [
            word(0.5, 0.65, "new"),
            word(0.7, 0.95, "york"),
            word(1.0, 1.2, "ya"),
        ];
        let words = one.iter().map(|w| (0, w)).chain(two.iter().map(|w| (3, w)));
        let got: Vec<_> = cues(&e, &own(&e), words, 30, 1, None)
            .iter()
            .map(parts)
            .collect();
        assert_eq!(
            got,
            vec![
                vec![(1, "uno dos".into()), (2, "big apple".into())],
                vec![(1, "tres".into()), (2, "big apple ya".into())],
            ]
        );
    }

    #[test]
    fn a_quick_turn_still_gets_readable_time() {
        // Speaker 1 to 20 s, speaker 2 from 20 s (`edit`), one mixed track: "no" starts 0.1 s after
        // "sí", one speaker after the other, not at once.
        let e = edit(&[(0.0, 40.0)], Captions::default());
        let (a, b) = (word(19.9, 20.0, "sí"), word(20.0, 20.2, "no"));
        let c = cues(&e, &own(&e), [(0, &a), (0, &b)], 30, 1, None);
        assert_eq!(solo(&c), vec!["sí", "no"]);
        assert!((c[0].end - c[0].start - READABLE_S).abs() < 1e-9, "{c:?}");
        assert!((c[1].start - c[0].end).abs() < 1e-9);
        assert!(c[1].end - c[1].start >= READABLE_S, "{c:?}");
    }

    #[test]
    fn the_last_cue_ends_with_the_edited_timeline() {
        // Kept 0-10 s; a last word at 9.8-9.9 would be held to 10.6.
        let e = edit(&[(0.0, 10.0)], Captions::default());
        let w = word(9.8, 9.9, "fin");
        let c = cues(&e, &own(&e), [(0, &w)], 30, 1, None);
        assert!((c[0].end - 10.0).abs() < 1e-9, "{c:?}");
    }

    #[test]
    fn cues_never_overlap_and_short_ones_are_held() {
        let e = edit(&[(0.0, 40.0)], Captions::default());
        // Speaker 1 to 19.9, speaker 2 from 20.0 on another track at once.
        let w = [
            word(19.5, 19.9, "sí"),
            word(20.0, 20.4, "no"),
            word(25.0, 25.1, "ya"),
        ];
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None);
        assert_eq!(c.len(), 3);
        for p in c.windows(2) {
            assert!(p[0].end <= p[1].start + 1e-9, "{c:?}");
        }
        assert!((c[0].end - 20.0).abs() < 1e-9, "held only up to the next");
        assert!(
            (c[2].end - 25.8).abs() < 1e-9,
            "a lone word is held to the minimum"
        );
    }

    /// Speakers by track: track 0 is speaker 1, track 3 speaker 2.
    fn two_tracks(captions: Captions) -> EditList {
        let mut e = edit(&[(0.0, 40.0)], captions);
        e.speakers = Speakers::new(vec![], &[Some(0), Some(3)], None);
        e
    }

    #[test]
    fn tracks_talking_at_once_share_one_cue_a_part_each() {
        let e = two_tracks(Captions::default());
        let one = [
            word(10.0, 10.4, "hola"),
            word(10.5, 10.8, "qué"),
            word(10.9, 11.2, "tal"),
            word(11.3, 11.6, "bien"),
        ];
        // "sí" starts before "qué" ends: at once. "vale" comes after a pause of speaker 2's own.
        let two = [word(10.6, 10.9, "sí"), word(12.0, 12.3, "vale")];
        let words = one.iter().map(|w| (0, w)).chain(two.iter().map(|w| (3, w)));
        let c = cues(&e, &own(&e), words, 30, 1, None);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(
            parts(&c[0]),
            vec![(1, "hola qué tal bien".into()), (2, "sí".into())]
        );
        // The stacked cue lasts until the last of them stops.
        assert!((c[0].start - 10.0).abs() < 1e-9 && (c[0].end - 11.6).abs() < 1e-9);
        assert_eq!(parts(&c[1]), vec![(2, "vale".into())]);
        // Each part in its own colour, a line break between them and no blank line.
        let text = write(&c, &e, &own(&e));
        assert!(
            text.contains(
                "<font color=\"#F2B33D\">hola qué tal bien</font>\r\n\
                 <font color=\"#5FB3F2\">sí</font>\r\n\r\n"
            ),
            "{text}"
        );
        // One after the other on two tracks is not at once: a cue each.
        let (a, b) = (word(20.0, 20.4, "ya"), word(20.5, 20.8, "no"));
        let c = cues(&e, &own(&e), [(0, &a), (3, &b)], 30, 1, None);
        assert_eq!(c.len(), 2, "{c:?}");
    }

    #[test]
    fn line_limits_hold_per_speaker_and_a_speaker_mid_word_stays_on_screen() {
        let e = two_tracks(Captions {
            chars: 12,
            lines: 1,
            seconds: 7.0,
            ..Default::default()
        });
        let one = [
            word(0.0, 0.3, "uno"),
            word(0.4, 0.7, "dos"),
            word(0.8, 1.1, "tres"),
            word(1.2, 1.5, "cuatro"), // a second line for speaker 1: a new cue
        ];
        // "sí" runs to 1.3 s, past the new cue's start at 1.2 s.
        let two = [word(0.5, 1.3, "sí"), word(1.4, 1.7, "claro")];
        let words = one.iter().map(|w| (0, w)).chain(two.iter().map(|w| (3, w)));
        let c = cues(&e, &own(&e), words, 30, 1, None);
        let got: Vec<_> = c.iter().map(parts).collect();
        assert_eq!(
            got,
            vec![
                vec![(1, "uno dos tres".into()), (2, "sí".into())],
                // Speaker 2's line is carried on, and their next word goes after it.
                vec![(1, "cuatro".into()), (2, "sí claro".into())],
            ]
        );
        for p in c.windows(2) {
            assert!(p[0].end <= p[1].start + 1e-9, "{c:?}");
        }
        // Speaker 2 is on screen without a gap from "sí" until "claro" ends.
        assert!((c[0].end - c[1].start).abs() < 1e-9, "{c:?}");
        assert!(c[1].end >= 1.7 - 1e-9, "{c:?}");
    }

    #[test]
    fn a_long_overlap_splits_at_the_longest_cue_both_speakers_kept() {
        let e = two_tracks(Captions {
            chars: 80,
            lines: 3,
            seconds: 2.0,
            ..Default::default()
        });
        // Both talk without a pause for 4.5 s, their words interleaved.
        let one: Vec<Word> = (0..15)
            .map(|k| word(k as f64 * 0.3, k as f64 * 0.3 + 0.25, "la"))
            .collect();
        let two: Vec<Word> = (0..15)
            .map(|k| word(k as f64 * 0.3 + 0.15, k as f64 * 0.3 + 0.4, "sí"))
            .collect();
        let words = one.iter().map(|w| (0, w)).chain(two.iter().map(|w| (3, w)));
        let c = cues(&e, &own(&e), words, 30, 1, None);
        assert!(c.len() >= 3, "{c:?}");
        for cue in &c {
            assert!(cue.end - cue.start <= 2.0 + 1e-9, "{c:?}");
            let who: Vec<u32> = cue.parts.iter().map(|p| p.speaker).collect();
            assert_eq!(who, vec![1, 2], "{c:?}");
        }
        for p in c.windows(2) {
            assert!(p[0].end <= p[1].start + 1e-9, "{c:?}");
        }
        // The last cue lasts until the last word ends.
        assert!((c.last().unwrap().end - 4.6).abs() < 1e-9, "{c:?}");
    }

    #[test]
    fn a_cue_pushed_past_the_end_joins_the_one_before() {
        // Kept 0-10 s. "b" follows "a" on another track, not at once: its own cue, which waits
        // for "a" to be readable and so would start at 10.1 s, past the end.
        let mut e = two_tracks(Captions::default());
        e.keep = vec![Range {
            start: 0.0,
            end: 10.0,
        }];
        let (a, b) = (word(9.6, 9.7, "a"), word(9.75, 9.9, "b"));
        let c = cues(&e, &own(&e), [(0, &a), (3, &b)], 30, 1, None);
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(parts(&c[0]), vec![(1, "a".into()), (2, "b".into())]);
        assert!((c[0].end - 10.0).abs() < 1e-9, "{c:?}");
    }

    #[test]
    fn an_overlap_stretch_on_one_track_stacks_both_speakers() {
        // Speaker 1 talks 0-20 and speaker 2 20-40 (`edit`), and speaker 2 also 5-7 over speaker 1.
        let mut e = edit(&[(0.0, 40.0)], Captions::default());
        let mut turns = e.speakers.turns.clone();
        turns.push(Turn {
            start: 5.0,
            end: 7.0,
            speaker: 2,
        });
        e.speakers = Speakers::new(turns, &[None, None], None);
        let w = [
            word(1.0, 1.3, "hola"),
            word(4.6, 4.95, "pero"), // speaker 1's, before the stretch
            word(5.1, 5.4, "sí"),    // in the stretch: speaker 2's, said with speaker 1
            word(5.45, 5.8, "mira"),
            word(8.0, 8.3, "vale"), // speaker 1 alone again, after a pause
        ];
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None);
        let got: Vec<_> = c.iter().map(parts).collect();
        assert_eq!(
            got,
            vec![
                vec![(1, "hola".into())],
                vec![(1, "pero".into()), (2, "sí mira".into())],
                vec![(1, "vale".into())],
            ]
        );
        // Every word once.
        let all: usize = c
            .iter()
            .flat_map(|c| &c.parts)
            .flat_map(|p| &p.lines)
            .map(|l| l.split_whitespace().count())
            .sum();
        assert_eq!(all, w.len());
    }

    #[test]
    fn odd_text_and_unlabelled_words() {
        let mut e = edit(&[(0.0, 100.0)], Captions::default());
        e.speakers = Speakers::default();
        let w = [
            word(1.0, 1.2, " <b>hey</b> "),
            word(1.3, 1.4, "{\\an8}vale"),
            word(1.5, 1.6, "   "),
            word(1.7, 1.8, "dos\r\nlíneas"),
            word(1.9, 2.0, "a<b"),
        ];
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None);
        assert_eq!(parts(&c[0]), vec![(0, "hey vale dos líneas ab".into())]);
        let text = write(&c, &e, &own(&e));
        assert!(!text.contains("<font"), "no speaker, no colour");
        assert!(cues(&e, &own(&e), [], 30, 1, None).is_empty());
        assert_eq!(stamp(3723.4567), "01:02:03,457");
    }

    #[test]
    fn bold_and_italic_wrap_each_line_outside_its_colour() {
        let w = [word(1.0, 1.4, "hola"), word(21.0, 21.4, "sí")];
        let styled = |bold, italic| {
            let e = edit(
                &[(0.0, 40.0)],
                Captions {
                    bold,
                    italic,
                    ..Default::default()
                },
            );
            write(
                &cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None),
                &e,
                &own(&e),
            )
        };
        let both = styled(true, true);
        assert!(
            both.contains("<b><i><font color=\"#F2B33D\">hola</font></i></b>\r\n"),
            "{both}"
        );
        assert!(both.contains("<b><i><font color=\"#5FB3F2\">sí</font></i></b>\r\n"));
        assert!(styled(true, false).contains("<b><font color=\"#F2B33D\">hola</font></b>\r\n"));
        assert!(styled(false, true).contains("<i><font color=\"#F2B33D\">hola</font></i>\r\n"));
        let plain = styled(false, false);
        assert!(!plain.contains("<b>") && !plain.contains("<i>"));
        assert!(plain.contains("\r\n<font color=\"#F2B33D\">hola</font>\r\n"));
        // A word nobody was heard saying keeps the style without a colour.
        let mut e = edit(
            &[(0.0, 100.0)],
            Captions {
                italic: true,
                ..Default::default()
            },
        );
        e.speakers = Speakers::default();
        let text = write(
            &cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None),
            &e,
            &own(&e),
        );
        assert!(text.contains("\r\n<i>hola</i>\r\n"), "{text}");
        assert!(!text.contains("<font"));
    }

    #[test]
    fn underline_wraps_inside_bold_and_italic_outside_the_colour() {
        let w = [word(1.0, 1.4, "hola")];
        let styled = |c: Captions, speakers: bool| {
            let mut e = edit(&[(0.0, 40.0)], c);
            if !speakers {
                e.speakers = Speakers::default();
            }
            write(
                &cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1, None),
                &e,
                &own(&e),
            )
        };
        let all = styled(
            Captions {
                bold: true,
                italic: true,
                underline: true,
                ..Default::default()
            },
            true,
        );
        assert!(
            all.contains("<b><i><u><font color=\"#F2B33D\">hola</font></u></i></b>\r\n"),
            "{all}"
        );
        let only = Captions {
            underline: true,
            ..Default::default()
        };
        assert!(styled(only.clone(), true)
            .contains("\r\n<u><font color=\"#F2B33D\">hola</font></u>\r\n"));
        assert!(styled(only, false).contains("\r\n<u>hola</u>\r\n"));
        let bold = Captions {
            bold: true,
            underline: true,
            ..Default::default()
        };
        assert!(
            styled(bold, true).contains("<b><u><font color=\"#F2B33D\">hola</font></u></b>\r\n")
        );
    }
}
