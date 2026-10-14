//! The edit list: which ranges of the source to keep, as a readable text file the user can diff,
//! commit and hand-edit, plus who speaks when (named speakers and their turns) and a score per short
//! time window for the later Review/Swipe modes.
//!
//! It is the one piece of state a project owns; everything else in the project folder is a cache
//! that can be rebuilt. The source itself is never touched.

mod caption_text;
mod captions;
mod captions_line;
mod fixes;
mod format;
mod frames;
mod reduce;
mod save;
mod scores;
mod speakers;
mod splice;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use caption_text::{Case, CensorStyle, Replace};
pub use captions::{default_color, parse_color, Captions, Layout};
pub use fixes::{
    check_fix, check_span, same_millisecond, set_fix, set_span, Fix, Fixed, Fixes, WordAt, MAX_FIX,
    MAX_SPAN,
};
pub use format::{parse, FORMAT_VERSION};
pub use frames::{frame_ranges, frame_span, frames_ceil, frames_floor};
pub use reduce::{keep_ranges, snap_boundary, word_span, Envelope, ENVELOPE_HOP};
pub use save::{amend, backup_path, hold, save, unstamped, Amended, Held, Saved};
pub use scores::windows;
pub use speakers::{
    check_name, default_name, Labeller, Person, SpeakerName, Speakers, Turn, MAX_NAME,
};
pub use splice::{cut_span, keep_span};

/// A kept stretch of the source, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub start: f64,
    pub end: f64,
}

impl Range {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }
}

/// How the first automatic reduction is made. Stored in the file so a result can be reproduced.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CutSettings {
    /// Kept before each speech region, and after it.
    pub pad_before: f64,
    pub pad_after: f64,
    /// Pauses shorter than this stay in: cutting every breath makes speech sound chopped.
    pub merge_gap: f64,
    /// How far a boundary may move to land on the quietest point.
    pub snap: f64,
    /// Crossfade at each cut, applied by the renderer.
    pub crossfade: f64,
    /// Length of a score window.
    pub window: f64,
}

impl Default for CutSettings {
    fn default() -> Self {
        Self {
            pad_before: 0.25,
            pad_after: 0.35,
            merge_gap: 1.0,
            snap: 0.15,
            crossfade: 0.03,
            window: 5.0,
        }
    }
}

/// One score window. `score` is what Review/Swipe will rank by; today it is the speech share, the
/// only signal so far. Highlight signals (laughter, game volume, chat) become more columns.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub start: f64,
    pub score: f64,
    pub speech: f64,
    pub words: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditList {
    pub source: PathBuf,
    pub duration: f64,
    pub settings: CutSettings,
    pub keep: Vec<Range>,
    /// Who speaks when; empty when the project has no speakers stage result.
    pub speakers: Speakers,
    /// How the caption file is cut into cues, and whether captions are on; `None` when the list has
    /// no `captions` line, and then it follows the user's defaults (2i, `config::captions`), which
    /// the caller resolves before cutting cues.
    pub captions: Option<Captions>,
    /// Words the user fixed (5h), and runs of words fixed as one text (5x), in time order: shown in
    /// place of what Whisper heard.
    pub fixes: Vec<Fix>,
    pub windows: Vec<Window>,
}

impl EditList {
    pub fn kept(&self) -> f64 {
        // `0.0 +`: an empty float sum is -0.0, which prints as "-0".
        0.0 + self.keep.iter().map(Range::len).sum::<f64>()
    }

    pub fn to_text(&self) -> String {
        format::write(self)
    }
}
