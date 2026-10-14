//! Captions as an SRT file on the edited timeline, each cue in its speaker's colour: SRT is the
//! format whose per-cue colour comes through into DaVinci Resolve. A Resolve subtitle track shows
//! one caption at a time, so cues never overlap and people talking at once share a cue, a [`Part`]
//! each. Times go through the kept ranges on the frame grid ([`frame_ranges`]), as the renderer and
//! the FCPXML do, so a cue lands on the same frames in the render and in Resolve.

use std::fmt::Write;

use serde::Serialize;

use super::caption_text;
use super::translated::{self, Shown};
use crate::edit::{frame_ranges, frames_ceil, Captions, EditList, Fixed, Fixes, Labeller, Times};
use crate::transcribe::Word;
use crate::translate::{Sentence, Translations};

/// A new cue after a pause this long in the edited timeline, even mid-sentence.
const PAUSE_S: f64 = 1.0;
/// A cue stays at least this long when the next one leaves room, so a lone short word is not
/// gone before it is read.
const MIN_S: f64 = 0.8;
/// No cue is shorter than this; when the next would start sooner, it waits. Shorter than [`MIN_S`]
/// so a quick exchange does not drift behind. Not applied under a word limit, where the waits would
/// add up until the captions trail the voice by seconds.
const READABLE_S: f64 = 0.5;

/// One speaker's lines in a cue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Part {
    /// Speaker id, 0 for none.
    pub speaker: u32,
    pub lines: Vec<String>,
    /// What the lines say, unit by unit in order, with when each was said (for the word highlight
    /// in Resolve and the preview). The SRT does not use them.
    #[serde(skip)]
    pub words: Vec<Spoken>,
}

/// A unit of a caption (a word, or a phrase find and replace made one) and when it was said, in
/// output seconds: the words' own times, translated words spread over their sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Spoken {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// A caption: one part per speaker talking in it, in the order they started, so more than one only
/// where people talk at once. Serialized for the app's preview over the picture.
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
    crate::text::one_spaced(&out)
}

/// The cues for `edit`'s kept words. `words` is every transcript word with its track, in any
/// order; `num/den` is the frame rate the edit is rounded to. Each word goes under its one speaker,
/// who joins an open cue only when talking at once with someone in it ([`Open::at_once`]). A
/// sentence not yet translated shows as said, so a caller writing a file checks [`missing`] first.
/// `c` is the resolved caption settings, apart from `edit` so a caller cannot forget the defaults.
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
    let kept = kept(&tl, edit, words);
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
            // Its speaker first, then anyone else whose overlap turn holds it.
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
/// here, so a phrase around it does not match). Fixes and retimes apply here, the one place word
/// text and times are read on their way to the timeline; a span fixed as one text gives way to
/// the text's own words ([`spread`]).
fn kept<'a>(
    tl: &Timeline,
    edit: &EditList,
    words: impl IntoIterator<Item = (usize, &'a Word)>,
) -> Vec<Shown> {
    let mut fixes = Fixes::new(&edit.fixes);
    let mut times = Times::new(&edit.times);
    // Stable, so words starting together keep the transcript's order, which tells their fixes
    // apart.
    let mut words: Vec<(usize, &Word)> = words.into_iter().collect();
    words.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
    let mut out = Vec::with_capacity(words.len());
    // Each span's kept words in time order, by the span's place in the list.
    let mut spans: Vec<SpanSaid> = Vec::new();
    // Each span's place in `spans`, by its place in the list.
    let mut at: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (track, w) in words {
        // Read before a cut word is dropped: `Fixes` counts every word.
        let fixed = fixes.of(track, w);
        // Counted in heard order as the fixes are, cut words too.
        let (start, end) = times.of(track, w);
        let Some(r) = tl.range_of((start + end) / 2.0) else {
            continue;
        };
        let text = match fixed {
            Some(Fixed::Span { index, text }) => {
                match at.get(&index) {
                    Some(&k) => spans[k].said.push((r, start, end)),
                    None => {
                        at.insert(index, spans.len());
                        spans.push(SpanSaid {
                            track,
                            text,
                            said: vec![(r, start, end)],
                        });
                    }
                }
                continue;
            }
            Some(Fixed::Word(t)) => t,
            None => &w.text,
        };
        out.push(Shown {
            track,
            start: (r, start),
            end: (r, end),
            text: clean(text),
        });
    }
    if spans.is_empty() {
        if !times.is_empty() {
            // A retimed word may now start before a word heard before it (another track's).
            out.sort_by(|a, b| a.start.1.total_cmp(&b.start.1));
        }
        return out;
    }
    for s in &spans {
        out.extend(spread(s.track, s.text, &s.said));
    }
    // A span's words came in at the end; another track's words said meanwhile go between them.
    out.sort_by(|a, b| a.start.1.total_cmp(&b.start.1));
    out
}

