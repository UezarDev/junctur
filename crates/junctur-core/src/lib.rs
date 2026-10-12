//! Junctur engine library.
//!
//! Track analysis comes first: what is on each audio track of a recording, which tracks are copies,
//! and each distinct track's role (speech detection refines Phase 1's loudness guess). Mix-only
//! tracks can then be split into a vocal stem and the rest ([`separate`]), with speech regions and
//! speakers found on the vocal stem (opt-in: the main pipeline reads tracks as they are).
//!
//! [`pipeline`] runs the stages for a recording in order (tracks, speech, speakers, [`transcribe`],
//! [`edit`]), each cached and resumable, and [`export`] turns the resulting edit list into an FCPXML
//! timeline. [`render`] turns it into a video, re-encoding only around the cuts. The speakers stage
//! tells voices apart on the recording itself (per track, or by voice on the mix), not on a stem,
//! naming the people the user enrolled once ([`voices`]) and finding where two talk at once.

pub mod cache;
pub mod config;
pub mod diarize;
pub mod edit;
pub mod error;
pub mod export;
pub mod ffmpeg;
pub mod models;
pub mod output;
pub mod pipeline;
pub mod render;
pub mod runtime;
pub mod separate;
pub mod signal;
pub mod tracks;
pub mod transcribe;
pub mod vad;
pub mod voices;

pub use error::{Error, Result};
pub use separate::{separate, SeparateOptions, Stems};
pub use tracks::{analyze, AnalyzeOptions, TrackReport};
