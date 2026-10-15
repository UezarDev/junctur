//! SigLIP 2 base through ONNX Runtime: a picture and a list of label prompts in, a share per label
//! out. Its two towers are separate models: the vision one runs for every frame, on the GPU when
//! the build has CUDA (400 to 800 ms a frame on a busy CPU); the text one only for a prompt never
//! embedded before, on the CPU, since its embeddings are kept ([`texts`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::runtime::{self, Device, DevicePref};
use crate::{models, output, Error, Result};

/// What `labels.json` names the model.
pub const NAME: &str = "siglip2-base-patch16-224";
/// The picture's side, in pixels.
pub const SIZE: u32 = 224;
/// Embedding length, both towers.
pub const DIM: usize = 768;
/// The text tower's input length: the tokenizer pads to it.
const TOKENS: usize = 64;
/// `<pad>`.
const PAD: i64 = 0;
/// Frames per run of the vision tower.
pub const BATCH: usize = 16;
/// The trained logit scale and bias, not in the ONNX export: `exp(logit_scale)` and `logit_bias` of
/// google/siglip2-base-patch16-224 (model.safetensors, commit 75de2d55).
const LOGIT_SCALE: f32 = 112.668_9;
const LOGIT_BIAS: f32 = -16.771_725;

/// The vision tower, open.
pub struct Vision {
    session: Session,
    pub device: Device,
}

impl Vision {
    /// Fetches the tower on first use (372 MB) and opens it where `pref` allows, the GPU first.
    pub fn open(cache_dir: &Path, pref: DevicePref) -> Result<Self> {
        let path = models::ensure(&models::SIGLIP_VISION, cache_dir)?;
        let (session, device) = runtime::open(&path, pref)?;
        Ok(Self { session, device })
    }

    /// Each frame's embedding, normalised; `frames` are [`SIZE`] by [`SIZE`] RGB, at most
    /// [`BATCH`] a call is best.
    pub fn embed(&mut self, frames: &[&[u8]]) -> Result<Vec<Vec<f32>>> {
        if frames.is_empty() {
            return Ok(Vec::new());
        }
        let side = SIZE as usize;
        let plane = side * side;
        let mut pixels = vec![0f32; frames.len() * 3 * plane];
        for (i, f) in frames.iter().enumerate() {
            if f.len() != plane * 3 {
                return Err(Error::Model(format!(
                    "a frame of {} bytes, not {SIZE}x{SIZE} RGB",
                    f.len()
                )));
            }
            // HWC bytes to CHW, (x / 255 - 0.5) / 0.5 as SigLIP's processor does.
            let at = i * 3 * plane;
            for (p, rgb) in f.chunks_exact(3).enumerate() {
                for c in 0..3 {
                    pixels[at + c * plane + p] = rgb[c] as f32 / 127.5 - 1.0;
                }
            }
        }
        let input = Tensor::from_array(([frames.len(), 3, side, side], pixels))?;
        let out = self.session.run(ort::inputs!["pixel_values" => input])?;
        pooled(&out["pooler_output"], frames.len())
    }
}

/// `n` rows of [`DIM`] from a `pooler_output`, each normalised.
fn pooled(v: &ort::value::DynValue, n: usize) -> Result<Vec<Vec<f32>>> {
    let (_, data) = v.try_extract_tensor::<f32>()?;
    if data.len() != n * DIM {
        return Err(Error::Model(format!(
            "SigLIP gave {} values for {n} embeddings",
            data.len()
        )));
    }
    // A NaN would score every label NaN, written as null, and `labels.json` would no longer read.
    if data.iter().any(|x| !x.is_finite()) {
        return Err(Error::Model(
            "SigLIP gave an embedding that is not a number".into(),
        ));
    }
    Ok(data.chunks_exact(DIM).map(normalised).collect())
}

fn normalised(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter().map(|x| x / n).collect()
}