/// A span fixed as one text, and its kept words in time order: each one's kept range, start and
/// end (as retimed).
struct SpanSaid<'a> {
    track: usize,
    text: &'a str,
    said: Vec<(usize, f64, f64)>,
}

/// A span's text as words on the timeline: the times of `said` (the span's kept words in time
/// order, with their kept ranges) end to end, pauses left out, cut into one equal share per word
/// of the text, each share placed back on the word it falls in. So no word lands over a pause or
/// a cut; with no length at all, every word starts and ends with the first.
fn spread(track: usize, text: &str, said: &[(usize, f64, f64)]) -> Vec<Shown> {
    let new: Vec<&str> = text.split_whitespace().collect();
    if new.is_empty() || said.is_empty() {
        return Vec::new();
    }
    // Each word's stretch, from where the one before it ended (words on one track may overlap).
    let mut stretches: Vec<(usize, f64, f64)> = Vec::with_capacity(said.len());
    let mut at = f64::NEG_INFINITY;
    for &(r, start, end) in said {
        let a = start.max(at);
        let b = end.max(a);
        stretches.push((r, a, b));
        at = b;
    }
    let total: f64 = stretches.iter().map(|(_, a, b)| b - a).sum();
    let first = stretches[0];
    // `x` seconds into the stretches end to end: a start goes into the stretch after a seam, an end
    // into the one before it.
    let place = |x: f64, end: bool| -> (usize, f64) {
        if total <= 0.0 {
            return (first.0, first.1);
        }
        let mut before = 0.0;
        for (i, &(r, a, b)) in stretches.iter().enumerate() {
            let len = b - a;
            let last = i + 1 == stretches.len();
            if x < before + len || (end && x <= before + len) || last {
                return (r, a + (x - before).clamp(0.0, len));
            }
            before += len;
        }
        (first.0, first.1)
    };
    let n = new.len() as f64;
    new.iter()
        .enumerate()
        .map(|(j, word)| Shown {
            track,
            start: place(total * j as f64 / n, false),
            end: place(total * (j + 1) as f64 / n, true),
            text: clean(word),
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

/// The sentences captions would translate: the kept words' sentences as said, each with its
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
    let kept = kept(&tl, edit, words);
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
pub(super) fn wrap_onto(mut lines: Vec<String>, w: &str, chars: usize) -> Vec<String> {
    match lines.last_mut() {
        Some(l) if width(l) + 1 + width(w) <= chars => {
            l.push(' ');
            l.push_str(w);
        }
        _ => lines.push(w.to_string()),
    }
    lines
}

/// Cues in order and never overlapping (a Resolve subtitle track holds one at a time), all within
/// the edited timeline's `total` seconds. Each cue lasts at least `floor`, then ends by the next
/// one's start, held to [`MIN_S`] where there is room, never past `longest`. A cue pushed past the
/// end joins the one before. Under a word limit nothing is pushed, and cues said at one instant
/// share it ([`share_instants`]).
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

/// The SRT text: CRLF line ends (what Resolve's own export writes), each line in `<font color>` of
/// its part's speaker, closed per line since importers disagree on a tag spanning a break; a word
/// nobody was heard saying is left white. Parts follow line by line with no blank line between
/// (it would end the cue). Bold, italic and underline wrap each line outside its colour, in the
/// order Resolve's own export writes; the rest of a subtitle's look is Resolve's Track Style.
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
mod tests;
