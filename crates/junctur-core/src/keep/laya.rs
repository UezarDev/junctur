//! Laya multilingual through ONNX Runtime: a question with two options and a state in, the keep
//! option's logit minus the cut option's out. The input is laid out exactly as Laya's Python SDK lays
//! it (`laya/common.py`, `build_head` and `build_sequence`), since the weights were trained on that
//! layout and nothing else; the ignored test against the SDK's own output pins it.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::runtime::{self, Device, DevicePref};
use crate::{models, Error, Result};

/// What `keep.json` names the model.
pub const NAME: &str = "laya-multilingual-fp16";
/// The whole input, in tokens (the multilingual config's `max_len`).
pub const MAX_LEN: usize = 1024;
/// The question and its options, in tokens (its `head_max_len`).
pub const HEAD_MAX_LEN: usize = 256;
/// Tokens kept of each option.
const OPTION_MAX: usize = 48;
/// `choice` in Laya's question types.
const CHOICE: i64 = 0;
/// Rows in one run, and tokens (rows times the longest): a full card would fall back to the CPU.
const BATCH_ROWS: usize = 16;
const BATCH_TOKENS: usize = 16 * MAX_LEN;

/// A two-option question: whether to keep a part, each option described in words.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub instructions: String,
    pub keep: String,
    pub cut: String,
}

/// Laya's vocabulary and the ids of the tokens its layout uses.
pub struct Vocab {
    tokenizer: Tokenizer,
    cls: u32,
    sep: u32,
    mask: u32,
    pad: u32,
}

/// The mask token's text: replaced by a space in anything given, as the SDK does, so no text can add
/// an option marker of its own.
const MASK: &str = "<mask>";

impl Vocab {
    /// Fetches the tokenizer on first use (34 MB) and reads it.
    pub fn open(cache_dir: &Path) -> Result<Self> {
        let path = models::ensure(&models::LAYA_TOKENIZER, cache_dir)?;
        Self::read(&std::fs::read_to_string(path)?)
    }

    /// The tokenizer from its JSON.
    pub fn read(json: &str) -> Result<Self> {
        let tokenizer = json
            .parse::<Tokenizer>()
            .map_err(|e| Error::Model(format!("Laya tokenizer: {e}")))?;
        let id = |t: &str| {
            tokenizer
                .token_to_id(t)
                .ok_or_else(|| Error::Model(format!("Laya tokenizer has no {t}")))
        };
        Ok(Self {
            cls: id("<bos>")?,
            sep: id("<eos>")?,
            mask: id(MASK)?,
            pad: id("<pad>")?,
            tokenizer,
        })
    }

    fn ids(&self, text: &str) -> Result<Vec<u32>> {
        let enc = self
            .tokenizer
            .encode(text.replace(MASK, " "), false)
            .map_err(|e| Error::Model(format!("Laya tokenizer: {e}")))?;
        Ok(enc.get_ids().to_vec())
    }

    /// The question's half of every input: `<bos> "choice question: " instructions <eos>`, then
    /// `<mask> " keep: " description <mask> " cut: " description <eos>`, with the markers' places.
    /// Options are held to [`OPTION_MAX`] tokens and the whole to about [`HEAD_MAX_LEN`] as
    /// `build_head` holds them (its budget leaves out the three separators, and so does this).
    pub fn head(&self, q: &Question) -> Result<(Vec<u32>, Vec<usize>)> {
        let head = self.ids(&format!(
            "choice question: {}",
            q.instructions.replace(MASK, " ")
        ))?;
        let mut options: Vec<Vec<u32>> = Vec::with_capacity(2);
        for (label, text) in [("keep", &q.keep), ("cut", &q.cut)] {
            let mut ids = self.ids(&format!(" {label}: {text}"))?;
            ids.truncate(OPTION_MAX);
            ids.insert(0, self.mask);
            options.push(ids);
        }
        let used = |o: &[Vec<u32>]| o.iter().map(Vec::len).sum::<usize>();
        let mut budget = HEAD_MAX_LEN as isize - used(&options) as isize;
        if budget < 16 {
            let per = ((HEAD_MAX_LEN - 16) / options.len()).max(4);
            options.iter_mut().for_each(|o| o.truncate(per));
            budget = HEAD_MAX_LEN as isize - used(&options) as isize;
        }
        let keep_head = (budget.max(8) as usize).min(head.len());
        let mut ids = Vec::with_capacity(HEAD_MAX_LEN + 3);
        ids.push(self.cls);
        ids.extend_from_slice(&head[..keep_head]);
        ids.push(self.sep);
        let mut markers = Vec::with_capacity(options.len());
        for o in options {
            markers.push(ids.len());
            ids.extend(o);
        }
        ids.push(self.sep);
        Ok((ids, markers))
    }

