//! Junctur engine library.
//!
//! Phase 1 holds only track analysis: before any other stage runs, work out what is on each audio
//! track of a recording, collapse copies, and guess each track's role. Later stages (VAD, Whisper,
//! the edit list) consume [`tracks::TrackReport`] so they process each distinct track once.

pub mod cache;
pub mod error;
pub mod ffmpeg;
pub mod signal;
pub mod tracks;

pub use error::{Error, Result};
pub use tracks::{analyze, AnalyzeOptions, TrackReport};
