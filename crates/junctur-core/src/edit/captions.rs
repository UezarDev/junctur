//! How captions are cut into cues and styled, as the edit list keeps it (colours are per speaker).
//! The SRT carries only what Resolve's subtitle import keeps (text, timing, colour, bold, italic,
//! underline); the look, layout, highlight and animation reach only Text+ clips and the preview.

use serde::{Deserialize, Serialize};

use super::caption_text::{self, Case, CensorStyle, Replace};
use super::worded::worded;

// `default`: settings saved before a field existed still load; `deny_unknown_fields`: a misspelt
// field is an error rather than silently the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Captions {
    /// Export: captions are optional; on, a render writes the caption file beside the video.
    pub on: bool,
    /// Show the captions over the picture in the editor as they play. Apart from `on` so captions
    /// can be watched without being written.
    pub preview: bool,
    /// Most characters on one line.
    pub chars: u32,
    /// Most lines in one cue, per speaker: people talking at once each get this many.
    pub lines: u32,
    /// Most words in one cue, per speaker like `lines`; 0 for no limit, 1 for one word at a time
    /// (short-form clips).
    pub words: u32,
    /// Longest a cue stays on screen, in seconds.
    pub seconds: f64,
    /// Every caption bold (`<b>`), which Resolve's SRT import applies and its own export writes.
    pub bold: bool,
    /// Every caption italic (`<i>`), likewise.
    pub italic: bool,
    /// Every caption underlined (`<u>`), which Resolve's SRT import draws in the speaker's colour.
    pub underline: bool,
    /// Font family of the Text+ captions and the preview.
    pub font: String,
    /// Text height (the font's em) in % of the picture height.
    pub size: f64,
    /// From the bottom of the picture to the bottom of the caption, in % of its height.
    pub position: f64,
    /// An outline round the letters, `outline_width` steps thick, in `outline_color`.
    pub outline: bool,
    pub outline_width: u32,
    pub outline_color: String,
    /// Where people talking at once sit.
    pub layout: Layout,
    /// The speaker (by name) who always keeps the normal caption spot when people talk at once;
    /// empty for none. A name, not an id: ids differ per recording, while a remembered voice
    /// names a person the same everywhere, so it works in the defaults too.
    pub protagonist: String,
    /// The Fusion Title clip in Resolve's media pool every caption is a copy of; empty for
    /// Junctur's own Text+; picked from a list of the open project's Fusion Titles.
    pub template: String,
    /// The named template keeps its own look: Junctur sets only each caption's text, timing,
    /// speaker colour, highlight and animation. Off, Junctur's look wins. Ignored with Junctur's
    /// own template, whose look is these settings.
    pub keep_look: bool,
    /// What a send to Resolve does with clips Junctur did not place that are in its captions' way
    /// on its tracks: ask (the default), or the choice remembered for this recording. Kept on
    /// the recording's own `captions` line; never a default (`to_config` in the app stores it as
    /// `ask`), so one recording's choice does not reach the next.
    pub existing_clips: ExistingClips,
    /// The text options ([`caption_text`]): what captions show, never the transcript.
    pub case: Case,
    /// Punctuation kept (the default) or removed.
    pub punctuation: bool,
    /// Words or phrases to hide, whole words, case aside.
    pub censor: Vec<String>,
    pub censor_style: CensorStyle,
    /// Find and replace, applied before case and censoring.
    pub replace: Vec<Replace>,
    /// The language captions are shown in ([`crate::translate`]), a code of
    /// [`crate::translate::LANGUAGES`]; empty (the default) for the language spoken. The text
    /// options above apply to the translated words.
    pub translate: String,
    /// The word being said lit up, word by word, in Resolve and the preview.
    pub highlight: bool,
    /// Its colour, `#RRGGBB`; empty (the default) for the speaker's own colour, the caption then
    /// white so the lit word shows.
    pub highlight_color: String,
    /// How a caption comes in and goes out, in Resolve and the preview.
    pub animation: Animation,
    /// How long the animation takes, in seconds.
    pub animation_length: f64,
}

worded! {
    /// A caption's in and out animation: auto-subs' animated caption's three, one at a time.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub enum Animation {
        #[default]
        None = "none",
        Fade = "fade",
        /// Grows in from smaller, fading.
        Pop = "pop",
        /// Rises in from lower, fading.
        Slide = "slide",
    }
}

worded! {
    /// What a send to Resolve does with clips Junctur did not place (no tag of its own) that sit
    /// where its new captions go on the tracks it places on (as auto-subs' replace / skip / new
    /// track). Its word is also the request's.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub enum ExistingClips {
        /// Change nothing and ask the user, with the count found.
        #[default]
        Ask = "ask",
        /// Delete those clips (only the ones a caption overlaps), then place.
        Replace = "replace",
        /// Leave them; the captions they overlap are not placed.
        Skip = "skip",
        /// Place that speaker's new captions on a track of their own beside it (`<track> (2)`).
        NewTrack = "newTrack",
    }
}

