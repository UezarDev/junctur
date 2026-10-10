//! Speech regions on every track that carries speech, read as-is (no voice isolation: stems cost
//! ~19 GB per 30h stream, and Silero holds up on the raw mix), plus the energy envelope the cut
//! snapping uses. One decode pass per track feeds both.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::edit::{Envelope, ENVELOPE_HOP};
use crate::ffmpeg::{PcmReader, Span};
use crate::vad::{self, Region, Vad};
use crate::{models, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechTrack {
    /// 0-based audio ordinal.
    pub track: usize,
    pub regions: Vec<Region>,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Speech {
    pub tracks: Vec<SpeechTrack>,
    pub seconds: f64,
}

impl Speech {
    pub fn all_regions(&self) -> Vec<Region> {
        self.tracks.iter().flat_map(|t| t.regions.clone()).collect()
    }
}

/// 60 s = 1875 whole VAD frames, so frames never straddle reads.
const READ: usize = vad::RATE as usize * 60;
const HOP: usize = (vad::RATE as f64 * ENVELOPE_HOP) as usize;

pub fn detect(file: &Path, tracks: &[usize], cache_dir: &Path) -> Result<(Speech, Envelope)> {
    let model = models::ensure(&models::SILERO_VAD, cache_dir)?;
    let mut env: Vec<f32> = Vec::new();
    let mut out = Vec::new();
    let mut seconds: f64 = 0.0;
    for &track in tracks {
        let mut vad = Vad::open(&model)?;
        let mut r = PcmReader::open(file, track, vad::RATE, 1, Span::default())?;
        let (mut probs, mut n) = (Vec::new(), 0usize);
        loop {
            let x = r.read(READ)?;
            if x.is_empty() {
                break;
            }
            probs.extend(vad.probs(&x)?);
            for (i, hop) in x.chunks(HOP).enumerate() {
                let rms = (hop.iter().map(|s| s * s).sum::<f32>() / hop.len() as f32).sqrt();
                let k = n / HOP + i;
                if k >= env.len() {
                    env.resize(k + 1, 0.0);
                }
                // The loudest speech track at each instant: a cut must be quiet on all of them.
                env[k] = env[k].max(rms);
            }
            n += x.len();
        }
        r.finish()?;
        let len = n as f64 / vad::RATE as f64;
        seconds = seconds.max(len);
        let regions = vad::regions(&probs, 0.0);
        out.push(SpeechTrack {
            track,
            share: vad::share(&regions, len),
            regions,
        });
    }
    Ok((
        Speech {
            tracks: out,
            seconds,
        },
        Envelope(env),
    ))
}

pub fn save_envelope(path: &Path, env: &Envelope) -> Result<()> {
    let bytes: Vec<u8> = env.0.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn load_envelope(path: &Path) -> Result<Envelope> {
    let b = std::fs::read(path)?;
    Ok(Envelope(
        b.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    ))
}