    /// The whole input for `state` after `head` (from [`Self::head`]): the state cut at its end to the
    /// room [`MAX_LEN`] leaves, and `<eos>`.
    pub fn sequence(&self, head: &(Vec<u32>, Vec<usize>), state: &str) -> Result<Row> {
        let (head_ids, markers) = head;
        let room = MAX_LEN.saturating_sub(head_ids.len() + 1);
        let mut ids = head_ids.clone();
        let state = self.ids(state)?;
        ids.extend_from_slice(&state[..state.len().min(room)]);
        ids.push(self.sep);
        ids.truncate(MAX_LEN);
        let markers = markers.iter().copied().filter(|&m| m < MAX_LEN).collect();
        Ok(Row { ids, markers })
    }
}

/// One input, laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub ids: Vec<u32>,
    /// Where each option's `<mask>` is: keep's, then cut's.
    pub markers: Vec<usize>,
}

/// The model, open.
pub struct Laya {
    session: Session,
    pad: u32,
    pub device: Device,
}

impl Laya {
    /// Fetches the model on first use (646 MB) and opens it where `pref` allows, the GPU first.
    pub fn open(cache_dir: &Path, vocab: &Vocab, pref: DevicePref) -> Result<Self> {
        let path = models::ensure(&models::LAYA_MODEL, cache_dir)?;
        let (session, device) = runtime::open(&path, pref)?;
        Ok(Self {
            session,
            pad: vocab.pad,
            device,
        })
    }

    /// Each row's keep logit minus its cut logit, in the rows' order. Run shortest first in batches,
    /// each padded to its longest, so a few long parts do not pad every short one to 1024.
    pub fn margins(&mut self, rows: &[Row], done: &mut dyn FnMut(usize)) -> Result<Vec<f32>> {
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by_key(|&i| rows[i].ids.len());
        let mut out = vec![0f32; rows.len()];
        let mut at = 0;
        while at < order.len() {
            let mut end = at;
            while end < order.len()
                && end - at < BATCH_ROWS
                && rows[order[end]].ids.len() * (end - at + 1) <= BATCH_TOKENS
            {
                end += 1;
            }
            let end = end.max(at + 1);
            let batch: Vec<&Row> = order[at..end].iter().map(|&i| &rows[i]).collect();
            for (k, m) in self.run(&batch)?.into_iter().enumerate() {
                out[order[at + k]] = m;
            }
            at = end;
            done(at);
        }
        Ok(out)
    }

    fn run(&mut self, batch: &[&Row]) -> Result<Vec<f32>> {
        let n = batch.len();
        let len = batch.iter().map(|r| r.ids.len()).max().unwrap_or(0);
        let mut ids = vec![self.pad as i64; n * len];
        let mut attention = vec![0i64; n * len];
        let mut marker_pos = vec![0i64; n * 2];
        let mut marker_mask = vec![false; n * 2];
        for (b, r) in batch.iter().enumerate() {
            if r.markers.len() != 2 {
                return Err(Error::Model(format!(
                    "a Laya input with {} option markers, not 2",
                    r.markers.len()
                )));
            }
            for (k, &id) in r.ids.iter().enumerate() {
                ids[b * len + k] = id as i64;
                attention[b * len + k] = 1;
            }
            for (k, &m) in r.markers.iter().enumerate() {
                marker_pos[b * 2 + k] = m as i64;
                marker_mask[b * 2 + k] = true;
            }
        }
        let out = self.session.run(ort::inputs![
            "input_ids" => Tensor::from_array(([n, len], ids))?,
            "attention_mask" => Tensor::from_array(([n, len], attention))?,
            "marker_pos" => Tensor::from_array(([n, 2], marker_pos))?,
            "marker_mask" => Tensor::from_array(([n, 2], marker_mask))?,
            "qtype" => Tensor::from_array(([n], vec![CHOICE; n]))?,
        ])?;
        let (_, logits) = out["logits"].try_extract_tensor::<f32>()?;
        if logits.len() != n * 2 {
            return Err(Error::Model(format!(
                "Laya gave {} logits for {n} inputs",
                logits.len()
            )));
        }
        let margins: Vec<f32> = logits.chunks_exact(2).map(|l| l[0] - l[1]).collect();
        // A NaN would score the part NaN, written as null, and `keep.json` would no longer read.
        if margins.iter().any(|m| !m.is_finite()) {
            return Err(Error::Model(
                "Laya gave a logit that is not a number".into(),
            ));
        }
        Ok(margins)
    }
}

/// The chance a part with `margin` stays, at temperature `t`.
pub fn p_keep(margin: f64, t: f64) -> f64 {
    1.0 / (1.0 + (-margin / t).exp())
}
