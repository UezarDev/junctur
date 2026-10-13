//! How captions are cut into cues and styled, as the edit list keeps it (5a, 5d). Colours are per
//! speaker ([`super::SpeakerName::color`]); this is the rest of what shapes the captions. The SRT
//! carries only what Resolve's subtitle import keeps: text, timing, colour, bold, italic and
//! underline (5d's research, `.planning/research/resolve-caption-styling.md`, and 5f). Font, size,
//! position and outline reach Resolve only as Text+ clips sent through its scripting API (5g), so
//! those fields drive that and the preview and the SRT ignores them, as it does the overlap layout
//! and the protagonist (5g). The text options (5j: case, punctuation, censoring, find and replace)
//! and the translation (5k) change the words every output shows, the SRT included, never the
//! transcript.
//!
//! Also the two switches (5d): export (`on`: a render writes the caption file) and the preview over
//! the picture in the editor, which are separate so captions can be watched without being written.
//! Every field has a serde default, so caption settings saved before a field existed (an older
//! edit list's line, or the caption defaults in `junctur/config.json`) still load.

use serde::{Deserialize, Serialize};

use super::caption_text::{self, Case, CensorStyle, Replace};

// `default`: a settings file or a command missing a field (one a later round adds) takes the built-in
// value instead of failing (2i); `deny_unknown_fields`: a misspelt one is an error rather than
// silently the default, as everywhere in the config file. The edit list's own `captions` line has
// its own parser (`format.rs`), which refuses unknown keys too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Most words in one cue, per speaker like `lines`; 0 for no limit, 1 for one word at a time
    /// (5i, short-form clips).
    pub words: u32,
    /// Longest a cue stays on screen, in seconds.
    pub seconds: f64,
    /// Every caption bold (`<b>`), which Resolve's SRT import applies and its own export writes.
    pub bold: bool,
    /// Every caption italic (`<i>`), likewise.
    pub italic: bool,
    /// Every caption underlined (`<u>`, 5f): Uezar's import of 5d's test file showed Resolve
    /// underlines in the speaker's colour.
    pub underline: bool,
    /// Font family of the Text+ captions and the preview (5g).
    pub font: String,
    /// Text height (the font's em) in % of the picture height (5g).
    pub size: f64,
    /// From the bottom of the picture to the bottom of the caption, in % of its height (5g).
    pub position: f64,
    /// An outline round the letters (5g), `outline_width` steps thick, in `outline_color`.
    pub outline: bool,
    pub outline_width: u32,
    pub outline_color: String,
    /// Where people talking at once sit (5g).
    pub layout: Layout,
    /// The speaker (by name) who always keeps the normal caption spot when people talk at once;
    /// empty for none (5g). A name, not an id: ids differ per recording, while a remembered voice
    /// names a person the same everywhere, so it works in the defaults too.
    pub protagonist: String,
    /// The Fusion Title clip in Resolve's media pool every caption is a copy of; empty for
    /// Junctur's own Text+ (5g).
    pub template: String,
    /// The text options (5j, [`caption_text`]): what captions show, never the transcript.
    pub case: Case,
    /// Punctuation kept (the default) or removed.
    pub punctuation: bool,
    /// Words or phrases to hide, whole words, case aside.
    pub censor: Vec<String>,
    pub censor_style: CensorStyle,
    /// Find and replace, applied before case and censoring.
    pub replace: Vec<Replace>,
    /// The language captions are shown in (5k, [`crate::translate`]), a code of
    /// [`crate::translate::LANGUAGES`]; empty (the default) for the language spoken. The text
    /// options above apply to the translated words.
    pub translate: String,
}

/// Where the captions of people talking at once go (5g, Uezar: "stacked (up down) or sideways
/// first, or one above and one below").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// Each later speaker above the one before.
    #[default]
    Stacked,
    /// Side by side across the picture.
    Side,
    /// One at the bottom, the next at the top of the picture.
    TopBottom,
}

