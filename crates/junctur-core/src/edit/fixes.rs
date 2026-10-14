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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::transcribe::Word;

/// Longest fixed text, in characters: a word or a short phrase.
pub const MAX_FIX: usize = 100;

/// One fixed word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    /// The audio track (0-based) it was heard on.
    pub track: usize,
    /// Its start in source seconds, as the transcript has it.
    pub start: f64,
    /// Which of the words heard on `track` starting in that millisecond it is, 0 for the first, in
    /// time order with ties in transcript order (5w; [`Fixes`] counts it).
    pub nth: usize,
    /// What it says.
    pub text: String,
}

impl Fix {
    fn key(&self) -> Key {
        key(self.track, self.start, self.nth)
    }

    /// Both name the same word.
    pub fn same_word(&self, other: &Fix) -> bool {
        self.key() == other.key()
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
fn millis(start: f64) -> i64 {
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

/// `text` as a fix keeps it: trimmed, on one line (the list's line ends it), runs of space as one, at
/// most [`MAX_FIX`] characters. Empty means no fix.
pub fn check_fix(text: &str) -> Result<String, String> {
    let one: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > MAX_FIX {
        return Err(format!("a fixed word is at most {MAX_FIX} characters"));
    }
    Ok(one)
}

/// Sets the fix of the word heard as `heard` on `track` at `start`, the `nth` starting in that
/// millisecond, to `text` (through [`check_fix`]); empty text, or the heard text again, removes it.
/// No other word's fix changes. Kept in time order. Whether anything changed.
pub fn set_fix(
    fixes: &mut Vec<Fix>,
    track: usize,
    start: f64,
    nth: usize,
    heard: &str,
    text: &str,
) -> Result<bool, String> {
    let text = check_fix(text)?;
    let heard = check_fix(heard).unwrap_or_default();
    let k = key(track, start, nth);
    let at = fixes.iter().position(|f| f.key() == k);
    let before = fixes.clone();
    match at {
        Some(i) if text.is_empty() || text == heard => {
            fixes.remove(i);
        }
        Some(i) => fixes[i].text = text,
        None if text.is_empty() || text == heard => {}
        None => {
            fixes.push(Fix {
                track,
                start,
                nth,
                text,
            });
            sort(fixes);
        }
    }
    Ok(*fixes != before)
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

/// A list's fixes by word, for reading many words (a 30 h transcript) against a few fixes. Reads the
/// words themselves to count each one's `nth`: every word of the transcript is to be given, in time
/// order with ties in transcript order (a stable sort by start of the segments' words, as the editor
/// and the captions have them), a word to be left out (cut) included.
pub struct Fixes<'a> {
    by_word: HashMap<Key, &'a str>,
    /// Per track, the millisecond of the latest word read and how many of its words were read.
    seen: HashMap<usize, (i64, usize)>,
}

impl<'a> Fixes<'a> {
    pub fn new(fixes: &'a [Fix]) -> Self {
        Self {
            by_word: fixes.iter().map(|f| (f.key(), f.text.as_str())).collect(),
            seen: HashMap::new(),
        }
    }

    /// The fixed text of the next word, `w` heard on `track`, if the user fixed it.
    pub fn of(&mut self, track: usize, w: &Word) -> Option<&'a str> {
        if self.by_word.is_empty() {
            return None;
        }
        let ms = millis(w.start);
        let seen = self.seen.entry(track).or_insert((ms, 0));
        if seen.0 == ms {
            seen.1 += 1;
        } else {
            *seen = (ms, 1);
        }
        self.by_word.get(&(track, ms, seen.1 - 1)).copied()
    }

    /// What the next word, `w` heard on `track`, says: its fix, else what Whisper heard.
    pub fn text<'w>(&mut self, track: usize, w: &'w Word) -> &'w str
    where
        'a: 'w,
    {
        self.of(track, w).unwrap_or(&w.text)
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
            text: "miel".into(),
        }];
        let mut by = Fixes::new(&f);
        let other = word(54.944, "yes");
        assert_eq!(by.text(0, &ws[1]), "en");
        assert_eq!(by.text(2, &other), "yes");
        assert_eq!(by.text(0, &ws[2]), "miel");
        assert!(same_millisecond(54.944, 54.9444) && !same_millisecond(54.944, 54.945));
    }
}
