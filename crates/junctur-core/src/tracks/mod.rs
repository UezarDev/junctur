//! Track analysis, the engine's first stage: what is on each audio track, which tracks are
//! copies, and a role guess per distinct track.

mod classify;
mod render;
pub mod roles;
mod window;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ffmpeg::{self, AudioStream};
use crate::signal::Activity;
use crate::vad::{self, Vad};
use crate::{cache, models, Result};
use roles::Speech;

pub use classify::{
    classify, classify_with, FAINT_MEAN_DB, MIX_RESIDUAL, NEAR_DUPLICATE_CORR, NEAR_DUPLICATE_DB,
};
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
    /// Run speech detection to decide roles (downloads the VAD model on first use).
    pub speech: bool,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            windows: 8,
            window_seconds: 10.0,
            cache_dir: None,
            use_cache: true,
            speech: true,
        }
    }
}

impl AnalyzeOptions {
    fn settings_key(&self) -> String {
        format!("w{}x{}s{}", self.windows, self.window_seconds, self.speech)
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
    /// None when speech detection was off, unavailable, or the track is silent.
    #[serde(default)]
    pub speech: Option<Speech>,
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
    /// Why speech detection did not run, when it was asked for and failed (e.g. offline).
    #[serde(default)]
    pub speech_error: Option<String>,
    /// True when this report came from the cache rather than a fresh analysis.
    #[serde(skip)]
    pub from_cache: bool,
}

fn entry(file: &Path, opts: &AnalyzeOptions) -> Result<PathBuf> {
    let dir = opts.cache_dir.clone().unwrap_or_else(cache::default_dir);
    Ok(cache::entry_path(
        &dir,
        &cache::key(file, &opts.settings_key())?,
    ))
}

/// The report an earlier [`analyze`] with `opts`' settings left in the cache, without analysing:
/// `None` when there is none, or it does not parse. For a caller that only wants the roles if they
/// are known (the app's player, 2e), where `analyze` would decode the whole file.
pub fn cached(file: &Path, opts: &AnalyzeOptions) -> Option<TrackReport> {
    let bytes = std::fs::read(entry(file, opts).ok()?).ok()?;
    let mut report = serde_json::from_slice::<TrackReport>(&bytes).ok()?;
    report.from_cache = true;
    Some(report)
}

pub fn analyze(file: &Path, opts: &AnalyzeOptions) -> Result<TrackReport> {
    let entry = entry(file, opts)?;
    if opts.use_cache {
        if let Some(report) = cached(file, opts) {
            return Ok(report);
        }
    }

    let probe = ffmpeg::probe(file)?;
    let (starts, len) = window_starts(probe.duration, opts.windows, opts.window_seconds);
    // One ffmpeg per window decodes every track (it opens the whole MP4 each time).
    let indices: Vec<usize> = probe.audio.iter().map(|t| t.index).collect();
    let mut per_track: Vec<Vec<Vec<f32>>> = vec![Vec::with_capacity(starts.len()); indices.len()];
    for &s in &starts {
        for (t, w) in ffmpeg::decode_windows(file, &indices, s, len)?
            .into_iter()
            .enumerate()
        {
            per_track[t].push(w);
        }
    }
    let samples = window::align(per_track, starts.len());
    let dir_for_models = opts.cache_dir.clone().unwrap_or_else(cache::default_dir);
    let (logical, speech_error) = if opts.speech {
        match models::ensure(&models::SILERO_VAD, &dir_for_models).and_then(|m| Vad::open(&m)) {
            Ok(mut vad) => {
                let mut failed = None;
                let l = classify_with(&samples, &mut |s| {
                    vad.reset();
                    match vad.probs(&vad::normalize(s)) {
                        Ok(p) => Some(roles::measure(&p, s)),
                        Err(e) => {
                            failed = Some(e.to_string());
                            None
                        }
                    }
                });
                (l, failed)
            }
            Err(e) => (classify(&samples), Some(e.to_string())),
        }
    } else {
        (classify(&samples), None)
    };

    let report = TrackReport {
        file: file.to_path_buf(),
        duration: probe.duration,
        windows: starts,
        window_seconds: len,
        tracks: probe.audio,
        logical,
        speech_error,
        from_cache: false,
    };
    if opts.use_cache {
        // A cache that cannot be written is not a failed analysis.
        if let Some(parent) = entry.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&entry, serde_json::to_vec_pretty(&report)?);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_reads_only_what_analyze_left() {
        let d = std::env::temp_dir().join(format!("junctur-tracks-cached-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let opts = AnalyzeOptions {
            cache_dir: Some(d.join("cache")),
            ..Default::default()
        };
        assert!(cached(&file, &opts).is_none(), "nothing cached yet");
        let report = TrackReport {
            file: file.clone(),
            duration: 10.0,
            windows: vec![0.0],
            window_seconds: 10.0,
            tracks: vec![],
            logical: vec![],
            speech_error: None,
            from_cache: false,
        };
        let path = entry(&file, &opts).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
        let got = cached(&file, &opts).unwrap();
        assert!(got.from_cache);
        assert_eq!(got.duration, 10.0);
        // Other settings are another entry.
        let other = AnalyzeOptions {
            windows: 3,
            ..opts.clone()
        };
        assert!(cached(&file, &other).is_none());
        std::fs::write(&path, b"{ broken").unwrap();
        assert!(cached(&file, &opts).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
