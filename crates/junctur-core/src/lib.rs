//! Junctur engine library.
//!
//! Track analysis comes first: what is on each audio track of a recording, which tracks are copies,
//! and each distinct track's role (speech detection refines Phase 1's loudness guess). Mix-only
//! tracks can then be split into a vocal stem and the rest ([`separate`]), with speech regions and
//! speakers found on the vocal stem. Later stages consume [`tracks::TrackReport`] and [`Stems`].

pub mod cache;
pub mod diarize;
pub mod error;
pub mod ffmpeg;
pub mod models;
pub mod runtime;
pub mod separate;
pub mod signal;
pub mod tracks;
pub mod vad;

pub use error::{Error, Result};
pub use separate::{separate, SeparateOptions, Stems};
pub use tracks::{analyze, AnalyzeOptions, TrackReport};
