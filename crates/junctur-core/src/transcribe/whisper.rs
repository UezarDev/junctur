//! whisper.cpp through `whisper-rs`: model choice for the free VRAM, then one chunk at a time.
//!
//! whisper.cpp rather than an ONNX export: its CUDA build runs large-v3 with quantised weights and
//! gives token timestamps directly, and the ONNX large-v3 exports need a hand-written decoder loop.

use std::path::Path;

use serde::{Deserialize, Serialize};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use super::languages::Languages;
use super::words::{self, Token, Word};
use crate::models::{self, Model};
use crate::runtime::{self, vram, Device, DevicePref};
use crate::{Error, Result};

/// Which model to load. `Auto` picks from the free VRAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelPref {
    #[default]
    Auto,
    LargeV3,
    LargeV3Q5,
}

/// Free MiB needed for each model on the GPU (weights + compute buffers, measured on large-v3).
const F16_MIB: u64 = 4800;
const Q5_MIB: u64 = 2200;

/// What will run, and why, decided before anything is loaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub model: String,
    pub device: Device,
    /// Set when the choice is a fallback the user should hear about.
    pub note: Option<String>,
}

pub fn choose(pref: ModelPref, device: DevicePref) -> (Choice, &'static Model) {
    let gpu = cfg!(feature = "cuda") && device == DevicePref::Auto;
    let free = if gpu { vram::free_mib() } else { None };
    let pick = |m: &'static Model, d: Device, note: Option<String>| {
        (
            Choice {
                model: m
                    .file
                    .trim_start_matches("ggml-")
                    .trim_end_matches(".bin")
                    .into(),
                device: d,
                note,
            },
            m,
        )
    };
    let q5 = &models::WHISPER_LARGE_V3_Q5;
    let f16 = &models::WHISPER_LARGE_V3;
    match (pref, free) {
        (ModelPref::LargeV3, Some(_)) => pick(f16, Device::Cuda, None),
        (ModelPref::LargeV3Q5, Some(_)) => pick(q5, Device::Cuda, None),
        (ModelPref::LargeV3, None) => pick(f16, Device::Cpu, None),
        (ModelPref::LargeV3Q5, None) => pick(q5, Device::Cpu, None),
        (ModelPref::Auto, Some(f)) if f >= F16_MIB => pick(f16, Device::Cuda, None),
        (ModelPref::Auto, Some(f)) if f >= Q5_MIB => pick(
            q5,
            Device::Cuda,
            Some(format!(
                "{f} MiB of VRAM free (another app holds the rest): using the 5-bit large-v3"
            )),
        ),
        (ModelPref::Auto, Some(f)) => pick(
            q5,
            Device::Cpu,
            Some(format!(
                "only {f} MiB of VRAM free, under the {Q5_MIB} MiB Whisper needs: running on CPU (slow)"
            )),
        ),
        (ModelPref::Auto, None) => pick(
            q5,
            Device::Cpu,
            gpu.then(|| "no usable NVIDIA GPU found: running on CPU (slow)".to_string()),
        ),
    }
}

pub struct Whisper {
    state: WhisperState,
    threads: i32,
    /// Token ids from end-of-text up are special (timestamps, language, markers).
    eot: i32,
}

impl Whisper {
    pub fn open(model: &Path, device: Device) -> Result<Self> {
        runtime::prepare_cuda();
        // whisper.cpp logs every layer to stderr; route it to the (unconfigured) `log` facade.
        whisper_rs::install_logging_hooks();
        let mut p = WhisperContextParameters::default();
        p.use_gpu(device == Device::Cuda).flash_attn(true);
        let ctx = WhisperContext::new_with_params(model, p)
            .map_err(|e| Error::Model(format!("whisper: {e}")))?;
        let state = ctx
            .create_state()
            .map_err(|e| Error::Model(format!("whisper: {e}")))?;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;
        let eot = ctx.token_eot();
        Ok(Self {
            state,
            threads,
            eot,
        })
    }

    /// Detect the language among `langs`, then transcribe. `samples` is 16 kHz mono; word times
    /// are seconds from the start of `samples`.
    pub fn run(&mut self, samples: &[f32], langs: &Languages) -> Result<(String, Vec<Word>)> {
        let err = |e: whisper_rs::WhisperError| Error::Model(format!("whisper: {e}"));
        self.state
            .pcm_to_mel(samples, self.threads as usize)
            .map_err(err)?;
        let (_, probs) = self
            .state
            .lang_detect(0, self.threads as usize)
            .map_err(err)?;
        let probs: Vec<(String, f32)> = langs
            .allowed
            .iter()
            .filter_map(|c| {
                let id = whisper_rs::get_lang_id(c)?;
                Some((c.clone(), *probs.get(id as usize)?))
            })
            .collect();
        let lang = langs.choose(&probs);

        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        });
        params.set_language(Some(&lang));
        params.set_n_threads(self.threads);
        params.set_token_timestamps(true);
        // Each chunk stands alone: carrying text across chunks is how Whisper loops on a phrase.
        params.set_no_context(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        self.state.full(params, samples).map_err(err)?;

        let mut tokens = Vec::new();
        for seg in self.state.as_iter() {
            // Whisper's own "this was not speech" estimate; such segments are usually invented text.
            if seg.no_speech_probability() > 0.8 {
                continue;
            }
            for i in 0..seg.n_tokens() {
                let Some(tok) = seg.get_token(i) else {
                    continue;
                };
                let d = tok.token_data();
                if d.id >= self.eot {
                    continue;
                }
                let Ok(bytes) = tok.to_bytes() else { continue };
                tokens.push(Token {
                    bytes: bytes.to_vec(),
                    start: d.t0 as f64 / 100.0,
                    end: d.t1 as f64 / 100.0,
                    p: d.p,
                });
            }
        }
        Ok((lang, words::group(&tokens)))
    }
}
