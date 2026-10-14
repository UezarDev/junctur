//! Speech detection with Silero VAD v5: per-frame speech probability, then speech regions.
//!
//! The model is stateful (an RNN state carried between frames) and, like the reference wrapper, each
//! 512-sample frame is fed with the previous frame's last 64 samples in front of it.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

use crate::runtime;
use crate::Result;

pub const RATE: u32 = 16_000;
pub const FRAME: usize = 512;
const CONTEXT: usize = 64;
pub const FRAME_SECONDS: f64 = FRAME as f64 / RATE as f64;

/// Region thresholds, the reference wrapper's defaults: enter speech at 0.5, leave below 0.35,
/// ignore blips under 250 ms and close gaps under 100 ms.
const ON: f32 = 0.5;
const OFF: f32 = 0.35;
const MIN_SPEECH_S: f64 = 0.25;
const MIN_GAP_S: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub start: f64,
    pub end: f64,
}

pub struct Vad {
    session: Session,
    /// The model's inputs, made once and written in place frame by frame: a 30 h track is 3.4
    /// million frames, too many to build three tensors for each.
    input: Tensor<f32>,
    state: Tensor<f32>,
    sr: Tensor<i64>,
}

impl Vad {
    /// The model is tiny and runs frame by frame, so GPU transfer costs more than it saves: CPU
    /// always, and one thread: a frame is too small to split, and tracks run side by side.
    pub fn open(model: &Path) -> Result<Self> {
        let session = runtime::open_on_cpu(model, Some(1))?;
        Ok(Self {
            session,
            input: Tensor::from_array(([1usize, CONTEXT + FRAME], vec![0.0; CONTEXT + FRAME]))?,
            state: Tensor::from_array(([2usize, 1, 128], vec![0.0; 2 * 128]))?,
            sr: Tensor::from_array((Vec::<usize>::new(), vec![RATE as i64]))?,
        })
    }

    pub fn reset(&mut self) {
        self.input.extract_tensor_mut().1.fill(0.0);
        self.state.extract_tensor_mut().1.fill(0.0);
    }

    /// Speech probability per 512-sample frame of 16 kHz mono; a short tail is zero-padded.
    pub fn probs(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let mut out = Vec::with_capacity(samples.len() / FRAME + 1);
        for chunk in samples.chunks(FRAME) {
            // The input is the previous frame's last CONTEXT samples, then this frame.
            let x = self.input.extract_tensor_mut().1;
            x.copy_within(FRAME.., 0);
            x[CONTEXT..CONTEXT + chunk.len()].copy_from_slice(chunk);
            x[CONTEXT + chunk.len()..].fill(0.0);
            let res = self.session.run(ort::inputs![
                "input" => &self.input,
                "state" => &self.state,
                "sr" => &self.sr
            ])?;
            out.push(res["output"].try_extract_tensor::<f32>()?.1[0]);
            let next = res["stateN"].try_extract_tensor::<f32>()?.1;
            self.state.extract_tensor_mut().1.copy_from_slice(next);
        }
        Ok(out)
    }
}

/// Speech regions (seconds from `offset`) from per-frame probabilities, with hysteresis.
pub fn regions(probs: &[f32], offset: f64) -> Vec<Region> {
    let mut raw: Vec<Region> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &p) in probs.iter().enumerate() {
        match start {
            None if p >= ON => start = Some(i),
            Some(s) if p < OFF => {
                raw.push(span(s, i, offset));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        raw.push(span(s, probs.len(), offset));
    }
    // The raw spans come from one forward scan, so a joined one's end is the later span's.
    debug_assert!(raw.windows(2).all(|w| w[0].end <= w[1].start));
    let mut merged = crate::spans::joined(raw, crate::spans::Join::Within(MIN_GAP_S));
    merged.retain(|r| r.end - r.start >= MIN_SPEECH_S);
    merged
}

fn span(a: usize, b: usize, offset: f64) -> Region {
    Region {
        start: offset + a as f64 * FRAME_SECONDS,
        end: offset + b as f64 * FRAME_SECONDS,
    }
}

/// Share of `total` seconds covered by `regions`.
pub fn share(regions: &[Region], total: f64) -> f64 {
    if total <= 0.0 {
        return 0.0;
    }
    // `0.0 +`: an empty float sum is -0.0, which would print as "-0%".
    ((0.0 + regions.iter().map(|r| r.end - r.start).sum::<f64>()) / total).min(1.0)
}

/// Scale a quiet buffer up so its loud frames sit near -20 dBFS (at most +60 dB). The VAD is not
/// level-invariant: a Discord feed recorded at -70 dB reads as silence unless it is brought up.
pub fn normalize(samples: &[f32]) -> Vec<f32> {
    let mut rms: Vec<f32> = samples
        .chunks(FRAME)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if rms.is_empty() {
        return Vec::new();
    }
    rms.sort_by(f32::total_cmp);
    let loud = rms[(rms.len() * 95 / 100).min(rms.len() - 1)];
    if loud <= 0.0 {
        return samples.to_vec();
    }
    let gain = (0.1 / loud).clamp(1.0, 1000.0);
    samples
        .iter()
        .map(|x| (x * gain).clamp(-1.0, 1.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteresis_merges_gaps_and_drops_blips() {
        // 10 frames speech, 2 frames gap (64 ms < 100 ms), 10 frames speech, silence, 3-frame blip.
        let mut p = vec![0.9; 10];
        p.extend([0.1; 2]);
        p.extend([0.9; 10]);
        p.extend([0.0; 40]);
        p.extend([0.9; 3]);
        p.extend([0.0; 5]);
        let r = regions(&p, 0.0);
        assert_eq!(r.len(), 1);
        assert!((r[0].end - 22.0 * FRAME_SECONDS).abs() < 1e-9);
    }

    #[test]
    fn between_thresholds_keeps_state() {
        let p = [0.4, 0.6, 0.4, 0.4, 0.4, 0.4, 0.4, 0.4, 0.4, 0.4, 0.2];
        let r = regions(&p, 10.0);
        assert_eq!(r.len(), 1);
        assert!((r[0].start - (10.0 + FRAME_SECONDS)).abs() < 1e-9);
    }

    #[test]
    fn share_and_normalize_edges() {
        assert_eq!(share(&[], 0.0), 0.0);
        let r = [Region {
            start: 0.0,
            end: 5.0,
        }];
        assert_eq!(share(&r, 10.0), 0.5);
        assert!(normalize(&[]).is_empty());
        assert_eq!(normalize(&[0.0; 10]), vec![0.0; 10]);
        let quiet = vec![0.0005f32; 2048];
        assert!((normalize(&quiet)[0] - 0.1).abs() < 1e-4);
    }
}
