//! The caption text options (5j): case, punctuation, censored words and find-and-replace pairs.
//! They change only what captions show (the SRT, Send to Resolve, the preview), never the
//! transcript, so turning one off brings the spoken words back; `export::caption_text` applies
//! them. A module of their own because they are the only list-valued caption settings, and their
//! checks are about characters the `captions` line and an SRT cannot carry.

use serde::{Deserialize, Serialize};

/// How caption letters are cased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Case {
    #[default]
    AsSpoken,
    Upper,
    Lower,
    /// The first letter of each word upper, the rest as spoken.
    Title,
}

impl Case {
    /// The word on the edit list's `captions` line, the same as in the config file.
    pub fn word(self) -> &'static str {
        match self {
            Case::AsSpoken => "asSpoken",
            Case::Upper => "upper",
            Case::Lower => "lower",
            Case::Title => "title",
        }
    }

    pub fn from_word(w: &str) -> Option<Self> {
        [Case::AsSpoken, Case::Upper, Case::Lower, Case::Title]
            .into_iter()
            .find(|c| c.word() == w)
    }
}

/// How a censored word is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CensorStyle {
    /// First and last letter kept: `f**k`.
    #[default]
    Middle,
    /// Every letter masked: `****`.
    Whole,
}

impl CensorStyle {
    pub fn word(self) -> &'static str {
        match self {
            CensorStyle::Middle => "middle",
            CensorStyle::Whole => "whole",
        }
    }

    pub fn from_word(w: &str) -> Option<Self> {
        [CensorStyle::Middle, CensorStyle::Whole]
            .into_iter()
            .find(|c| c.word() == w)
    }
}

/// A find-and-replace pair: `from`, whole words or a phrase, case aside; an empty `to` removes them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Replace {
    pub from: String,
    pub to: String,
}

/// Most entries in the censor list, and in the replacements.
pub const MAX_ENTRIES: usize = 200;
/// Longest entry, in characters.
pub const MAX_ENTRY: usize = 100;
/// The `captions` line's separators between entries and between a pair's halves.
pub const ENTRY_SEP: char = '|';
pub const PAIR_SEP: char = '>';

/// Why an entry cannot be kept, if it cannot: `"` and control characters would end the line's
/// quotes, `|` and `>` are its separators, and `<` or `{` reads as a tag in an SRT (Whisper's own
/// are stripped by `clean`; a replacement goes in as written).
fn check_entry(what: &str, s: &str, must_match: bool) -> Result<(), String> {
    if s.chars().count() > MAX_ENTRY {
        return Err(format!("{what} {s:?} is over {MAX_ENTRY} characters"));
    }
    if let Some(bad) = s
        .chars()
        .find(|&c| matches!(c, '"' | '|' | '>' | '<' | '{' | '}') || c.is_control())
    {
        return Err(format!("{what} {s:?} cannot hold {bad:?}"));
    }
    // A word with no letter or digit has no whole word to match.
    if must_match && !s.chars().any(char::is_alphanumeric) {
        return Err(format!("{what} {s:?} needs a letter or digit"));
    }
    Ok(())
}

/// The censor list and the replacements, checked for the `captions` line and the SRT.
pub fn check(censor: &[String], replace: &[Replace]) -> Result<(), String> {
    if censor.len() > MAX_ENTRIES || replace.len() > MAX_ENTRIES {
        return Err(format!(
            "at most {MAX_ENTRIES} censored words and {MAX_ENTRIES} replacements"
        ));
    }
    for w in censor {
        check_entry("the censored word", w, true)?;
    }
    for r in replace {
        check_entry("the text to find", &r.from, true)?;
        check_entry("the replacement", &r.to, false)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(from: &str, to: &str) -> Replace {
        Replace {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn entries_the_line_or_an_srt_cannot_carry_are_refused() {
        assert!(check(
            &["damn".into(), "two words".into()],
            &[pair("gonna", "going to"), pair("um", "")]
        )
        .is_ok());
        for bad in ["a|b", "a>b", "<i>", "{x}", "say \"hi\"", "a\nb", "...", ""] {
            assert!(check(&[bad.into()], &[]).is_err(), "{bad}");
            assert!(check(&[], &[pair(bad, "x")]).is_err(), "{bad}");
        }
        assert!(check(&[], &[pair("x", "<b>")]).is_err());
        assert!(check(&["x".repeat(MAX_ENTRY + 1)], &[]).is_err());
        assert!(check(&vec!["x".into(); MAX_ENTRIES + 1], &[]).is_err());
    }

    #[test]
    fn words_round_trip() {
        for c in [Case::AsSpoken, Case::Upper, Case::Lower, Case::Title] {
            assert_eq!(Case::from_word(c.word()), Some(c));
        }
        for c in [CensorStyle::Middle, CensorStyle::Whole] {
            assert_eq!(CensorStyle::from_word(c.word()), Some(c));
        }
        assert_eq!(Case::from_word("shout"), None);
    }
}
