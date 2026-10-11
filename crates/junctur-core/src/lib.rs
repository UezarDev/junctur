//! Junctur engine library.
//!
//! Stage one is track analysis: work out what is on each audio track of a recording, collapse
//! copies, and guess each track's role. Stage two transcribes the speech tracks: the VAD cuts them
//! into short speech chunks and Whisper (large-v3-turbo unless large-v3 is asked for) hears each.
//! The edit list will consume both.

pub mod cache;
pub mod config;
pub mod error;
pub mod ffmpeg;
pub mod signal;
mod tool;
pub mod tracks;
pub mod transcribe;
pub mod vad;
pub mod whisper;

pub use error::{Error, Result};
pub use tracks::{analyze, AnalyzeOptions, TrackReport};
pub use transcribe::{transcribe, TranscribeOptions, Transcript};
