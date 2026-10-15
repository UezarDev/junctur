//! Words the user fixed, kept in the edit list (not the transcript, a cache a re-run replaces) as
//! `fix <track> <start> <text>`. A word is named by track, start to the millisecond and `nth` among
//! that millisecond's words, as indices are not stable; a span covers every key between its ends.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::transcribe::Word;

/// Longest fixed text of one word, in characters: a word or a short phrase.
pub const MAX_FIX: usize = 100;

/// Longest fixed text of a span, in characters: a sentence or a few.
pub const MAX_SPAN: usize = 1000;

/// A word as a fix names it, its track aside: its start and which of that millisecond's words it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WordAt {
    /// In source seconds, as the transcript has it.
    pub start: f64,
    /// 0 for the first word on its track starting in that millisecond: Whisper gives runs of short
    /// words one start.
    pub nth: usize,
}

impl WordAt {
    fn key(&self) -> (i64, usize) {
        (millis(self.start), self.nth)
    }
}

/// One fixed word, or a span of words fixed as one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    /// The audio track (0-based) it was heard on.
    pub track: usize,
    /// Its start in source seconds, as the transcript has it (a span's: its first word's).
    pub start: f64,
    /// Which of the words heard on `track` starting in that millisecond it is, 0 for the first, in
    /// time order with ties in transcript order ([`Fixes`] counts it).
    pub nth: usize,
    /// A span's last word, on the same track; `None` for one word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<WordAt>,
    /// What it says.
    pub text: String,
}

impl Fix {
    fn key(&self) -> Key {
        key(self.track, self.start, self.nth)
    }

    /// Its first and last words' keys on its track.
    pub(super) fn ends(&self) -> ((i64, usize), (i64, usize)) {
        let first = (millis(self.start), self.nth);
        (first, self.through.map_or(first, |t| t.key()))
    }

    /// Both name the same word.
    pub fn same_word(&self, other: &Fix) -> bool {
        self.key() == other.key()
    }

    /// Both cover a word in common (a span and a fix inside it, or two spans).
    pub fn overlaps(&self, other: &Fix) -> bool {
        let (a, b) = self.ends();
        let (c, d) = other.ends();
        self.track == other.track && a <= d && c <= b
    }

    /// Its first and last words' keys, for a check of a whole list ([`clash`]).
    fn span_keys(&self) -> (usize, (i64, usize), (i64, usize)) {
        let (a, b) = self.ends();
        (self.track, a, b)
    }

    /// A span of words rather than one.
    pub fn is_span(&self) -> bool {
        self.through.is_some()
    }
}

/// A word: its track, its start to the millisecond as the list writes it ([`written`]), and which of
/// that millisecond's words it is.
type Key = (usize, i64, usize);

fn key(track: usize, start: f64, nth: usize) -> Key {
    (track, millis(start), nth)
}

/// Both starts fall in the same millisecond, as a fix names it: words that do are told apart by
/// their `nth`.
pub fn same_millisecond(a: f64, b: f64) -> bool {
    millis(a) == millis(b)
}

/// Seconds to whole milliseconds, half away from zero.
pub(super) fn millis(start: f64) -> i64 {
    (start * 1000.0).round() as i64
}

/// A fix's start as the list writes it: its key's millisecond, as seconds. Not `{:.3}`, which rounds
/// a half millisecond to even (12.3125 to 12.312) where the key rounds it away (12313), so a fix
/// written that way would name another millisecond when read back and stop applying.
pub(super) fn written(start: f64) -> String {
    let ms = millis(start);
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.unsigned_abs();
    format!("{sign}{}.{:03}", ms / 1000, ms % 1000)
}

/// Whether `a` comes after `b` as words on one track do (a span's last word before its first).
pub(super) fn after(a: &WordAt, b: &WordAt) -> bool {
    a.key() > b.key()
}

