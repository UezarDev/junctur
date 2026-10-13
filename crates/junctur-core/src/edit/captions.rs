//! How captions are cut into cues and styled, as the edit list keeps it (5a, 5d). Colours are per
//! speaker ([`super::SpeakerName::color`]); this is the rest of what shapes the caption file, and
//! only what an SRT carries into DaVinci Resolve: text, timing, colour, bold, italic and underline
//! (5f). 5d's research (`.planning/research/resolve-caption-styling.md`) found nothing else an SRT import
//! keeps: size, font family, outline, shadow, background and position are Resolve's subtitle
//! Track Style, which no import sets, and a position tag (`{\an8}`) shows as literal text. So they
//! are not settings here; offering them would promise what Resolve drops.
//!
//! Also the two switches (5d): export (`on`: a render writes the caption file) and the preview over
//! the picture in the editor, which are separate so captions can be watched without being written.
//! Every field has a serde default, so caption settings saved before a field existed (an older
//! edit list's line, or the caption defaults in `junctur/config.json`) still load.

use serde::{Deserialize, Serialize};

// `default`: a settings file or a command missing a field (one a later round adds) takes the built-in
// value instead of failing (2i); `deny_unknown_fields`: a misspelt one is an error rather than
// silently the default, as everywhere in the config file. The edit list's own `captions` line has
// its own parser (`format.rs`), which refuses unknown keys too.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Captions {
    /// Export: captions are optional; on, a render writes the caption file beside the video.
    pub on: bool,
    /// Show the captions over the picture in the editor as they play (5d). Preview only: it
    /// changes nothing in any file but this setting.
    pub preview: bool,
    /// Most characters on one line.
    pub chars: u32,
    /// Most lines in one cue, per speaker: people talking at once each get this many (5e).
    pub lines: u32,
    /// Longest a cue stays on screen, in seconds.
    pub seconds: f64,
    /// Every caption bold (`<b>`), which Resolve's SRT import applies and its own export writes.
    pub bold: bool,
    /// Every caption italic (`<i>`), likewise.
    pub italic: bool,
    /// Every caption underlined (`<u>`, 5f): Uezar's import of 5d's test file showed Resolve
    /// underlines in the speaker's colour.
    pub underline: bool,
}

/// 42 characters and two lines is the common broadcast and streaming limit, readable at a glance;
/// 6 s keeps a long sentence from sitting on screen after it is said.
impl Default for Captions {
    fn default() -> Self {
        Self {
            on: false,
            preview: false,
            chars: 42,
            lines: 2,
            seconds: 6.0,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

pub const CHARS: std::ops::RangeInclusive<u32> = 10..=80;
pub const LINES: std::ops::RangeInclusive<u32> = 1..=3;
pub const SECONDS: std::ops::RangeInclusive<f64> = 1.0..=15.0;

impl Captions {
    /// Why these settings cannot make sensible cues, if they cannot.
    pub fn check(&self) -> Result<(), String> {
        if !CHARS.contains(&self.chars) {
            return Err(format!(
                "characters per line must be {} to {}",
                CHARS.start(),
                CHARS.end()
            ));
        }
        if !LINES.contains(&self.lines) {
            return Err(format!(
                "lines per speaker must be {} to {}",
                LINES.start(),
                LINES.end()
            ));
        }
        if !SECONDS.contains(&self.seconds) {
            return Err(format!(
                "a caption lasts {} to {} s at most",
                SECONDS.start(),
                SECONDS.end()
            ));
        }
        Ok(())
    }
}

/// `#RRGGBB`, upper case, or `None` for anything else.
pub fn parse_color(s: &str) -> Option<String> {
    let hex = s.strip_prefix('#')?;
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_uppercase()))
}

/// A colour per speaker until the user picks one: distinct on dark video and on each other.
const PALETTE: [&str; 8] = [
    "#F2B33D", "#5FB3F2", "#8BD17C", "#E58BD8", "#F28B5F", "#7FD8D0", "#C9A3F2", "#D8D36F",
];

pub fn default_color(id: u32) -> String {
    PALETTE[(id.max(1) as usize - 1) % PALETTE.len()].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_limits() {
        assert_eq!(parse_color("#a1b2c3").as_deref(), Some("#A1B2C3"));
        assert_eq!(parse_color("a1b2c3"), None);
        assert_eq!(parse_color("#a1b2c"), None);
        assert_eq!(parse_color("#a1b2cg"), None);
        assert_eq!(default_color(1), "#F2B33D");
        assert_eq!(default_color(9), default_color(1));
        assert!(Captions::default().check().is_ok());
        let bad = |c: Captions| c.check().is_err();
        assert!(bad(Captions {
            chars: 5,
            ..Default::default()
        }));
        assert!(bad(Captions {
            lines: 0,
            ..Default::default()
        }));
        assert!(bad(Captions {
            seconds: 30.0,
            ..Default::default()
        }));
    }

    #[test]
    fn settings_saved_before_a_field_existed_still_load() {
        // 5a's four fields only, as an older build's caption defaults would hold them.
        let old: Captions =
            serde_json::from_str(r#"{"on":true,"chars":30,"lines":1,"seconds":4}"#).unwrap();
        assert_eq!(
            old,
            Captions {
                on: true,
                chars: 30,
                lines: 1,
                seconds: 4.0,
                ..Default::default()
            }
        );
        assert!(!old.preview && !old.bold && !old.italic && !old.underline);
        let all: Captions = serde_json::from_str(
            r#"{"on":false,"preview":true,"chars":42,"lines":2,"seconds":6,"bold":true,"italic":true}"#,
        )
        .unwrap();
        assert!(all.preview && all.bold && all.italic && !all.underline);
        // 5f's underline beside them.
        let u: Captions = serde_json::from_str(r#"{"underline":true}"#).unwrap();
        assert!(u.underline && !u.bold);
    }
}
