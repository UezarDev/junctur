//! Captions as an SRT file on the edited timeline, each cue in its speaker's colour: SRT is the
//! format whose per-cue colour comes through into DaVinci Resolve. A Resolve subtitle track shows
//! one caption at a time, so cues never overlap and people talking at once share a cue, a [`Part`]
//! each. Times go through the kept ranges on the frame grid ([`frame_ranges`](crate::edit::frame_ranges)), as the renderer and
//! the FCPXML do, so a cue lands on the same frames in the render and in Resolve.

mod file;
mod kept_words;
mod open;
mod timeline;
mod timing;

use serde::Serialize;

use super::caption_text;
use super::translated;
use crate::edit::{Captions, EditList, Labeller};
use crate::transcribe::Word;
use crate::translate::Translations;

pub use file::write;
pub use kept_words::{missing, sentences};
pub(super) use open::wrap_onto;

use kept_words::{clean, kept, said, sentence_words};
use open::{Open, Said};
use timeline::Timeline;
use timing::settle;

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

/// Characters as a reader counts them.
fn width(s: &str) -> usize {
    s.chars().count()
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
    let translated = translations
        .filter(|_| !c.translate.is_empty())
        .map(|t| (t, sentence_words(&kept, &tl, &label)));
    cut(&tl, &label, kept, translated, c)
}

/// [`cues`] with every translation, from one pass over the kept words; while some of the
/// sentences ([`sentences`]) have no translation yet, `Err` of how many lack one and of how many.
pub fn finished<'a>(
    edit: &EditList,
    c: &Captions,
    words: impl IntoIterator<Item = (usize, &'a Word)>,
    num: u64,
    den: u64,
    translations: &Translations,
) -> Result<Vec<Cue>, (usize, usize)> {
    let tl = Timeline::new(edit, num, den);
    let label = edit.speakers.labeller();
    let kept = kept(&tl, edit, words);
    if c.translate.is_empty() {
        return Ok(cut(&tl, &label, kept, None, c));
    }
    let s = sentence_words(&kept, &tl, &label);
    let said = said(&kept, &s);
    match missing(c, &said, translations) {
        0 => Ok(cut(&tl, &label, kept, Some((translations, s)), c)),
        n => Err((n, said.len())),
    }
}

/// The cues of the kept words, translated with each sentence's translation when given.
fn cut(
    tl: &Timeline,
    label: &Labeller,
    kept: Vec<translated::Shown>,
    translated: Option<(&Translations, Vec<Vec<usize>>)>,
    c: &Captions,
) -> Vec<Cue> {
    let mut shown = match translated {
        Some((t, s)) => translated::apply(kept, &s, |text| t.get(&c.translate, text), clean),
        None => kept,
    };
    // Taken, not cloned: only the words' tracks and times are read past here.
    let texts: Vec<(usize, String)> = shown
        .iter_mut()
        .map(|w| (w.track, std::mem::take(&mut w.text)))
        .collect();

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
    let floor = (c.words == 0).then_some(READABLE_S);
    settle(&mut out, c.seconds, floor, tl.total());
    out
}

#[cfg(test)]
mod tests;
