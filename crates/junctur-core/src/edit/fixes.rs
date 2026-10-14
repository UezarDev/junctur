//! Words the user fixed (5h): a misheard word's right spelling, kept in the edit list as `fix
//! <track> <start> <text>` and shown wherever the word is (the transcript panel, the caption
//! preview, the SRT).
//!
//! In the edit list because a fix is the user's edit: saved with every other edit, kept through
//! re-cuts, backed up before a list is replaced. Not beside the transcript, which is a cache a
//! re-run replaces whole. A word is named by its track and its start to the millisecond (the list's
//! precision): stable while a transcript grows piece by piece (2j) and the same in the app and the
//! CLI, where word indices are not.

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
    /// What it says.
    pub text: String,
}

impl Fix {
    fn key(&self) -> (usize, i64) {
        key(self.track, self.start)
    }

    /// Both name the same word.
    pub fn same_word(&self, other: &Fix) -> bool {
        self.key() == other.key()
    }
}

/// A word's start to the millisecond, as the list writes it ([`written`]).
fn key(track: usize, start: f64) -> (usize, i64) {
    (track, millis(start))
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

/// Sets the fix of the word heard as `heard` on `track` at `start` to `text` (through
/// [`check_fix`]); empty text, or the heard text again, removes it. Kept in time order. Whether
/// anything changed.
pub fn set_fix(
    fixes: &mut Vec<Fix>,
    track: usize,
    start: f64,
    heard: &str,
    text: &str,
) -> Result<bool, String> {
    let text = check_fix(text)?;
    let heard = check_fix(heard).unwrap_or_default();
    let k = key(track, start);
    let at = fixes.iter().position(|f| f.key() == k);
    let before = fixes.clone();
    match at {
        Some(i) if text.is_empty() || text == heard => {
            fixes.remove(i);
        }
        Some(i) => fixes[i].text = text,
        None if text.is_empty() || text == heard => {}
        None => {
            fixes.push(Fix { track, start, text });
            sort(fixes);
        }
    }
    Ok(*fixes != before)
}

/// Marks the word heard as `heard` on `track` at `start` as checked by the user (5o): a fix line with
/// the heard text, so every fix line means "the user vouched for this word" and the transcript panel
/// flags none of them. Not a line kind of its own: an older build reads it as a fix that changes
/// nothing, where a new kind would need a format version it refuses. A word with a fix already is
/// left as it is. Whether anything changed.
pub fn confirm_fix(
    fixes: &mut Vec<Fix>,
    track: usize,
    start: f64,
    heard: &str,
) -> Result<bool, String> {
    let k = key(track, start);
    if fixes.iter().any(|f| f.key() == k) {
        return Ok(false);
    }
    let text = check_fix(heard)?;
    if text.is_empty() {
        return Err("this word has no text to confirm".into());
    }
    fixes.push(Fix { track, start, text });
    sort(fixes);
    Ok(true)
}

/// The list's order: by start, then track.
pub(super) fn sort(fixes: &mut [Fix]) {
    fixes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.track.cmp(&b.track)));
}

/// A list's fixes by word, for reading many words (a 30 h transcript) against a few fixes.
pub struct Fixes<'a> {
    by_word: HashMap<(usize, i64), &'a str>,
}

impl<'a> Fixes<'a> {
    pub fn new(fixes: &'a [Fix]) -> Self {
        Self {
            by_word: fixes.iter().map(|f| (f.key(), f.text.as_str())).collect(),
        }
    }

    /// The fixed text of `w` heard on `track`, if the user fixed it.
    pub fn of(&self, track: usize, w: &Word) -> Option<&'a str> {
        if self.by_word.is_empty() {
            return None;
        }
        self.by_word.get(&key(track, w.start)).copied()
    }

    /// What `w` heard on `track` says: its fix, else what Whisper heard.
    pub fn text<'w>(&self, track: usize, w: &'w Word) -> &'w str
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
            p_min: None,
        }
    }

    #[test]
    fn a_fix_is_set_changed_and_removed() {
        let mut f = Vec::new();
        assert!(set_fix(&mut f, 1, 12.34, " Wezar,", " Uezar, ").unwrap());
        assert_eq!(f[0].text, "Uezar,");
        assert!(
            !set_fix(&mut f, 1, 12.34, " Wezar,", "Uezar,").unwrap(),
            "same"
        );
        assert!(set_fix(&mut f, 0, 3.0, "a", "b").unwrap());
        assert_eq!(
            f.iter().map(|x| x.track).collect::<Vec<_>>(),
            [0, 1],
            "in time"
        );
        // The heard text again, or nothing, takes the fix away.
        assert!(set_fix(&mut f, 1, 12.34, " Wezar,", "Wezar,").unwrap());
        assert!(set_fix(&mut f, 0, 3.0, "a", "  ").unwrap());
        assert!(f.is_empty());
        assert!(
            !set_fix(&mut f, 0, 3.0, "a", "a").unwrap(),
            "nothing to fix"
        );
        // One line, not too long.
        assert!(set_fix(&mut f, 0, 3.0, "a", "Tsun\nheto").unwrap());
        assert_eq!(f[0].text, "Tsun heto");
        assert!(set_fix(&mut f, 0, 3.0, "a", &"x".repeat(MAX_FIX + 1)).is_err());
    }

    /// 5o: a confirmed word is a fix to its own text; a fixed word stays fixed, and the heard text
    /// typed in again (5h) takes the confirmation away like any fix.
    #[test]
    fn a_word_is_confirmed_as_a_fix_to_what_was_heard() {
        let mut f = Vec::new();
        assert!(confirm_fix(&mut f, 1, 12.34, " Uezar, ").unwrap());
        assert_eq!(f[0].text, "Uezar,");
        assert!(!confirm_fix(&mut f, 1, 12.34, "Uezar,").unwrap(), "once");
        assert_eq!(Fixes::new(&f).text(1, &word(12.34, "Uezar,")), "Uezar,");
        assert!(set_fix(&mut f, 0, 3.0, "Wezar", "Uezar").unwrap());
        assert!(!confirm_fix(&mut f, 0, 3.0, "Wezar").unwrap());
        assert_eq!(f[0].text, "Uezar", "the fix stands");
        assert!(set_fix(&mut f, 1, 12.34, "Uezar,", "Uezar,").unwrap());
        assert_eq!(f.len(), 1);
        assert!(confirm_fix(&mut f, 0, 9.0, "  ").is_err());
    }

    #[test]
    fn a_half_millisecond_start_is_written_as_its_key() {
        assert_eq!(written(12.3125), "12.313");
        assert_eq!(written(2.5), "2.500");
        assert_eq!(written(0.0004), "0.000");
        assert_eq!(written(-1.5), "-1.500");
        let start: f64 = written(12.3125).parse().unwrap();
        assert_eq!(key(0, start), key(0, 12.3125));
    }

    #[test]
    fn a_word_is_found_by_track_and_millisecond() {
        let fixes = vec![Fix {
            track: 1,
            start: 312.34,
            text: "Uezar".into(),
        }];
        let by = Fixes::new(&fixes);
        // A start summed from a chunk's and a token's time, off in the last bits.
        let heard = word(300.0 + 12.34, "Wezar");
        assert_eq!(by.text(1, &heard), "Uezar");
        assert_eq!(by.text(0, &heard), "Wezar", "another track");
        assert_eq!(by.text(1, &word(312.341, "Wezar")), "Wezar", "another word");
    }
}
