//! The translation model run through ONNX Runtime (5k): a sentence in, its translation out.
//!
//! An encoder–decoder: the source sentence, behind its language's token and ended by `</s>`, is
//! encoded once; the decoder then writes the translation a token at a time, starting from `</s>`
//! and the target language's token, until it writes `</s>`. Greedy (the likeliest token each step),
//! not the five-way beam search the model was published with: a beam is five decoders' work for a
//! caption-length sentence, and greedy reads the same on short sentences. The decoder file has its
//! key/value cache merged in (`use_cache_branch`), so each step feeds only the newest token.

use std::path::Path;

use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};
use tokenizers::Tokenizer;

use crate::runtime::{self, Device, DevicePref};
use crate::{models, Error, Result};

/// `</s>`: ends the source, starts and ends the translation.
const EOS: u32 = 2;
/// `<pad>`, after a shorter source in a batch.
const PAD: i64 = 1;
/// Decoder layers, each with a key and value cache for itself and for the encoder.
const LAYERS: usize = 12;
const HEADS: usize = 16;
const HEAD_DIM: usize = 64;
/// Source tokens past this are dropped: a caption sentence is a few dozen (sentences are cut at
/// [`crate::export::sentences`]'s word limit), and the model was trained on at most 1024.
const MAX_SOURCE: usize = 250;

pub struct Model {
    tokenizer: Tokenizer,
    encoder: Session,
    decoder: Session,
    pub device: Device,
}

impl Model {
    /// Fetches the model on first use (632 MB) into `cache_dir` and opens it.
    pub fn open(cache_dir: &Path, pref: DevicePref) -> Result<Self> {
        let tok = models::ensure(&models::TRANSLATE_TOKENIZER, cache_dir)?;
        let enc = models::ensure(&models::TRANSLATE_ENCODER, cache_dir)?;
        let dec = models::ensure(&models::TRANSLATE_DECODER, cache_dir)?;
        let tokenizer = tokenizer(&std::fs::read_to_string(tok)?)?;
        let (encoder, device) = runtime::open(&enc, pref)?;
        let (decoder, _) = runtime::open(&dec, pref)?;
        Ok(Self {
            tokenizer,
            encoder,
            decoder,
            device,
        })
    }

    /// The token of a language code (`__fr__`).
    fn language(&self, code: &str) -> Result<u32> {
        self.tokenizer
            .token_to_id(&format!("__{code}__"))
            .ok_or_else(|| Error::Model(format!("the translation model has no language {code:?}")))
    }

    /// `text`, said in `from`, in `to` (the model's language codes).
    pub fn translate(&mut self, text: &str, from: &str, to: &str) -> Result<String> {
        Ok(self.translate_batch(&[(text, from)], to)?.remove(0))
    }

    /// Each `(text, said in)` of `batch` in `to`, in order. A batch is decoded together, a row
    /// each: a step reads the weights once for every row, which is most of what a step costs on a
    /// CPU, so a batch of several takes little longer than one sentence (measured in 5k). Rows may
    /// be in different languages, each behind its own language's token.
    pub fn translate_batch(&mut self, batch: &[(&str, &str)], to: &str) -> Result<Vec<String>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let rows = batch.len();
        let mut sources: Vec<Vec<i64>> = Vec::with_capacity(rows);
        for (text, from) in batch {
            let enc = self
                .tokenizer
                .encode(*text, false)
                .map_err(|e| Error::Model(format!("translation tokenizer: {e}")))?;
            let mut ids = vec![self.language(from)? as i64];
            ids.extend(enc.get_ids().iter().take(MAX_SOURCE).map(|&i| i as i64));
            ids.push(EOS as i64);
            sources.push(ids);
        }
        // Shorter rows padded at the end, masked out of the attention.
        let n = sources.iter().map(Vec::len).max().unwrap_or(0);
        let mut ids = Vec::with_capacity(rows * n);
        let mut mask = Vec::with_capacity(rows * n);
        for s in &sources {
            ids.extend(
                s.iter()
                    .copied()
                    .chain(std::iter::repeat_n(PAD, n - s.len())),
            );
            mask.extend((0..n).map(|i| i64::from(i < s.len())));
        }
        let mask = Tensor::from_array(([rows, n], mask))?;
        let hidden = {
            let ids = Tensor::from_array(([rows, n], ids))?;
            let mut out = self
                .encoder
                .run(ort::inputs!["input_ids" => ids, "attention_mask" => &mask])?;
            out.remove("last_hidden_state")
                .ok_or_else(|| Error::Model("the encoder gave no hidden state".into()))?
        };