/// `text` as a fix keeps it: trimmed, on one line (the list's line ends it), runs of space as one, at
/// most [`MAX_FIX`] characters. Empty means no fix.
pub fn check_fix(text: &str) -> Result<String, String> {
    check_text(text, MAX_FIX, "a fixed word")
}

/// As [`check_fix`], for a span's text: at most [`MAX_SPAN`] characters.
pub fn check_span(text: &str) -> Result<String, String> {
    check_text(text, MAX_SPAN, "a fixed text")
}

fn check_text(text: &str, max: usize, what: &str) -> Result<String, String> {
    let one = crate::text::squeezed(text);
    if one.chars().count() > max {
        return Err(format!("{what} is at most {max} characters"));
    }
    Ok(one)
}

/// Sets the fix of the word heard as `heard` on `track` at `start`, the `nth` starting in that
/// millisecond, to `text` (through [`check_fix`]); empty text, or the heard text again, removes it.
/// No other word's fix changes, but a span holding this word goes: one word cannot show two
/// texts. Kept in time order. Whether anything changed.
pub fn set_fix(
    fixes: &mut Vec<Fix>,
    track: usize,
    start: f64,
    nth: usize,
    heard: &str,
    text: &str,
) -> Result<bool, String> {
    let at = WordAt { start, nth };
    set_span(fixes, track, at, at, heard, text)
}

/// Sets the text of the words heard on `track` from `first` to `last`, heard as `heard`, to `text`
/// (through [`check_span`], or [`check_fix`] for one word): every fix touching those words goes, and
/// one fix takes their place unless `text` is empty or the heard text again. Whether anything
/// changed.
pub fn set_span(
    fixes: &mut Vec<Fix>,
    track: usize,
    first: WordAt,
    last: WordAt,
    heard: &str,
    text: &str,
) -> Result<bool, String> {
    if after(&first, &last) {
        return Err("a fixed text's last word comes before its first".into());
    }
    let one = first.key() == last.key();
    let text = if one {
        check_fix(text)?
    } else {
        check_span(text)?
    };
    let heard = check_span(heard).unwrap_or_default();
    let new = Fix {
        track,
        start: first.start,
        nth: first.nth,
        through: (!one).then_some(last),
        text,
    };
    let before = fixes.clone();
    // The fix already there for exactly these words keeps its own start (a re-read word's start may
    // differ from the list's in the last bits), so setting it again changes nothing.
    let same = fixes
        .iter()
        .position(|f| f.track == track && f.ends() == new.ends());
    fixes.retain(|f| !f.overlaps(&new));
    if !new.text.is_empty() && new.text != heard {
        match same {
            Some(i) => fixes.push(Fix {
                text: new.text,
                ..before[i].clone()
            }),
            None => fixes.push(new),
        }
        sort(fixes);
    }
    Ok(*fixes != before)
}

/// Whether two fixes of `fixes` cover a word in common, which the list's parser refuses. Code adding
/// fixes in bulk checks it before writing: a list that would not read back must never be saved.
pub fn clash(fixes: &[Fix]) -> bool {
    let mut keys: Vec<_> = fixes.iter().map(Fix::span_keys).collect();
    keys.sort_by_key(|k| (k.0, k.1));
    keys.windows(2)
        .any(|w| w[0].0 == w[1].0 && w[1].1 <= w[0].2)
}

/// Marks the word heard as `heard` on `track` at `start` (the `nth` of that millisecond) as checked
/// by the user: a fix line with the heard text, not a line kind of its own, so older builds still
/// read the list. A word already fixed or inside a span is left. Whether anything changed.
pub fn confirm_fix(
    fixes: &mut Vec<Fix>,
    track: usize,
    start: f64,
    nth: usize,
    heard: &str,
) -> Result<bool, String> {
    let new = Fix {
        track,
        start,
        nth,
        through: None,
        text: String::new(),
    };
    if fixes.iter().any(|f| f.overlaps(&new)) {
        return Ok(false);
    }
    let text = check_fix(heard)?;
    if text.is_empty() {
        return Err("this word has no text to confirm".into());
    }
    fixes.push(Fix { text, ..new });
    sort(fixes);
    Ok(true)
}

