//! Words the user fixed (5h): a misheard word's right spelling, kept in the edit list as `fix
//! <track> <start> <text>` and shown wherever the word is (the transcript panel, the caption
//! preview, the SRT).
//!
//! In the edit list because a fix is the user's edit: saved with every other edit, kept through
//! re-cuts, backed up before a list is replaced. Not beside the transcript, which is a cache a
//! re-run replaces whole. A word is named by its track and its start to the millisecond (the list's
//! precision): stable while a transcript grows piece by piece (2j) and the same in the app and the
//! CLI, where word indices are not. And by which of the words starting in that millisecond on that
//! track it is (5w): Whisper gives runs of short words one start ("en" and "él" both at 54.944), and
//! a fix keyed by the millisecond alone showed on all of them and could not be taken from one.
//!
//! A fix can also cover a run of words (5x: a whole sentence retyped as one edit), a span from its
//! first word to its last on one track. Which words it covers is a comparison of keys, not a lookup
//! of words: a track's words in key order (millisecond, then `nth`) are its words in time order, so a
//! word is in a span when its key lies between the span's ends, whatever words the transcript holds
//! now (a re-transcribed word that moved, a last word that is gone).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::transcribe::Word;

/// Longest fixed text of one word, in characters: a word or a short phrase.
pub const MAX_FIX: usize = 100;

/// Longest fixed text of a span (5x), in characters: a sentence or a few.
pub const MAX_SPAN: usize = 1000;

/// A word as a fix names it, its track aside: its start and which of that millisecond's words it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WordAt {
    /// In source seconds, as the transcript has it.
    pub start: f64,
    /// 0 for the first word on its track starting in that millisecond (5w).
    pub nth: usize,
}

impl WordAt {
    fn key(&self) -> (i64, usize) {
        (millis(self.start), self.nth)
    }
}

/// One fixed word, or a span of words fixed as one (5x).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    /// The audio track (0-based) it was heard on.
    pub track: usize,
    /// Its start in source seconds, as the transcript has it (a span's: its first word's).
    pub start: f64,
    /// Which of the words heard on `track` starting in that millisecond it is, 0 for the first, in
    /// time order with ties in transcript order (5w; [`Fixes`] counts it).
    pub nth: usize,
    /// A span's last word (5x), on the same track; `None` for one word.
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

    /// Both cover a word in common (5x: a span and a fix inside it, or two spans).
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

    /// A span of words rather than one (5x).
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
/// their `nth` (5w).
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

/// Whether `a` comes after `b` as words on one track do (5x: a span's last word before its first).
pub(super) fn after(a: &WordAt, b: &WordAt) -> bool {
    a.key() > b.key()
}

/// `text` as a fix keeps it: trimmed, on one line (the list's line ends it), runs of space as one, at
/// most [`MAX_FIX`] characters. Empty means no fix.
pub fn check_fix(text: &str) -> Result<String, String> {
    check_text(text, MAX_FIX, "a fixed word")
}

/// As [`check_fix`], for a span's text: at most [`MAX_SPAN`] characters (5x).
pub fn check_span(text: &str) -> Result<String, String> {
    check_text(text, MAX_SPAN, "a fixed text")
}

fn check_text(text: &str, max: usize, what: &str) -> Result<String, String> {
    let one: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        return Err(format!("{what} is at most {max} characters"));
    }
    Ok(one)
}

/// Sets the fix of the word heard as `heard` on `track` at `start`, the `nth` starting in that
/// millisecond, to `text` (through [`check_fix`]); empty text, or the heard text again, removes it.
/// No other word's fix changes, but a span holding this word goes (5x): one word cannot show two
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

/// Sets the text of the words heard on `track` from `first` to `last` (5x), heard as `heard` (the
/// words joined by spaces), to `text` (through [`check_span`], or [`check_fix`] for one word): every
/// fix on the track touching those words goes, the fixes of single words inside them and a span
/// overlapping them, and one fix takes their place unless `text` is empty or the heard text again
/// (so an empty text clears the words' fixes). `first == last` is a one-word fix. Kept in time
/// order. Whether anything changed.
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

/// Whether two fixes of `fixes` cover a word in common, which the list's parser refuses (a second line
/// for one word, a span over a fixed word or another span). For the code that adds fixes in bulk (5aa:
/// a spread, a carry onto a new transcript) to check before anything is written: a list that would not
/// read back must never be saved.
pub fn clash(fixes: &[Fix]) -> bool {
    let mut keys: Vec<_> = fixes.iter().map(Fix::span_keys).collect();
    keys.sort_by_key(|k| (k.0, k.1));
    keys.windows(2)
        .any(|w| w[0].0 == w[1].0 && w[1].1 <= w[0].2)
}

