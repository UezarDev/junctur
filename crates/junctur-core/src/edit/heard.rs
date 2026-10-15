//! Reads a transcript's words as fixes name them: which words a fix is on, as heard, for the carry
//! onto a new transcription (`carry.rs`) and the list of a recording's fixes. Apart from `fixes.rs`
//! because it works over a whole transcript, not one word.

use std::collections::HashMap;

use super::fixes::{self, check_span, Fix, Fixes, WordAt};
use crate::text::{bare, split_bare};
use crate::transcribe::Word;

/// A word's text as two hearings of the same word compare: trimmed of the punctuation around it,
/// lower-cased. "Bane," and "bane" are alike; "Bane" and "Vayne" are not.
pub fn likeness(text: &str) -> String {
    bare(text).to_lowercase()
}

/// `s`'s punctuation before and after its word.
fn around(s: &str) -> (&str, &str) {
    let (lead, _, trail) = split_bare(s.trim());
    (lead, trail)
}

/// One word of the stream, as the scans need it.
pub(super) struct Heard<'a> {
    pub(super) track: usize,
    pub(super) at: WordAt,
    pub(super) key: (i64, usize),
    pub(super) text: &'a str,
    pub(super) like: &'a str,
    pub(super) start: f64,
    pub(super) end: f64,
}

/// Each word's [`likeness`], in the stream's order.
pub(super) fn likes(words: &[(usize, &Word)]) -> Vec<String> {
    words.iter().map(|(_, w)| likeness(&w.text)).collect()
}

/// The words in [`Fixes`]' order with each one's place in its millisecond (`nth`), counted per track
/// as [`Fixes::of`] counts it; `likes` is [`likes`] of them.
pub(super) fn read<'a>(words: &[(usize, &'a Word)], likes: &'a [String]) -> Vec<Heard<'a>> {
    let mut seen: HashMap<usize, (i64, usize)> = HashMap::new();
    words
        .iter()
        .zip(likes)
        .map(|(&(track, w), like)| {
            let ms = (w.start * 1000.0).round() as i64;
            let s = seen.entry(track).or_insert((ms, 0));
            if s.0 == ms {
                s.1 += 1;
            } else {
                *s = (ms, 1);
            }
            let nth = s.1 - 1;
            Heard {
                track,
                at: WordAt {
                    start: w.start,
                    nth,
                },
                key: (ms, nth),
                text: w.text.trim(),
                like,
                start: w.start,
                end: w.end,
            }
        })
        .collect()
}

/// Which words a fix of `fixes` already covers, word by word.
pub(super) fn covered(fixes: &[Fix], words: &[(usize, &Word)]) -> Vec<bool> {
    let mut by = Fixes::new(fixes);
    words.iter().map(|&(t, w)| by.of(t, w).is_some()).collect()
}

/// Per track, the stream's places of its words, in time order.
pub(super) fn by_track(heard: &[Heard]) -> HashMap<usize, Vec<usize>> {
    let mut out: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, h) in heard.iter().enumerate() {
        out.entry(h.track).or_default().push(i);
    }
    out
}

/// The stream's places of the words `f` names, on its track: one for a word fix, its words for a span.
pub(super) fn words_of(
    f: &Fix,
    heard: &[Heard],
    tracks: &HashMap<usize, Vec<usize>>,
) -> Vec<usize> {
    let first = (fixes::millis(f.start), f.nth);
    let last = f.through.map_or(first, |t| (fixes::millis(t.start), t.nth));
    let Some(on) = tracks.get(&f.track) else {
        return Vec::new();
    };
    let from = on.partition_point(|&i| heard[i].key < first);
    on[from..]
        .iter()
        .copied()
        .take_while(|&i| heard[i].key <= last)
        .collect()
}

/// What a word fix says on its word heard again with other punctuation (the carry onto a new
/// transcription).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Text {
    /// The fixed word in the new word's own punctuation: the fix kept the punctuation it was heard
    /// with ("Bane," → "Vayne,"), so "Bane." heard now becomes "Vayne.".
    Core(String),
    /// The text as typed.
    Typed(String),
}

impl Text {
    /// The rule a one-word fix from `heard` to `fixed` follows on its word heard again.
    pub(super) fn of(heard: &str, fixed: &str) -> Text {
        if around(fixed) == around(heard) && !bare(fixed).is_empty() {
            Text::Core(bare(fixed).to_string())
        } else {
            Text::Typed(fixed.to_string())
        }
    }

    pub(super) fn on(&self, heard: &str) -> String {
        match self {
            Text::Core(core) => {
                let (lead, trail) = around(heard);
                format!("{lead}{core}{trail}")
            }
            Text::Typed(t) => t.clone(),
        }
    }
}

/// A fix of the list where the transcript has it (5ae: the list the user takes wrong fixes out of).
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub fix: Fix,
    /// Its first and last words' places in the transcript; `None` when it is on no word of it now.
    pub words: Option<(usize, usize)>,
    /// Its words as heard, joined by spaces; empty when it is on no word.
    pub heard: String,
}

impl Placed {
    /// A fix of the heard text itself (Looks right, or an older Junctur's Undo those and Remove): the
    /// words read as heard.
    pub fn as_heard(&self) -> bool {
        !self.heard.is_empty() && check_span(&self.heard).ok() == check_span(&self.fix.text).ok()
    }
}

/// Every fix of `fixes` with the words of `words` (the transcript, in [`Fixes`]' order) it is on, in
/// the list's order.
pub fn placed(fixes: &[Fix], words: &[(usize, &Word)]) -> Vec<Placed> {
    let likes = likes(words);
    let heard = read(words, &likes);
    let tracks = by_track(&heard);
    fixes
        .iter()
        .map(|f| {
            let at = words_of(f, &heard, &tracks);
            Placed {
                fix: f.clone(),
                words: at.first().zip(at.last()).map(|(&a, &b)| (a, b)),
                heard: at
                    .iter()
                    .map(|&i| heard[i].text)
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        })
        .collect()
}

/// Takes the fixes of `gone` still in `fixes` as listed out of the list (5ae's Remove): their words
/// read as heard again. One changed since stays. How many went.
pub fn remove(fixes: &mut Vec<Fix>, gone: &[Fix]) -> usize {
    let before = fixes.len();
    fixes.retain(|f| !gone.iter().any(|g| same(f, g)));
    before - fixes.len()
}

/// Both name the same words with the same text (a fix read back from the list may differ from the one
/// it was written from in the last bits of its start).
pub(super) fn same(a: &Fix, b: &Fix) -> bool {
    a.track == b.track && a.ends() == b.ends() && a.text == b.text
}

#[cfg(test)]
mod tests;
