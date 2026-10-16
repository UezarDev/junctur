//! Junctur engine library: [`pipeline`] runs a recording's stages in order (tracks, speech,
//! speakers, [`edit`], [`labels`], [`transcribe`], [`keep`]), each cached and resumable; [`export`] and [`render`] turn
//! the edit list into an FCPXML timeline or a video.

pub mod cache;
pub mod config;
pub mod diarize;
pub mod edit;
pub mod error;
pub mod export;
pub mod ffmpeg;
pub mod hash;
pub mod hidden;
pub mod keep;
pub mod labels;
pub mod like;
pub mod models;
pub mod moments;
pub mod output;
pub mod paths;
pub mod pipeline;
pub mod render;
pub mod resolve;
pub mod rollcall;
pub mod runtime;
pub mod separate;
pub mod signal;
pub mod spans;
#[cfg(test)]
mod test_support;
pub mod text;
pub mod tracks;
pub mod transcribe;
pub mod translate;
pub mod vad;
pub mod voices;
pub mod waves;
pub mod wordlist;

pub use error::{Error, Result};
pub use separate::{separate, SeparateOptions, Stems};
pub use tracks::{analyze, AnalyzeOptions, TrackReport};
