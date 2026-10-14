//! The kept words on their way to the timeline, fixes and retimes applied, and the sentences
//! captions translate, cut from the same words as the cues.

use super::timeline::Timeline;
use super::PAUSE_S;
use crate::edit::{Captions, EditList, Fixed, Fixes, Labeller, Times};
use crate::export::translated::{self, Shown};
use crate::transcribe::Word;
use crate::translate::{Sentence, Translations};

/// What a word shows as: whole `<...>` and `{...}` spans removed (they would read as tags or
/// `{\an8}`-style codes; a bracket left open drops just the bracket), line breaks as spaces (a
/// blank line ends a cue), runs of space as one, trimmed.
pub(super) fn clean(text: &str) -> String {
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

/// The kept words, in time order, each in its kept range and cleaned (a word left empty is kept
/// here, so a phrase around it does not match). Fixes and retimes apply here, the one place word
/// text and times are read on their way to the timeline; a span fixed as one text gives way to
/// the text's own words ([`spread`]).
pub(super) fn kept<'a>(
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
pub(super) fn sentence_words(kept: &[Shown], tl: &Timeline, label: &Labeller) -> Vec<Vec<usize>> {
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