/// Marks the word heard as `heard` on `track` at `start`, the `nth` starting in that millisecond (5w),
/// as checked by the user (5o): a fix line with the heard text, so every fix line means "the user
/// vouched for this word" and the transcript panel flags none of them. Not a line kind of its own: an
/// older build reads it as a fix that changes nothing, where a new kind would need a format version
/// it refuses. A word with a fix already is left as it is, and so is a word inside a span (5x): the
/// span is the user's text for it. Whether anything changed. Being a fix line, it goes the way of
/// one: the heard text (or nothing) typed into the fix field takes it away ([`set_fix`]) and the
/// word is flagged again, on purpose, so there is one rule for what that field does whatever the
/// line was made by; a span set over it takes it away too, and the span vouches for the word.
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
    /// which shows `text` in place of all of them (5x).
    Span { index: usize, text: &'a str },
}

/// A list's fixes by word, for reading many words (a 30 h transcript) against a few fixes. Reads the
/// words themselves to count each one's `nth`: every word of the transcript is to be given, in time
/// order with ties in transcript order (a stable sort by start of the segments' words, as the editor
/// and the captions have them), a word to be left out (cut) included.
pub struct Fixes<'a> {
    by_word: HashMap<Key, &'a str>,
    /// Per track, its spans in order of their first word (5x).
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
    /// tells a span apart (5x repair: no caller is left to read a span's words as heard by mistake).
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
mod tests {
    use super::*;

    fn word(start: f64, text: &str) -> Word {
        Word {
            start,
            end: start + 0.3,
            text: text.into(),
            p: 1.0,
            p_min: None,
        }
    }

    #[test]
    fn a_fix_is_set_changed_and_removed() {
        let mut f = Vec::new();
        assert!(set_fix(&mut f, 1, 12.34, 0, " Wezar,", " Uezar, ").unwrap());
        assert_eq!(f[0].text, "Uezar,");
        assert!(
            !set_fix(&mut f, 1, 12.34, 0, " Wezar,", "Uezar,").unwrap(),
            "same"
        );
        assert!(set_fix(&mut f, 0, 3.0, 0, "a", "b").unwrap());
        assert_eq!(
            f.iter().map(|x| x.track).collect::<Vec<_>>(),
            [0, 1],
            "in time"
        );
        // The heard text again, or nothing, takes the fix away.
        assert!(set_fix(&mut f, 1, 12.34, 0, " Wezar,", "Wezar,").unwrap());
        assert!(set_fix(&mut f, 0, 3.0, 0, "a", "  ").unwrap());
        assert!(f.is_empty());
        assert!(
            !set_fix(&mut f, 0, 3.0, 0, "a", "a").unwrap(),
            "nothing to fix"
        );
        // One line, not too long.
        assert!(set_fix(&mut f, 0, 3.0, 0, "a", "Tsun\nheto").unwrap());
        assert_eq!(f[0].text, "Tsun heto");
        assert!(set_fix(&mut f, 0, 3.0, 0, "a", &"x".repeat(MAX_FIX + 1)).is_err());
    }

    /// 5o: a confirmed word is a fix to its own text; a fixed word stays fixed, and the heard text
    /// typed in again (5h) takes the confirmation away like any fix.
    #[test]
    fn a_word_is_confirmed_as_a_fix_to_what_was_heard() {
        let mut f = Vec::new();
        assert!(confirm_fix(&mut f, 1, 12.34, 0, " Uezar, ").unwrap());
        assert_eq!(f[0].text, "Uezar,");
        assert!(!confirm_fix(&mut f, 1, 12.34, 0, "Uezar,").unwrap(), "once");
        assert_eq!(Fixes::new(&f).text(1, &word(12.34, "Uezar,")), "Uezar,");
        assert!(set_fix(&mut f, 0, 3.0, 0, "Wezar", "Uezar").unwrap());
        assert!(!confirm_fix(&mut f, 0, 3.0, 0, "Wezar").unwrap());
        assert_eq!(f[0].text, "Uezar", "the fix stands");
        assert!(set_fix(&mut f, 1, 12.34, 0, "Uezar,", "Uezar,").unwrap());
        assert_eq!(f.len(), 1);
        assert!(confirm_fix(&mut f, 0, 9.0, 0, "  ").is_err());
    }