worded! {
    /// Where the captions of people talking at once go.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub enum Layout {
        /// Each later speaker above the one before.
        #[default]
        Stacked = "stacked",
        /// Side by side across the picture.
        Side = "side",
        /// One at the bottom, the next at the top of the picture.
        TopBottom = "topBottom",
    }
}

/// A Resolve title's name as Junctur compares it: spaces, tabs and line breaks round it
/// dropped, ASCII only. The Junctur script in Resolve trims a clip's name by the very same rule
/// (`junctur.lua`'s `title_name`), so a title listed in the picker is the one a send finds;
/// Rust's own `trim` also drops Unicode spaces, which Lua's patterns do not know.
pub fn title_name(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
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
            keep_look: false,
            existing_clips: ExistingClips::Ask,
            case: Case::AsSpoken,
            punctuation: true,
            censor: Vec::new(),
            censor_style: CensorStyle::Middle,
            replace: Vec::new(),
            translate: String::new(),
            highlight: false,
            highlight_color: String::new(),
            animation: Animation::None,
            // auto-subs' animated caption's own default.
            animation_length: 0.2,
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
/// Seconds: shorter is a jump, longer eats into a short caption's time on screen.
pub const ANIMATION_LENGTH: std::ops::RangeInclusive<f64> = 0.05..=1.0;

/// Every caption's bold, italic and underline, as the SRT and the Resolve send each read them.
/// Kept as three fields of [`Captions`], which are the config file's and the page's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl TextStyle {
    /// The SRT tags that are on, outermost first, in the order Resolve's own export writes them.
    pub fn tags(self) -> impl DoubleEndedIterator<Item = &'static str> {
        [("b", self.bold), ("i", self.italic), ("u", self.underline)]
            .into_iter()
            .filter_map(|(t, on)| on.then_some(t))
    }
}

impl Captions {
    pub fn style(&self) -> TextStyle {
        TextStyle {
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
        }
    }

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
        if !self.highlight_color.is_empty() && parse_color(&self.highlight_color).is_none() {
            return Err(format!(
                "{:?} is not a colour like #FFD400",
                self.highlight_color
            ));
        }
        if !ANIMATION_LENGTH.contains(&self.animation_length) {
            return Err(format!(
                "an animation takes {} to {} s",
                ANIMATION_LENGTH.start(),
                ANIMATION_LENGTH.end()
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
        // A clip's tag carries the template's name between `|`s (`resolve::job`): one inside
        // it could make two different tags read alike.
        if self.template.contains('|') {
            return Err("the template name cannot hold a |".into());
        }
        if !self.translate.is_empty() && !crate::translate::known(&self.translate) {
            return Err(format!(
                "captions cannot be translated into {:?}",
                self.translate
            ));
        }
        caption_text::check(&self.censor, &self.replace)
    }

    /// Whether the template's own look is kept: only with a template of the user's named.
    pub fn keeps_look(&self) -> bool {
        self.keep_look && !title_name(&self.template).is_empty()
    }

    /// Whether Resolve needs the animated caption template.
    pub fn animated(&self) -> bool {
        self.highlight || self.animation != Animation::None
    }

    /// A speaker's caption colours under the highlight: the text's and the lit word's. With
    /// the highlight colour left to the speaker's, the text is white and the word the speaker's;
    /// a chosen colour lights the word over the speaker's colour. White on white would show
    /// nothing, so a speaker with no colour of their own gets yellow.
    pub fn highlight_colors(&self, speaker: &str) -> (String, String) {
        let speaker = parse_color(speaker).unwrap_or_else(|| WHITE.into());
        match parse_color(&self.highlight_color) {
            Some(word) => (speaker, word),
            None if speaker == WHITE => (WHITE.into(), UNCOLOURED_HIGHLIGHT.into()),
            None => (WHITE.into(), speaker),
        }
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

const WHITE: &str = "#FFFFFF";
/// The lit word of a speaker with no colour (or a white one).
const UNCOLOURED_HIGHLIGHT: &str = "#FFD400";

/// `#RRGGBB`, upper case, or `None` for anything else.
pub fn parse_color(s: &str) -> Option<String> {
    let hex = s.strip_prefix('#')?;
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_uppercase()))
}

/// Colours in [`default_color`]'s palette.
pub const PALETTE_SIZE: u32 = 8;

/// A colour per speaker until the user picks one: distinct on dark video and on each other.
const PALETTE: [&str; PALETTE_SIZE as usize] = [
    "#F2B33D", "#5FB3F2", "#8BD17C", "#E58BD8", "#F28B5F", "#7FD8D0", "#C9A3F2", "#D8D36F",
];

pub fn default_color(id: u32) -> String {
    PALETTE[(id.max(1) as usize - 1) % PALETTE.len()].to_string()
}

#[cfg(test)]
mod tests;