/// The file the text embeddings are kept in, beside the models: one per model, keyed by prompt.
fn kept(cache_dir: &Path) -> PathBuf {
    models::dir(cache_dir).join("siglip2_base_224_labels.json")
}

/// Each prompt's embedding, normalised, in order. Read from the kept ones where they are; the
/// others embedded by the text tower (fetched on first need, 1.13 GB, opened on the CPU) and kept.
/// A kept file that does not read is embedded over.
pub fn texts(cache_dir: &Path, prompts: &[String]) -> Result<Vec<Vec<f32>>> {
    let path = kept(cache_dir);
    let mut known: BTreeMap<String, Vec<f32>> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    known.retain(|_, v| v.len() == DIM);
    let missing: Vec<&String> = prompts.iter().filter(|p| !known.contains_key(*p)).collect();
    if !missing.is_empty() {
        let tok = models::ensure(&models::SIGLIP_TOKENIZER, cache_dir)?;
        let model = models::ensure(&models::SIGLIP_TEXT, cache_dir)?;
        let tokenizer = std::fs::read_to_string(tok)?
            .parse::<Tokenizer>()
            .map_err(|e| Error::Model(format!("SigLIP tokenizer: {e}")))?;
        let mut session = runtime::open_on_cpu(&model, None)?;
        let mut ids = Vec::with_capacity(missing.len() * TOKENS);
        for p in &missing {
            ids.extend(token_ids(&tokenizer, p)?);
        }
        let input = Tensor::from_array(([missing.len(), TOKENS], ids))?;
        let out = session.run(ort::inputs!["input_ids" => input])?;
        for (p, e) in missing
            .iter()
            .zip(pooled(&out["pooler_output"], missing.len())?)
        {
            known.insert((*p).clone(), e);
        }
        // Two runs at once may each write theirs; a prompt lost that way is embedded again next time.
        output::write_replacing(&path, serde_json::to_vec(&known)?)?;
    }
    Ok(prompts.iter().map(|p| known[p].clone()).collect())
}

/// `prompt`'s ids as the text tower reads them: lowercased (SigLIP 2 was trained on lowercased
/// text), `<eos>` after, padded or cut to [`TOKENS`].
fn token_ids(tokenizer: &Tokenizer, prompt: &str) -> Result<Vec<i64>> {
    let enc = tokenizer
        .encode(prompt.to_lowercase(), true)
        .map_err(|e| Error::Model(format!("SigLIP tokenizer: {e}")))?;
    let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
    ids.resize(TOKENS, PAD);
    Ok(ids)
}

/// `image`'s share of each label of `texts` (embeddings, normalised): the softmax of SigLIP's
/// logits over the list, so they sum to 1. Not its own per-label sigmoid, which puts every label of
/// a game frame near 0 and so neither ranks them nor reads as a share.
pub fn shares(image: &[f32], texts: &[Vec<f32>]) -> Vec<f32> {
    let logits: Vec<f32> = texts
        .iter()
        .map(|t| LOGIT_SCALE * dot(image, t) + LOGIT_BIAS)
        .collect();
    let top = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = logits.iter().map(|l| (l - top).exp()).collect();
    let sum: f32 = e.iter().sum();
    e.iter().map(|x| x / sum).collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_sum_to_one_and_follow_the_nearest_label() {
        let img = normalised(&[1.0, 0.2, 0.0]);
        let texts = vec![
            normalised(&[0.0, 1.0, 0.0]),
            normalised(&[1.0, 0.0, 0.0]),
            normalised(&[0.0, 0.0, 1.0]),
        ];
        let s = shares(&img, &texts);
        assert!((s.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(s[1] > s[0] && s[0] > s[2]);
        assert!(shares(&img, &[]).is_empty());
    }

    #[test]
    fn the_logit_scale_is_the_trained_ones_exponent() {
        assert!((LOGIT_SCALE - 4.724_453_4_f32.exp()).abs() < 1e-3);
    }
}