    /// 5o with 5x: a word inside a span is not confirmed beside it (the span is the user's text for
    /// it), and a span set over a confirmed word takes the confirmation into it.
    #[test]
    fn a_confirmation_and_a_span_never_both_name_a_word() {
        let at = |start| WordAt { start, nth: 0 };
        let mut f = Vec::new();
        assert!(confirm_fix(&mut f, 0, 2.0, 0, "dos").unwrap());
        assert!(set_span(&mut f, 0, at(1.0), at(3.0), "uno dos tres", "one two three").unwrap());
        assert_eq!(f.len(), 1, "the confirmation went into the span");
        assert!(f[0].is_span());
        assert!(!confirm_fix(&mut f, 0, 2.0, 0, "dos").unwrap());
        assert_eq!(f.len(), 1, "no confirmation inside a span");
        // Outside it, on another track or past its end: confirmed as before.
        assert!(confirm_fix(&mut f, 1, 2.0, 0, "dos").unwrap());
        assert!(confirm_fix(&mut f, 0, 4.0, 0, "cuatro").unwrap());
    }

    /// 5o with 5w: confirming one of the words starting in one millisecond confirms that word alone,
    /// next to another's fix, and taking it away leaves the other.
    #[test]
    fn a_word_of_those_starting_together_is_confirmed_alone() {
        let ws = same_start();
        let mut f = Vec::new();
        assert!(set_fix(&mut f, 0, 54.944, 0, "en", "de").unwrap());
        assert!(confirm_fix(&mut f, 0, 54.944, 1, "él").unwrap());
        assert!(
            !confirm_fix(&mut f, 0, 54.944, 0, "en").unwrap(),
            "fixed already"
        );
        assert_eq!(f.iter().map(|x| x.nth).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(said(&f, &ws), ["manada", "de", "él", "y"]);
        assert!(set_fix(&mut f, 0, 54.944, 1, "él", "").unwrap());
        assert_eq!(said(&f, &ws), ["manada", "de", "él", "y"]);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn two_fixes_on_one_word_are_found_before_a_list_is_written() {
        // 5aa repair: the parser refuses such a list, so code adding fixes in bulk checks first.
        let span = Fix {
            track: 0,
            start: 1.0,
            nth: 0,
            through: Some(WordAt { start: 3.0, nth: 0 }),
            text: "a b c".into(),
        };
        let word = |track, start: f64| Fix {
            track,
            start,
            nth: 0,
            through: None,
            text: "x".into(),
        };
        assert!(!clash(&[span.clone(), word(0, 4.0), word(1, 2.0)]));
        assert!(clash(&[word(0, 4.0), span.clone(), word(0, 2.0)]));
        assert!(clash(&[word(0, 2.0), word(0, 2.0)]));
        assert!(!clash(&[]));
    }

    #[test]
    fn a_half_millisecond_start_is_written_as_its_key() {
        assert_eq!(written(12.3125), "12.313");
        assert_eq!(written(2.5), "2.500");
        assert_eq!(written(0.0004), "0.000");
        assert_eq!(written(-1.5), "-1.500");
        let start: f64 = written(12.3125).parse().unwrap();
        assert_eq!(key(0, start, 0), key(0, 12.3125, 0));
    }

    #[test]
    fn a_word_is_found_by_track_and_millisecond() {
        let fixes = vec![Fix {
            track: 1,
            start: 312.34,
            nth: 0,
            through: None,
            text: "Uezar".into(),
        }];
        // A start summed from a chunk's and a token's time, off in the last bits.
        let heard = word(300.0 + 12.34, "Wezar");
        assert_eq!(Fixes::new(&fixes).text(1, &heard), "Uezar");
        assert_eq!(Fixes::new(&fixes).text(0, &heard), "Wezar", "another track");
        assert_eq!(
            Fixes::new(&fixes).text(1, &word(312.341, "Wezar")),
            "Wezar",
            "another word"
        );
    }

    /// The words of the 2026-06-15 recording around 54.9 s: Whisper gave "en" and "él" one start.
    fn same_start() -> Vec<Word> {
        vec![
            word(54.174, "manada"),
            word(54.944, "en"),
            word(54.944, "él"),
            word(55.5, "y"),
        ]
    }

    /// What `ws` say, heard on track 0, read in order.
    fn said(fixes: &[Fix], ws: &[Word]) -> Vec<String> {
        let mut by = Fixes::new(fixes);
        ws.iter().map(|w| by.text(0, w).to_string()).collect()
    }

    #[test]
    fn words_starting_in_one_millisecond_each_take_their_own_fix() {
        // 5w: a fix on "él" showed on "en" too, and neither could be taken away alone.
        let ws = same_start();
        let mut f = Vec::new();
        assert!(set_fix(&mut f, 0, 54.944, 1, "él", "miel").unwrap());
        assert_eq!(said(&f, &ws), ["manada", "en", "miel", "y"]);
        assert!(set_fix(&mut f, 0, 54.944, 0, "en", "de").unwrap());
        assert_eq!(said(&f, &ws), ["manada", "de", "miel", "y"]);
        // Taking one away leaves the other.
        assert!(set_fix(&mut f, 0, 54.944, 0, "en", "").unwrap());
        assert_eq!(said(&f, &ws), ["manada", "en", "miel", "y"]);
        assert!(set_fix(&mut f, 0, 54.944, 1, "él", "él").unwrap());
        assert!(f.is_empty());
        // Another track's word at the same moment counts on its own track.
        let f = vec![Fix {
            track: 0,
            start: 54.944,
            nth: 1,
            through: None,
            text: "miel".into(),
        }];
        let mut by = Fixes::new(&f);
        let other = word(54.944, "yes");
        assert_eq!(by.text(0, &ws[1]), "en");
        assert_eq!(by.text(2, &other), "yes");
        assert_eq!(by.text(0, &ws[2]), "miel");
        assert!(same_millisecond(54.944, 54.9444) && !same_millisecond(54.944, 54.945));
    }

    #[test]
    fn two_tracks_interleaved_in_one_millisecond_are_counted_each_on_its_own() {
        // 5w: `Fixes` counts each track's words of a millisecond as they come, whatever the other
        // track's words between them, and starts again at the next millisecond.
        let fix = |track, start, nth, text: &str| Fix {
            track,
            start,
            nth,
            through: None,
            text: text.into(),
        };
        let f = vec![
            fix(0, 7.0, 1, "zero-1"),
            fix(1, 7.0, 0, "one-0"),
            fix(1, 7.0, 2, "one-2"),
            fix(0, 7.0, 2, "zero-2"),
            fix(1, 7.5, 0, "next"),
        ];
        let stream = [
            (1, word(7.0, "a")),
            (0, word(7.0, "b")),
            (0, word(7.0004, "c")),
            (1, word(7.0, "d")),
            (0, word(7.0, "e")),
            (1, word(7.0003, "f")),
            (1, word(7.5, "g")),
            (0, word(7.5, "h")),
        ];
        let mut by = Fixes::new(&f);
        let got: Vec<&str> = stream.iter().map(|(t, w)| by.text(*t, w)).collect();
        assert_eq!(
            got,
            ["one-0", "b", "zero-1", "d", "zero-2", "one-2", "next", "h"]
        );
    }

    fn at(start: f64, nth: usize) -> WordAt {
        WordAt { start, nth }
    }

    /// What `ws` show, heard on track 0: a span's text once, on its first word.
    fn shown(fixes: &[Fix], ws: &[Word]) -> Vec<String> {
        let mut by = Fixes::new(fixes);
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for w in ws {
            match by.of(0, w) {
                Some(Fixed::Span { index, text }) => {
                    if !seen.contains(&index) {
                        seen.push(index);
                        out.push(text.to_string());
                    }
                }
                Some(Fixed::Word(t)) => out.push(t.to_string()),
                None => out.push(w.text.clone()),
            }
        }
        out
    }

    fn sentence() -> Vec<Word> {
        vec![
            word(1.0, "so"),
            word(2.0, "the"),
            word(2.5, "wezar"),
            word(3.0, "went"),
            word(3.5, "hom"),
            word(4.0, "ok"),
        ]
    }

    #[test]
    fn a_span_replaces_the_fixes_inside_it_and_an_empty_text_clears_them() {
        // 5x: a sentence retyped as one edit; the word fixes inside it go.
        let ws = sentence();
        let mut f = Vec::new();
        set_fix(&mut f, 0, 2.5, 0, "wezar", "Uezar").unwrap();
        set_fix(&mut f, 0, 3.5, 0, "hom", "home").unwrap();
        set_fix(&mut f, 0, 4.0, 0, "ok", "okay").unwrap();
        assert_eq!(
            shown(&f, &ws),
            ["so", "the", "Uezar", "went", "home", "okay"]
        );
        let heard = "the wezar went hom";
        assert!(set_span(
            &mut f,
            0,
            at(2.0, 0),
            at(3.5, 0),
            heard,
            " Uezar  went home. "
        )
        .unwrap());
        assert_eq!(shown(&f, &ws), ["so", "Uezar went home.", "okay"]);
        assert_eq!(f.len(), 2, "the two word fixes inside went: {f:?}");
        assert_eq!(f[0].through, Some(at(3.5, 0)));
        // The same text again changes nothing; another replaces it.
        assert!(!set_span(&mut f, 0, at(2.0, 0), at(3.5, 0), heard, "Uezar went home.").unwrap());
        assert!(set_span(&mut f, 0, at(2.0, 0), at(3.5, 0), heard, "Uezar left.").unwrap());
        assert_eq!(shown(&f, &ws), ["so", "Uezar left.", "okay"]);
        // A span overlapping it takes its place whole.
        assert!(set_span(
            &mut f,
            0,
            at(1.0, 0),
            at(2.5, 0),
            "so the wezar",
            "So Uezar"
        )
        .unwrap());
        assert_eq!(shown(&f, &ws), ["So Uezar", "went", "hom", "okay"]);
        // The heard text again, or nothing, clears every fix in the words.
        assert!(set_span(
            &mut f,
            0,
            at(1.0, 0),
            at(4.0, 0),
            "so the wezar went hom ok",
            ""
        )
        .unwrap());
        assert!(f.is_empty());
        set_fix(&mut f, 0, 3.0, 0, "went", "left").unwrap();
        assert!(set_span(&mut f, 0, at(2.0, 0), at(3.5, 0), heard, heard).unwrap());
        assert!(f.is_empty());
        // Backwards is refused; too long too; one word is a word fix.
        assert!(set_span(&mut f, 0, at(3.5, 0), at(2.0, 0), heard, "x").is_err());
        assert!(set_span(
            &mut f,
            0,
            at(2.0, 0),
            at(3.5, 0),
            heard,
            &"x".repeat(MAX_SPAN + 1)
        )
        .is_err());
        assert!(set_span(
            &mut f,
            0,
            at(2.0, 0),
            at(3.5, 0),
            heard,
            &"x".repeat(MAX_FIX + 1)
        )
        .unwrap());
        assert!(set_span(&mut f, 0, at(2.5, 0), at(2.5, 0), "wezar", "Uezar").unwrap());
        assert_eq!(
            f.len(),
            1,
            "the word fix took the span holding the word: {f:?}"
        );
        assert!(!f[0].is_span());
    }

    #[test]
    fn a_word_is_in_a_span_by_its_key_whatever_the_words_now() {
        // 5x: the span names its first and last words; a word between them is in it, also one the
        // transcript did not have when it was set, and nothing past its last key is.
        let span = vec![Fix {
            track: 0,
            start: 2.0,
            nth: 0,
            through: Some(at(3.5, 0)),
            text: "Uezar went home.".into(),
        }];
        // Re-transcribed: "the" gone, a word at 2.7, two words at 3.5 (the second after the last).
        let ws = [
            word(1.0, "so"),
            word(2.5, "wezar"),
            word(2.7, "uh"),
            word(3.5, "hom"),
            word(3.5, "e"),
            word(4.0, "ok"),
        ];
        assert_eq!(shown(&span, &ws), ["so", "Uezar went home.", "e", "ok"]);
        // Another track's words are not in it.
        let mut by = Fixes::new(&span);
        assert_eq!(by.of(1, &ws[1]), None);
        assert_eq!(
            by.text(0, &ws[1]),
            "wezar",
            "a span word reads as heard on its own"
        );
        // Spans of two tracks, in any order, each on its own.
        let two = vec![
            Fix {
                track: 1,
                start: 1.0,
                nth: 0,
                through: Some(at(4.0, 0)),
                text: "B".into(),
            },
            span[0].clone(),
        ];
        let mut by = Fixes::new(&two);
        assert_eq!(
            by.of(1, &ws[0]),
            Some(Fixed::Span {
                index: 0,
                text: "B"
            })
        );
        assert_eq!(
            by.of(0, &ws[1]),
            Some(Fixed::Span {
                index: 1,
                text: "Uezar went home."
            })
        );
        assert_eq!(by.of(0, &ws[5]), None);
    }
}
