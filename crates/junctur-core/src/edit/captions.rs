//! How captions are cut into cues, as the edit list keeps it (5a). Colours are per speaker
//! ([`super::SpeakerName::color`]); this is the rest of what shapes the caption file, and only what
//! an SRT carries into DaVinci Resolve: text, timing and colour. Size, font and position do not
//! survive an SRT import (5a's check), so they are not settings here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Captions {
    /// Captions are optional: on, a render writes the caption file beside the video.
    pub on: bool,
    /// Most characters on one line.
    pub chars: u32,
    /// Most lines in one cue, per speaker: people talking at once each get this many (5e).
    pub lines: u32,
    /// Longest a cue stays on screen, in seconds.
    pub seconds: f64,
}

/// 42 characters and two lines is the common broadcast and streaming limit, readable at a glance;
/// 6 s keeps a long sentence from sitting on screen after it is said.
impl Default for Captions {
    fn default() -> Self {
        Self {
            on: false,
            chars: 42,
            lines: 2,
            seconds: 6.0,
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
}
