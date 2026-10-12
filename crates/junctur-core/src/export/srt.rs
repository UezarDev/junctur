//! Captions as an SRT file on the edited timeline, each cue in its speaker's colour (5a).
//!
//! SRT because it is the one format checked to bring per-cue colour into DaVinci Resolve: Uezar
//! imported a test file with `<font color>` per cue and the colours came through; the same test as
//! WebVTT (`::cue` classes) did not. Premiere Pro is unverified; reports say its import gives every
//! caption its default style. Only what an SRT carries into Resolve is written: text, timing,
//! colour. Each line gets its own `<font>` tag, since a tag spanning a line break is the case
//! importers disagree on.
//!
//! Times are output times: a kept word is mapped through the kept ranges on the frame grid
//! ([`frame_ranges`]), exactly as the renderer and the FCPXML lay the ranges end to end, so a cue
//! lands on the same frames in the render and in Resolve's timeline. A range-by-range sum of
//! seconds would drift by up to a frame per cut, seconds over a long edit.

use std::fmt::Write;

use crate::edit::{frame_ranges, frames_ceil, Captions, EditList};
use crate::transcribe::Word;

/// A new cue after a pause this long in the edited timeline, even mid-sentence.
const PAUSE_S: f64 = 1.0;
/// A cue stays at least this long when the next one leaves room: one word flashed for its own
/// 0.2 s cannot be read.
const MIN_S: f64 = 0.8;

#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// Output seconds.
    pub start: f64,
    pub end: f64,
    /// Speaker id, 0 for none.
    pub speaker: u32,
    pub lines: Vec<String>,
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

/// `words` broken greedily into lines of at most `chars` (a longer word gets a line to itself).
fn wrap(words: &[String], chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for w in words {
        match lines.last_mut() {
            Some(l) if width(l) + 1 + width(w) <= chars => {
                l.push(' ');
                l.push_str(w);
            }
            _ => lines.push(w.clone()),
        }
    }
    lines
}

/// What a word shows as: trimmed, without `<` and `>` (they would read as tags), braces kept out
/// too (`{\an8}`-style codes).
fn clean(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | '{' | '}'))
        .collect()
}

/// The cues for `edit`'s kept words. `words` is every transcript word with the track it was heard
/// on, in any order; `num/den` is the frame rate the edit is rounded to (the source's).
pub fn cues<'a>(
    edit: &EditList,
    words: impl IntoIterator<Item = (usize, &'a Word)>,
    num: u64,
    den: u64,
) -> Vec<Cue> {
    let c: &Captions = &edit.captions;
    let tl = Timeline::new(edit, num, den);
    let label = edit.speakers.labeller();
    let mut words: Vec<(usize, &Word)> = words.into_iter().collect();
    words.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));

    struct Open {
        start: f64,
        end: f64,
        speaker: u32,
        words: Vec<String>,
    }
    let mut out: Vec<Cue> = Vec::new();
    let mut open: Option<Open> = None;
    let close = |o: Open, out: &mut Vec<Cue>| {
        out.push(Cue {
            start: o.start,
            end: o.end,
            speaker: o.speaker,
            lines: wrap(&o.words, c.chars as usize),
        })
    };
    for (track, w) in words {
        let text = clean(&w.text);
        if text.is_empty() {
            continue;
        }
        let Some(i) = tl.range_of((w.start + w.end) / 2.0) else {
            continue;
        };
        let (start, end) = (tl.map(i, w.start), tl.map(i, w.end));
        let speaker = label.of_word(track, w.start, w.end).unwrap_or(0);
        if let Some(o) = &mut open {
            let mut more = o.words.clone();
            more.push(text.clone());
            let fits = wrap(&more, c.chars as usize).len() <= c.lines as usize;
            if o.speaker == speaker && start - o.end < PAUSE_S && end - o.start <= c.seconds && fits
            {
                o.words = more;
                o.end = o.end.max(end);
                continue;
            }
        }
        if let Some(o) = open.take() {
            close(o, &mut out);
        }
        open = Some(Open {
            start,
            end,
            speaker,
            words: vec![text],
        });
    }
    if let Some(o) = open.take() {
        close(o, &mut out);
    }
    settle(&mut out, c.seconds);
    out
}

