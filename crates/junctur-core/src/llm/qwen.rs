//! A Qwen3 model through ONNX Runtime (1.7B or 4B, [`Size`]): a chat prompt in, the reply's text
//! out, greedy. The export takes the whole key/value cache in and gives it back a token longer each
//! step (fp16, a key and a value per layer of 8 heads by 128), so a step feeds only the newest
//! token; the prompt goes in a chunk at a time, since the export gives logits for every position it
//! is fed (151 936 floats each).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use ort::memory::Allocator;
use ort::session::{Session, SessionInputValue};
use ort::value::{DynTensor, DynValue, Tensor, TensorElementType};
use tokenizers::Tokenizer;

use crate::runtime::{self, Device, DevicePref};
use crate::{models, Error, Result};

/// Key/value heads and their width, the same in both sizes; the layers are counted from the inputs.
const KV_HEADS: usize = 8;
const HEAD_DIM: usize = 128;
/// `<|im_end|>` and `<|endoftext|>`: the reply is over.
const ENDS: [u32; 2] = [151_645, 151_643];
/// Prompt tokens fed per run: the logits of 128 positions are 78 MB.
const CHUNK: usize = 128;

/// Which Qwen3 reads requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    /// Qwen3 1.7B, 1.43 GB: the default.
    #[default]
    Small,
    /// Qwen3 4B Instruct 2507, 2.89 GB: opted into when the small one misreads.
    Large,
}

impl Size {
    pub fn name(self) -> &'static str {
        match self {
            Size::Small => "qwen3-1.7b",
            Size::Large => "qwen3-4b",
        }
    }

    /// The model's files, the graph first.
    pub fn files(self) -> &'static [&'static models::Model] {
        match self {
            Size::Small => &[&models::QWEN3_1_7B],
            Size::Large => &[
                &models::QWEN3_4B,
                &models::QWEN3_4B_DATA,
                &models::QWEN3_4B_DATA_1,
            ],
        }
    }

    /// Megabytes to download.
    pub fn megabytes(self) -> u64 {
        match self {
            Size::Small => 1426,
            Size::Large => 2890,
        }
    }

    /// The hybrid 1.7B thinks unless its reply opens with an empty think block; the 2507 4B never
    /// thinks.
    pub fn thinks(self) -> bool {
        self == Size::Small
    }

    /// Every file in the cache already.
    pub fn downloaded(self, cache_dir: &Path) -> bool {
        self.files()
            .iter()
            .chain([&&models::QWEN3_1_7B_TOKENIZER])
            .all(|m| models::dir(cache_dir).join(m.file).is_file())
    }
}

pub struct Qwen {
    session: Session,
    tokenizer: Tokenizer,
    layers: usize,
    /// The export takes `position_ids` (the 1.7B one; the 4B one works them out itself).
    positions: bool,
    pub device: Device,
}

impl Qwen {
    /// Fetches the model on first use and opens it where `pref` allows, the GPU first.
    pub fn open(cache_dir: &Path, size: Size, pref: DevicePref, stop: &AtomicBool) -> Result<Self> {
        let tok = models::ensure_or_stop(&models::QWEN3_1_7B_TOKENIZER, cache_dir, stop)?;
        let mut paths = Vec::new();
        for m in size.files() {
            paths.push(models::ensure_or_stop(m, cache_dir, stop)?);
        }
        let tokenizer = std::fs::read_to_string(tok)?
            .parse::<Tokenizer>()
            .map_err(|e| Error::Model(format!("Qwen tokenizer: {e}")))?;
        let (session, device) = runtime::open(&paths[0], pref)?;
        let layers = session
            .inputs()
            .iter()
            .filter(|i| i.name().starts_with("past_key_values."))
            .count()
            / 2;
        let positions = session.inputs().iter().any(|i| i.name() == "position_ids");
        if layers == 0 {
            return Err(Error::Model("the model takes no key/value cache".into()));
        }
        Ok(Self {
            session,
            tokenizer,
            layers,
            positions,
            device,
        })
    }

    /// The reply to `prompt` (the chat template's text, ending where the reply starts), at most
    /// `most` tokens, ended early once `done` says the text so far is enough.
    pub fn reply(
        &mut self,
        prompt: &str,
        most: usize,
        done: &dyn Fn(&str) -> bool,
    ) -> Result<String> {
        let enc = self
            .tokenizer
            .encode(prompt, false)
            .map_err(|e| Error::Model(format!("Qwen tokenizer: {e}")))?;
        let ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        if ids.is_empty() {
            return Err(Error::Model("an empty prompt".into()));
        }
        let empty = || -> Result<DynValue> {
            Ok(DynTensor::new(
                &Allocator::default(),
                TensorElementType::Float16,
                [1usize, KV_HEADS, 0, HEAD_DIM],
            )?
            .into_dyn())
        };
        let mut past: Vec<(String, DynValue)> = Vec::with_capacity(self.layers * 2);
        for i in 0..self.layers {
            for kv in ["key", "value"] {
                past.push((format!("past_key_values.{i}.{kv}"), empty()?));
            }
        }
        let mut seen = 0usize;
        let mut next = self.step(&ids, &mut seen, &mut past)?;
        let mut out: Vec<u32> = Vec::new();
        while out.len() < most && !ENDS.contains(&next) {
            out.push(next);
            if out.len().is_multiple_of(4) && done(&self.text(&out)?) {
                break;
            }
            next = self.step(&[next as i64], &mut seen, &mut past)?;
        }
        self.text(&out)
    }

    fn text(&self, ids: &[u32]) -> Result<String> {
        self.tokenizer
            .decode(ids, true)
            .map_err(|e| Error::Model(format!("Qwen tokenizer: {e}")))
    }

    /// Feeds `ids` after the `seen` tokens the cache holds (in chunks of [`CHUNK`]); the likeliest
    /// next token.
    fn step(
        &mut self,
        ids: &[i64],
        seen: &mut usize,
        past: &mut [(String, DynValue)],
    ) -> Result<u32> {
        let mut best = 0u32;
        for chunk in ids.chunks(CHUNK) {
            let n = chunk.len();
            let total = *seen + n;
            let mut inputs: Vec<(String, SessionInputValue)> = vec![
                (
                    "input_ids".into(),
                    Tensor::from_array(([1usize, n], chunk.to_vec()))?.into(),
                ),
                (
                    "attention_mask".into(),
                    Tensor::from_array(([1usize, total], vec![1i64; total]))?.into(),
                ),
            ];
            if self.positions {
                let at: Vec<i64> = (*seen as i64..total as i64).collect();
                inputs.push((
                    "position_ids".into(),
                    Tensor::from_array(([1usize, n], at))?.into(),
                ));
            }
            inputs.extend(past.iter().map(|(name, v)| (name.clone(), v.into())));
            let mut out = self.session.run(inputs)?;
            let (shape, logits) = out["logits"].try_extract_tensor::<f32>()?;
            let vocab = *shape.last().unwrap_or(&0) as usize;
            if vocab == 0 || logits.len() != n * vocab {
                return Err(Error::Model("Qwen gave no logits".into()));
            }
            best = argmax(&logits[(n - 1) * vocab..]) as u32;
            for (name, value) in past.iter_mut() {
                let present = name.replace("past_key_values", "present");
                *value = out
                    .remove(&present)
                    .ok_or_else(|| Error::Model(format!("Qwen gave no {present}")))?;
            }
            *seen = total;
        }
        Ok(best)
    }
}

fn argmax(v: &[f32]) -> usize {
    v.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i)
}