/// The list's order: by start, then track, then which word of the millisecond.
pub(super) fn sort(fixes: &mut [Fix]) {
    fixes.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then(a.track.cmp(&b.track))
            .then(a.nth.cmp(&b.nth))
    });
}

/// What a word shows ([`Fixes::of`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fixed<'a> {
    /// The word's own fix.
    Word(&'a str),
    /// It is one of the words of span `index` (its place in the slice [`Fixes`] was made from),
    /// which shows `text` in place of all of them.
    Span { index: usize, text: &'a str },
}

/// A list's fixes by word, for reading many words (a 30 h transcript) against a few fixes. Reads the
/// words themselves to count each one's `nth`: every word of the transcript is to be given, in time
/// order with ties in transcript order (a stable sort by start of the segments' words, as the editor
/// and the captions have them), a word to be left out (cut) included.
pub struct Fixes<'a> {
    by_word: HashMap<Key, &'a str>,
    /// Per track, its spans in order of their first word.
    spans: HashMap<usize, Vec<Span<'a>>>,
    /// Per track, the millisecond of the latest word read and how many of its words were read.
    seen: HashMap<usize, (i64, usize)>,
}

struct Span<'a> {
    first: (i64, usize),
    last: (i64, usize),
    index: usize,
    text: &'a str,
}

impl<'a> Fixes<'a> {
    pub fn new(fixes: &'a [Fix]) -> Self {
        let mut spans: HashMap<usize, Vec<Span<'a>>> = HashMap::new();
        let mut by_word = HashMap::new();
        for (index, f) in fixes.iter().enumerate() {
            if f.is_span() {
                let (first, last) = f.ends();
                spans.entry(f.track).or_default().push(Span {
                    first,
                    last,
                    index,
                    text: &f.text,
                });
            } else {
                by_word.insert(f.key(), f.text.as_str());
            }
        }
        for s in spans.values_mut() {
            s.sort_by_key(|s| s.first);
        }
        Self {
            by_word,
            spans,
            seen: HashMap::new(),
        }
    }

    /// What the next word, `w` heard on `track`, shows if the user fixed it: its own fix, or the span
    /// it is in. The span wins: [`set_span`] never leaves both, and a list holding both is refused
    /// when read.
    pub fn of(&mut self, track: usize, w: &Word) -> Option<Fixed<'a>> {
        if self.by_word.is_empty() && self.spans.is_empty() {
            return None;
        }
        let ms = millis(w.start);
        let seen = self.seen.entry(track).or_insert((ms, 0));
        if seen.0 == ms {
            seen.1 += 1;
        } else {
            *seen = (ms, 1);
        }
        let k = (ms, seen.1 - 1);
        if let Some(spans) = self.spans.get(&track) {
            // The last span starting at or before the word, if the word is not past its end.
            let i = spans.partition_point(|s| s.first <= k);
            if let Some(s) = i.checked_sub(1).map(|i| &spans[i]).filter(|s| k <= s.last) {
                return Some(Fixed::Span {
                    index: s.index,
                    text: s.text,
                });
            }
        }
        self.by_word.get(&(track, k.0, k.1)).map(|t| Fixed::Word(t))
    }

    /// What the next word, `w` heard on `track`, says on its own, for the tests: its fix, else what
    /// Whisper heard; a word in a span reads as heard. Callers showing words take [`Self::of`], which
    /// tells a span apart.
    #[cfg(test)]
    pub fn text<'w>(&mut self, track: usize, w: &'w Word) -> &'w str
    where
        'a: 'w,
    {
        match self.of(track, w) {
            Some(Fixed::Word(t)) => t,
            _ => &w.text,
        }
    }
}

#[cfg(test)]
mod tests;