/// Cues in order and never overlapping (Resolve puts subtitles on one track, which cannot hold
/// two at once): each ends by the next one's start; a short one is held to [`MIN_S`] where there
/// is room, never past the longest a cue may last.
fn settle(cues: &mut [Cue], longest: f64) {
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    for i in 0..cues.len() {
        let next = cues.get(i + 1).map_or(f64::INFINITY, |n| n.start);
        let c = &mut cues[i];
        let want = c.end.max(c.start + MIN_S).min(c.start + longest);
        c.end = want.min(next).max(c.start + 0.001);
        let end = c.end;
        if let Some(n) = cues.get_mut(i + 1) {
            // Words of two tracks can start together; the later cue starts once this one ends.
            n.start = n.start.max(end);
            if n.end <= n.start {
                n.end = n.start + 0.001;
            }
        }
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

/// The SRT text: CRLF line ends (what Windows tools and Resolve's own export write), each line of
/// a speaker's cue in `<font color>`; a word nobody was heard saying is left white.
pub fn write(cues: &[Cue], edit: &EditList) -> String {
    let mut out = String::new();
    for (n, c) in cues.iter().enumerate() {
        let _ = write!(
            out,
            "{}\r\n{} --> {}\r\n",
            n + 1,
            stamp(c.start),
            stamp(c.end)
        );
        let color = edit.speakers.color(c.speaker);
        for line in &c.lines {
            match color {
                Some(col) => {
                    let _ = write!(out, "<font color=\"{col}\">{line}</font>\r\n");
                }
                None => {
                    let _ = write!(out, "{line}\r\n");
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
            captions,
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
        let c = cues(&e, w.iter().map(|w| (0, w)), 30, 1);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(c[0].lines, vec!["hola amigos"]);
        assert_eq!(c[0].speaker, 1);
        assert!((c[0].start - 0.5).abs() < 1e-9);
        assert_eq!(c[1].lines, vec!["qué tal"]);
        assert_eq!(c[1].speaker, 2);
        assert!((c[1].start - 2.2).abs() < 1e-9, "{}", c[1].start);
        let text = write(&c, &e);
        assert!(text.starts_with("1\r\n00:00:00,500 --> 00:00:01,600\r\n<font color=\"#F2B33D\">hola amigos</font>\r\n\r\n2\r\n00:00:02,200 --> "), "{text}");
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
        let c = cues(&e, [(0, &last)], 30, 1);
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
            word(8.0, 8.2, "siete"), // still under 1 s gap? no: 2.5 s pause
            word(19.0, 19.5, "a"),
            word(20.5, 20.8, "b"), // speaker 2: new cue
        ];
        let c = cues(&e, w.iter().map(|w| (0, w)), 30, 1);
        let lines: Vec<String> = c.iter().map(|c| c.lines.join("|")).collect();
        assert_eq!(
            lines,
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
        let c = cues(&e, w.iter().map(|w| (0, w)), 30, 1);
        assert!(c.len() >= 3);
        assert!(c.iter().all(|c| c.end - c.start <= 2.0 + 1e-9), "{c:?}");
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
        let c = cues(&e, w.iter().map(|w| (0, w)), 30, 1);
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

    #[test]
    fn odd_text_and_unlabelled_words() {
        let mut e = edit(&[(0.0, 100.0)], Captions::default());
        e.speakers = Speakers::default();
        let w = [
            word(1.0, 1.2, " <b>hey</b> "),
            word(1.3, 1.4, "{\\an8}"),
            word(1.5, 1.6, "   "),
        ];
        let c = cues(&e, w.iter().map(|w| (0, w)), 30, 1);
        assert_eq!(c[0].lines, vec!["bhey/b \\an8"]);
        let text = write(&c, &e);
        assert!(!text.contains("<font"), "no speaker, no colour");
        assert!(cues(&e, [], 30, 1).is_empty());
        assert_eq!(stamp(3723.4567), "01:02:03,457");
    }
}
