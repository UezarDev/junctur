//! Track analysis, the engine's first stage: what is on each audio track, which tracks are
//! copies, and a role guess per distinct track.

mod classify;
mod render;
mod window;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ffmpeg::{self, AudioStream};
use crate::signal::Activity;
use crate::{cache, Result};

pub use classify::{classify, FAINT_MEAN_DB, MIX_RESIDUAL, NEAR_DUPLICATE_CORR, NEAR_DUPLICATE_DB};
pub use window::window_starts;

#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    /// How many windows to sample across the file.
    pub windows: usize,
    pub window_seconds: f64,
    /// Cache directory; None means [`cache::default_dir`].
    pub cache_dir: Option<PathBuf>,
    /// Read and write the cache.
    pub use_cache: bool,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            windows: 8,
            window_seconds: 10.0,
            cache_dir: None,
            use_cache: true,
        }
    }
}

impl AnalyzeOptions {
    fn settings_key(&self) -> String {
        format!("w{}x{}", self.windows, self.window_seconds)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Everything together: the only substantial track, or the sum of other tracks.
    Mix,
    /// Mic or Discord voice.
    Voice,
    GamePc,
    /// Reserved: no heuristic in this phase tells music from game audio.
    Music,
    Silent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sameness {
    /// One physical track.
    Single,
    /// All members decode to the same samples.
    Exact,
    /// Members differ only slightly (correlation and level within thresholds).
    Near,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogicalTrack {
    /// Audio ordinals (0-based) of the physical tracks in this group; the first is the one later
    /// stages decode.
    pub members: Vec<usize>,
    pub sameness: Sameness,
    pub activity: Activity,
    pub role: Role,
    pub confidence: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackReport {
    pub file: PathBuf,
    pub duration: f64,
    /// Start times (seconds) of the sampled windows.
    pub windows: Vec<f64>,
    pub window_seconds: f64,
    pub tracks: Vec<AudioStream>,
    pub logical: Vec<LogicalTrack>,
    /// True when this report came from the cache rather than a fresh analysis.
    #[serde(skip)]
    pub from_cache: bool,
}

pub fn analyze(file: &Path, opts: &AnalyzeOptions) -> Result<TrackReport> {
    let dir = opts.cache_dir.clone().unwrap_or_else(cache::default_dir);
    let entry = cache::entry_path(&dir, "tracks", &cache::key(file, &opts.settings_key())?);
    if opts.use_cache {
        if let Some(mut report) = cache::read::<TrackReport>(&entry) {
            report.from_cache = true;
            return Ok(report);
        }
    }

    let probe = ffmpeg::probe(file)?;
    let (starts, len) = window_starts(probe.duration, opts.windows, opts.window_seconds);
    let mut per_track: Vec<Vec<Vec<f32>>> = Vec::with_capacity(probe.audio.len());
    for t in &probe.audio {
        per_track.push(
            starts
                .iter()
                .map(|&s| ffmpeg::decode_window(file, t.index, s, len))
                .collect::<Result<_>>()?,
        );
    }
    let samples = window::align(per_track, starts.len());
    let logical = classify(&samples);

    let report = TrackReport {
        file: file.to_path_buf(),
        duration: probe.duration,
        windows: starts,
        window_seconds: len,
        tracks: probe.audio,
        logical,
        from_cache: false,
    };
    if opts.use_cache {
        cache::write(&entry, &report)?;
    }
    Ok(report)
}