        // A translation runs about as long as its source; past twice that it is looping.
        let longest = 2 * n + 10;
        let target = self.language(to)? as i64;
        let mut tokens: Vec<Vec<u32>> = vec![Vec::new(); rows];
        let mut finished = vec![false; rows];
        let mut next: Vec<i64> = (0..rows).flat_map(|_| [EOS as i64, target]).collect();
        // The cache by input name: empty before the first step, then the decoder's own grows a
        // token a step, while the encoder's is computed once, in the first step: the cached branch
        // does not pass it through to its outputs.
        let empty = || Tensor::from_array(([rows, HEADS, 0, HEAD_DIM], Vec::<f32>::new()));
        let mut past: Vec<(String, DynValue)> = Vec::with_capacity(LAYERS * 4);
        for name in cache_names("past_key_values") {
            past.push((name, empty()?.into_dyn()));
        }
        let mut cached = false;
        for _ in 0..longest {
            let k = next.len() / rows;
            let mut inputs: Vec<(String, SessionInputValue)> = vec![
                ("encoder_attention_mask".into(), (&mask).into()),
                (
                    "input_ids".into(),
                    Tensor::from_array(([rows, k], std::mem::take(&mut next)))?.into(),
                ),
                ("encoder_hidden_states".into(), (&hidden).into()),
                (
                    "use_cache_branch".into(),
                    Tensor::from_array(([1usize], vec![cached]))?.into(),
                ),
            ];
            inputs.extend(past.iter().map(|(n, v)| (n.clone(), v.into())));
            let mut out = self.decoder.run(inputs)?;
            let (shape, logits) = out["logits"].try_extract_tensor::<f32>()?;
            let vocab = *shape.last().unwrap_or(&0) as usize;
            if vocab == 0 || logits.len() != rows * k * vocab {
                return Err(Error::Model("the decoder gave no logits".into()));
            }
            // Each row's last position. A finished row goes on being fed `</s>`, its output unused:
            // rows start together, and the decoder has no padding mask of its own.
            for (r, done) in finished.iter_mut().enumerate() {
                let at = (r * k + k - 1) * vocab;
                let best = argmax(&logits[at..at + vocab]) as u32;
                if !*done {
                    if best == EOS {
                        *done = true;
                    } else {
                        tokens[r].push(best);
                    }
                }
                next.push(if *done { EOS as i64 } else { best as i64 });
            }
            for (name, value) in &mut past {
                if cached && name.contains(".encoder.") {
                    continue;
                }
                let present = name.replace("past_key_values", "present");
                *value = out
                    .remove(&present)
                    .ok_or_else(|| Error::Model(format!("the decoder gave no {present}")))?;
            }
            cached = true;
            if finished.iter().all(|&d| d) {
                break;
            }
        }
        tokens
            .iter()
            .map(|t| {
                self.tokenizer
                    .decode(t, true)
                    .map(|s| s.trim().to_string())
                    .map_err(|e| Error::Model(format!("translation tokenizer: {e}")))
            })
            .collect()
    }
}

/// The tokenizer from its `tokenizer.json`. The export lists merges whose pieces or result are not
/// in its vocabulary (its JavaScript reader skips them; the Rust one refuses the file), and no
/// such merge can ever produce a token, so they are dropped first.
fn tokenizer(json: &str) -> Result<Tokenizer> {
    let mut v: serde_json::Value = serde_json::from_str(json)?;
    let model = &mut v["model"];
    let vocab = model["vocab"].take();
    let has = |s: &str| vocab.get(s).is_some();
    if let Some(merges) = model["merges"].as_array_mut() {
        merges.retain(|m| {
            let pair: Vec<&str> = match m {
                serde_json::Value::String(s) => s.splitn(2, ' ').collect(),
                serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect(),
                _ => Vec::new(),
            };
            pair.len() == 2
                && has(pair[0])
                && has(pair[1])
                && has(&format!("{}{}", pair[0], pair[1]))
        });
    }
    model["vocab"] = vocab;
    serde_json::to_string(&v)?
        .parse::<Tokenizer>()
        .map_err(|e| Error::Model(format!("translation tokenizer: {e}")))
}

/// The cache inputs or outputs, in the order the model lists them: per layer the decoder's key and
/// value, then the encoder's.
fn cache_names(prefix: &str) -> Vec<String> {
    (0..LAYERS)
        .flat_map(|i| {
            [
                "decoder.key",
                "decoder.value",
                "encoder.key",
                "encoder.value",
            ]
            .map(|kv| format!("{prefix}.{i}.{kv}"))
        })
        .collect()
}

fn argmax(v: &[f32]) -> usize {
    v.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i)
}