impl Layout {
    /// The word on the edit list's `captions` line, the same as in the config file.
    pub fn word(self) -> &'static str {
        match self {
            Layout::Stacked => "stacked",
            Layout::Side => "side",
            Layout::TopBottom => "topBottom",
        }
    }

    pub fn from_word(w: &str) -> Option<Self> {
        [Layout::Stacked, Layout::Side, Layout::TopBottom]
            .into_iter()
            .find(|l| l.word() == w)
    }
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
            words: 0,
            seconds: 6.0,
            bold: false,
            italic: false,
            underline: false,
            font: "Open Sans".into(),
            size: 4.5,
            position: 6.0,
            outline: true,
            outline_width: 2,
            outline_color: "#000000".into(),
            layout: Layout::Stacked,
            protagonist: String::new(),
            template: String::new(),
            case: Case::AsSpoken,
            punctuation: true,
            censor: Vec::new(),
            censor_style: CensorStyle::Middle,
            replace: Vec::new(),
            translate: String::new(),
        }
    }
}

pub const CHARS: std::ops::RangeInclusive<u32> = 10..=80;
pub const LINES: std::ops::RangeInclusive<u32> = 1..=3;
/// Words per speaker when limited (0 is no limit): 20 is about three full lines, past which the
/// line limit cuts first.
pub const WORDS: std::ops::RangeInclusive<u32> = 1..=20;
pub const SECONDS: std::ops::RangeInclusive<f64> = 1.0..=15.0;
pub const SIZE: std::ops::RangeInclusive<f64> = 2.0..=15.0;
pub const POSITION: std::ops::RangeInclusive<f64> = 0.0..=45.0;
pub const OUTLINE_WIDTH: std::ops::RangeInclusive<u32> = 1..=10;

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
        if self.words != 0 && !WORDS.contains(&self.words) {
            return Err(format!(
                "words per caption must be {} to {}, or 0 for no limit",
                WORDS.start(),
                WORDS.end()
            ));
        }
        if !SECONDS.contains(&self.seconds) {
            return Err(format!(
                "a caption lasts {} to {} s at most",
                SECONDS.start(),
                SECONDS.end()
            ));
        }
        if !SIZE.contains(&self.size) {
            return Err(format!(
                "caption size must be {} to {} % of the picture height",
                SIZE.start(),
                SIZE.end()
            ));
        }
        if !POSITION.contains(&self.position) {
            return Err(format!(
                "caption position must be {} to {} % of the picture height",
                POSITION.start(),
                POSITION.end()
            ));
        }
        if !OUTLINE_WIDTH.contains(&self.outline_width) {
            return Err(format!(
                "outline thickness must be {} to {}",
                OUTLINE_WIDTH.start(),
                OUTLINE_WIDTH.end()
            ));
        }
        if parse_color(&self.outline_color).is_none() {
            return Err(format!(
                "{:?} is not a colour like #000000",
                self.outline_color
            ));
        }
        if self.font.trim().is_empty() {
            return Err("a font is needed".into());
        }
        // Quoted on the edit list's line (format.rs), so a quote or a line break would end them.
        for (what, s) in [
            ("font", &self.font),
            ("protagonist", &self.protagonist),
            ("template", &self.template),
        ] {
            if s.contains('"') || s.chars().any(char::is_control) {
                return Err(format!("the {what} name cannot hold quotes or line breaks"));
            }
        }
        if !self.translate.is_empty() && !crate::translate::known(&self.translate) {
            return Err(format!(
                "captions cannot be translated into {:?}",
                self.translate
            ));
        }
        caption_text::check(&self.censor, &self.replace)
    }

    /// The id of the speaker in `names` the protagonist names, if any (case and spaces aside).
    pub fn protagonist_in(&self, names: &[super::SpeakerName]) -> Option<u32> {
        let want = self.protagonist.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        names
            .iter()
            .find(|n| n.name.trim().to_lowercase() == want)
            .map(|n| n.id)
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
            words: 21,
            ..Default::default()
        }));
        assert!(Captions {
            words: 1,
            ..Default::default()
        }
        .check()
        .is_ok());
        assert!(bad(Captions {
            seconds: 30.0,
            ..Default::default()
        }));
        // 5g's look.
        for c in [
            Captions {
                size: 1.0,
                ..Default::default()
            },
            Captions {
                position: 50.0,
                ..Default::default()
            },
            Captions {
                outline_width: 0,
                ..Default::default()
            },
            Captions {
                outline_color: "black".into(),
                ..Default::default()
            },
            Captions {
                font: " ".into(),
                ..Default::default()
            },
            Captions {
                protagonist: "Ana \"A\"".into(),
                ..Default::default()
            },
            Captions {
                template: "a\nb".into(),
                ..Default::default()
            },
        ] {
            assert!(bad(c));
        }
    }

    #[test]
    fn the_protagonist_is_found_by_name() {
        let name = |id: u32, name: &str| super::super::SpeakerName {
            id,
            name: name.into(),
            track: None,
            color: "#FFFFFF".into(),
        };
        let names = [name(1, "Ana"), name(2, "Lucía")];
        let c = |p: &str| Captions {
            protagonist: p.into(),
            ..Default::default()
        };
        assert_eq!(c("ana").protagonist_in(&names), Some(1));
        assert_eq!(c(" LUCÍA ").protagonist_in(&names), Some(2));
        assert_eq!(c("Bo").protagonist_in(&names), None);
        assert_eq!(c("").protagonist_in(&names), None);
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
        // 5f's settings load 5g's look at its defaults; 5g's own fields read back.
        assert_eq!(u.font, "Open Sans");
        assert_eq!((u.size, u.position, u.outline), (4.5, 6.0, true));
        assert_eq!(u.layout, Layout::Stacked);
        let g: Captions = serde_json::from_str(
            r##"{"font":"Noto Sans","size":6,"outlineWidth":3,"outlineColor":"#202020","layout":"topBottom","protagonist":"Ana","template":"My Title"}"##,
        )
        .unwrap();
        assert_eq!(g.layout, Layout::TopBottom);
        assert_eq!((g.size, g.outline_width), (6.0, 3));
        assert_eq!(
            (g.protagonist.as_str(), g.template.as_str()),
            ("Ana", "My Title")
        );
        assert!(serde_json::from_str::<Captions>(r#"{"layout":"diagonal"}"#).is_err());
        // 5g's settings load 5i's word limit off; its own value reads back.
        assert_eq!(g.words, 0);
        let i: Captions = serde_json::from_str(r#"{"words":1}"#).unwrap();
        assert_eq!(i.words, 1);
        // 5i's settings load 5j's text as spoken; its own fields read back.
        assert_eq!((i.case, i.punctuation), (Case::AsSpoken, true));
        assert!(i.censor.is_empty() && i.replace.is_empty());
        let j: Captions = serde_json::from_str(
            r#"{"case":"title","punctuation":false,"censor":["damn"],"censorStyle":"whole","replace":[{"from":"gonna","to":"going to"}]}"#,
        )
        .unwrap();
        assert_eq!(
            (j.case, j.punctuation, j.censor_style),
            (Case::Title, false, CensorStyle::Whole)
        );
        assert_eq!(j.censor, ["damn"]);
        assert_eq!(j.replace[0].to, "going to");
        assert!(j.check().is_ok());
        let bad: Captions = serde_json::from_str(r#"{"censor":["a|b"]}"#).unwrap();
        assert!(bad.check().is_err());
        // 5j's settings load 5k's captions untranslated; its own value reads back and is checked.
        assert_eq!(j.translate, "");
        let k: Captions = serde_json::from_str(r#"{"translate":"fr"}"#).unwrap();
        assert_eq!(k.translate, "fr");
        assert!(k.check().is_ok());
        let bad: Captions = serde_json::from_str(r#"{"translate":"klingon"}"#).unwrap();
        assert!(bad.check().is_err());
    }
}
