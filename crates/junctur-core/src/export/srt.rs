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

use crate::edit::{frame_ranges, frames_ceil, Captions, EditList};
use crate::transcribe::Word;

/// A new cue after a pause this long in the edited timeline, even mid-sentence.
const PAUSE_S: f64 = 1.0;
/// A cue stays at least this long when the next one leaves room: one word flashed for its own
/// 0.2 s cannot be read.
const MIN_S: f64 = 0.8;
/// No cue is shorter than this: when the next one would start sooner (quick turns, or a stacked
/// overlap cut at its limits), the next one waits. Shorter than [`MIN_S`] so a quick exchange does not drift far behind.
const READABLE_S: f64 = 0.5;
/// [`READABLE_S`] when a word limit is set (5i): shorter than a spoken word, so one word at a time
/// follows the voice instead of each word waiting out 0.5 s and the captions falling behind.
const WORD_READABLE_S: f64 = 0.2;

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
/// `c` is the caption settings to cut with, resolved by the caller: the list's own, else the
/// user's defaults (2i, `config::captions`). Taken apart from `edit`, whose `captions` may be
/// `None`, so a caller cannot forget the defaults and silently get the built-in ones.
pub fn cues<'a>(
    edit: &EditList,
    c: &Captions,
    words: impl IntoIterator<Item = (usize, &'a Word)>,
    num: u64,
    den: u64,
) -> Vec<Cue> {
    let tl = Timeline::new(edit, num, den);
    let label = edit.speakers.labeller();
    let mut words: Vec<(usize, &Word)> = words.into_iter().collect();
    words.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));

    let mut out: Vec<Cue> = Vec::new();
    let mut open: Option<Open> = None;
    for (track, w) in words {
        let text = clean(&w.text);
        if text.is_empty() {
            continue;
        }
        let Some(i) = tl.range_of((w.start + w.end) / 2.0) else {
            continue;
        };
        let said = Said {
            track,
            start: tl.map(i, w.start),
            end: tl.map(i, w.end),
            // Its speaker first, then anyone else whose overlap turn holds it (5b).
            speakers: label.speakers_of_word(track, w.start, w.end),
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
    let floor = if c.words == 0 {
        READABLE_S
    } else {
        WORD_READABLE_S
    };
    settle(&mut out, c.seconds, floor, tl.total());
    out
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
    /// Transcript words taken, for the word limit (5i): one each, whatever spaces its text holds.
    words: u32,
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
                let lines: Vec<String> = p.lines.last().cloned().into_iter().collect();
                parts.push(OpenPart {
                    speaker: p.speaker,
                    words: lines.iter().map(|l| l.split(' ').count() as u32).sum(),
                    lines,
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
                p.lines = wrap_onto(std::mem::take(&mut p.lines), text, chars);
                p.words += 1;
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
/// `floor` ([`READABLE_S`], or [`WORD_READABLE_S`] under a word limit): a next cue starting sooner
/// (a quick turn) waits that long. Then
/// each ends by the next one's start, held to [`MIN_S`] where there is room, never past the
/// longest a cue may last. A cue pushed past the end of the timeline joins the one before.
fn settle(cues: &mut Vec<Cue>, longest: f64, floor: f64, total: f64) {
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
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
    // before it rather than being lost (a part per speaker, as in any cue).
    let mut kept: Vec<Cue> = Vec::with_capacity(cues.len());
    for c in cues.drain(..) {
        match kept.last_mut() {
            Some(prev) if c.end <= c.start => {
                for part in c.parts {
                    match prev.parts.iter_mut().find(|p| p.speaker == part.speaker) {
                        Some(p) => p.lines.extend(part.lines),
                        None => prev.parts.push(part),
                    }
                }
            }
            _ if c.end <= c.start => {}
            _ => kept.push(c),
        }
    }
    *cues = kept;
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
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1);
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
        let c = cues(&e, &own(&e), [(0, &last)], 30, 1);
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
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1);
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
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1);
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
            cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1)
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
        let got: Vec<_> = cues(&e, &own(&e), words, 30, 1).iter().map(parts).collect();
        assert_eq!(
            got,
            vec![
                vec![(1, "uno dos".into()), (2, "sí".into())],
                vec![(1, "tres".into()), (2, "sí no".into())],
            ]
        );
    }

    #[test]
    fn a_quick_turn_still_gets_readable_time() {
        // Speaker 1 to 20 s, speaker 2 from 20 s (`edit`), one mixed track: "no" starts 0.1 s after
        // "sí", one speaker after the other, not at once.
        let e = edit(&[(0.0, 40.0)], Captions::default());
        let (a, b) = (word(19.9, 20.0, "sí"), word(20.0, 20.2, "no"));
        let c = cues(&e, &own(&e), [(0, &a), (0, &b)], 30, 1);
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
        let c = cues(&e, &own(&e), [(0, &w)], 30, 1);
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
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1);
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
        let c = cues(&e, &own(&e), words, 30, 1);
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
        let c = cues(&e, &own(&e), [(0, &a), (3, &b)], 30, 1);
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
        let c = cues(&e, &own(&e), words, 30, 1);
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
        let c = cues(&e, &own(&e), words, 30, 1);
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
        let c = cues(&e, &own(&e), [(0, &a), (3, &b)], 30, 1);
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
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1);
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
        let c = cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1);
        assert_eq!(parts(&c[0]), vec![(0, "hey vale dos líneas ab".into())]);
        let text = write(&c, &e, &own(&e));
        assert!(!text.contains("<font"), "no speaker, no colour");
        assert!(cues(&e, &own(&e), [], 30, 1).is_empty());
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
                &cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1),
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
            &cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1),
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
                &cues(&e, &own(&e), w.iter().map(|w| (0, w)), 30, 1),
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
