//! whisper.cpp through `whisper-rs`: model choice (large-v3-turbo, or a fallback for the free VRAM),
//! then the language of a group of chunks, then one chunk at a time.
//!
//! whisper.cpp rather than an ONNX export: its CUDA build runs large-v3 with quantised weights and
//! gives token timestamps directly, and the ONNX large-v3 exports need a hand-written decoder loop.

use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use super::languages::Languages;
use super::words::{self, Token, Word};
use crate::models::{self, Model};
use crate::runtime::{self, vram, Device, DevicePref};
use crate::{Error, Result};

/// Which model to load. `Auto` (the default) means large-v3-turbo, on the GPU when it fits in the free
/// VRAM, else on CPU; the others are what the user asked for, by `--model` or the config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "String")]
pub enum ModelPref {
    #[default]
    Auto,
    LargeV3Turbo,
    /// Best quality, opt-in: f16 large-v3 is slower than turbo and wants about 4.8 GB of free VRAM.
    LargeV3,
    /// 1b's fallback, kept as an explicit choice: it needs more VRAM than turbo (below), so `Auto`
    /// never falls back to it, but it is large-v3's decoder for anyone who prefers it.
    LargeV3Q5,
}

impl ModelPref {
    /// Every accepted name, aliases after the canonical one. The CLI and the config file both read
    /// this, so they accept the same words.
    pub const NAMES: &[(&str, ModelPref)] = &[
        ("auto", ModelPref::Auto),
        ("large-v3-turbo", ModelPref::LargeV3Turbo),
        ("turbo", ModelPref::LargeV3Turbo),
        ("large-v3", ModelPref::LargeV3),
        ("best", ModelPref::LargeV3),
        ("large-v3-q5_0", ModelPref::LargeV3Q5),
    ];

    /// The model this preference loads. Known without asking the GPU: `Auto` only picks the device,
    /// so a run that has nothing left to transcribe never needs `nvidia-smi`.
    pub fn model(self) -> &'static Model {
        match self {
            ModelPref::Auto | ModelPref::LargeV3Turbo => &models::WHISPER_LARGE_V3_TURBO,
            ModelPref::LargeV3 => &models::WHISPER_LARGE_V3,
            ModelPref::LargeV3Q5 => &models::WHISPER_LARGE_V3_Q5,
        }
    }

    /// The model's name as [`Choice::model`] and the transcript record it (`large-v3-turbo`, ...).
    pub fn name(self) -> String {
        model_name(self.model())
    }
}

fn model_name(m: &Model) -> String {
    m.file
        .trim_start_matches("ggml-")
        .trim_end_matches(".bin")
        .into()
}

impl FromStr for ModelPref {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == s)
            .map(|&(_, m)| m)
            .ok_or_else(|| {
                let names: Vec<&str> = Self::NAMES.iter().map(|(n, _)| *n).collect();
                format!("unknown model {s} ({})", names.join(", "))
            })
    }
}

impl TryFrom<String> for ModelPref {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, String> {
        s.parse()
    }
}

/// Free MiB turbo needs on the GPU (weights + compute buffers), measured in 1f: a 2025 MiB peak over
/// a 10-minute recording with beam search 5. The 5-bit large-v3 peaked at 2211 MiB on the same audio
/// (its 32 decoder layers hold more KV cache than turbo's 4 save in weights), so there is no free-VRAM
/// band where it fits and turbo does not.
const TURBO_MIB: u64 = 2100;

/// What will run, and why, decided before anything is loaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub model: String,
    pub device: Device,
    /// Set when the choice is a fallback the user should hear about.
    pub note: Option<String>,
}

/// Why the GPU was or was not looked at, so a CPU run can say why it is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gpu {
    /// The caller asked for the CPU (`--cpu`): nothing to warn about.
    Off,
    /// Built without `cuda`. 2l: the app's normal build was one, and its CPU runs said nothing.
    NotBuilt,
    /// A GPU answered but the CUDA runtime DLL whisper.cpp delay-loads is not there (named).
    NoRuntime(&'static str),
    /// Looked: `free` says what was found.
    Looked,
}

pub fn choose(pref: ModelPref, device: DevicePref) -> (Choice, &'static Model) {
    let mut gpu = match device {
        DevicePref::Cpu => Gpu::Off,
        DevicePref::Auto if !cfg!(feature = "cuda") => Gpu::NotBuilt,
        DevicePref::Auto => Gpu::Looked,
    };
    let mut free = if gpu == Gpu::Looked {
        vram::free_mib()
    } else {
        None
    };
    // 2l: without cuBLAS whisper.cpp dies on its first GPU call instead of failing, so the GPU is only
    // offered to it when the DLL can be found.
    if free.is_some() {
        if let Some(dll) = runtime::whisper_cuda_missing() {
            (gpu, free) = (Gpu::NoRuntime(dll), None);
        }
    }
    pick(pref, free, gpu)
}

/// The choice for `free` MiB of VRAM (None: no GPU answered, or none was asked). `gpu` says why, so a
/// CPU run that the user did not ask for carries a note. Pure, so the fallback table is testable
/// without a GPU.
///
/// `Auto` without room on the card runs turbo on CPU rather than 1b's 5-bit large-v3: the 5-bit model
/// would not fit either (see [`TURBO_MIB`]), and on CPU turbo's 4 decoder layers make it the faster.
fn pick(pref: ModelPref, free: Option<u64>, gpu: Gpu) -> (Choice, &'static Model) {
    let m = pref.model();
    let (device, note) = match (pref, free, gpu) {
        (_, None, Gpu::NotBuilt) => (
            Device::Cpu,
            Some("this build has no GPU support (built without `--features cuda`): running on CPU (slow)".to_string()),
        ),
        (_, None, Gpu::NoRuntime(dll)) => (
            Device::Cpu,
            Some(format!(
                "NVIDIA GPU found but the CUDA 13 runtime ({dll}) is not installed: running on CPU (slow)"
            )),
        ),
        (ModelPref::Auto, Some(f), _) if f < TURBO_MIB => (
            Device::Cpu,
            Some(format!(
                "only {f} MiB of VRAM free, under the {TURBO_MIB} MiB Whisper needs: running on CPU (slow)"
            )),
        ),
        (ModelPref::Auto, None, Gpu::Looked) => (
            Device::Cpu,
            Some("no usable NVIDIA GPU found: running on CPU (slow)".to_string()),
        ),
        // Explicit models, and `Auto` with room: the GPU whenever one answers.
        (_, Some(_), _) => (Device::Cuda, None),
        (_, None, _) => (Device::Cpu, None),
    };
    let choice = Choice {
        model: model_name(m),
        device,
        note,
    };
    (choice, m)
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

    /// The language of `samples` (16 kHz mono, Whisper looks at the first 30 s) among `langs`.
    pub fn detect(&mut self, samples: &[f32], langs: &Languages) -> Result<String> {
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
        Ok(langs.choose(&probs))
    }

    /// Transcribe `samples` (16 kHz mono) as `lang`; word times are seconds from their start.
    pub fn transcribe(&mut self, samples: &[f32], lang: &str) -> Result<Vec<Word>> {
        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        });
        params.set_language(Some(lang));
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
        Ok(words::group(&tokens))
    }
}

fn err(e: whisper_rs::WhisperError) -> Error {
    Error::Model(format!("whisper: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picked(pref: ModelPref, free: Option<u64>, gpu: Gpu) -> (String, Device, bool) {
        let (c, _) = pick(pref, free, gpu);
        (c.model, c.device, c.note.is_some())
    }

    #[test]
    fn auto_is_turbo_on_gpu_else_cpu() {
        let turbo = || "large-v3-turbo".to_string();
        assert_eq!(ModelPref::default(), ModelPref::Auto);
        assert_eq!(
            picked(ModelPref::Auto, Some(TURBO_MIB), Gpu::Looked),
            (turbo(), Device::Cuda, false)
        );
        assert_eq!(
            picked(ModelPref::Auto, Some(TURBO_MIB - 1), Gpu::Looked),
            (turbo(), Device::Cpu, true)
        );
        assert_eq!(
            picked(ModelPref::Auto, None, Gpu::Looked),
            (turbo(), Device::Cpu, true)
        );
        // `--cpu`: nothing to warn about.
        assert_eq!(
            picked(ModelPref::Auto, None, Gpu::Off),
            (turbo(), Device::Cpu, false)
        );
    }

    #[test]
    fn explicit_models_ignore_free_vram() {
        assert_eq!(
            picked(ModelPref::LargeV3, Some(100), Gpu::Looked),
            ("large-v3".into(), Device::Cuda, false)
        );
        assert_eq!(
            picked(ModelPref::LargeV3Q5, None, Gpu::Looked),
            ("large-v3-q5_0".into(), Device::Cpu, false)
        );
        assert_eq!(
            picked(ModelPref::LargeV3Turbo, Some(100), Gpu::Looked),
            ("large-v3-turbo".into(), Device::Cuda, false)
        );
    }

    /// 2l: a CPU-only build, or a GPU without the CUDA runtime, ran on CPU and said nothing.
    #[test]
    fn a_cpu_run_nobody_asked_for_says_why() {
        for pref in [ModelPref::Auto, ModelPref::LargeV3, ModelPref::LargeV3Turbo] {
            let (c, _) = pick(pref, None, Gpu::NotBuilt);
            assert_eq!(c.device, Device::Cpu);
            assert!(c.note.unwrap().contains("--features cuda"));
            let (c, _) = pick(pref, None, Gpu::NoRuntime("cublas64_13.dll"));
            assert_eq!(c.device, Device::Cpu);
            assert!(c.note.unwrap().contains("cublas64_13.dll"));
            assert_eq!(pick(pref, None, Gpu::Off).0.note, None);
        }
    }

    #[test]
    fn names_and_aliases_parse() {
        for &(name, m) in ModelPref::NAMES {
            assert_eq!(name.parse::<ModelPref>().unwrap(), m);
        }
        assert_eq!("best".parse::<ModelPref>(), Ok(ModelPref::LargeV3));
        assert_eq!("turbo".parse::<ModelPref>(), Ok(ModelPref::LargeV3Turbo));
        let err = "large-v4".parse::<ModelPref>().unwrap_err();
        assert!(err.contains("large-v3-turbo"), "{err}");
        assert_eq!(
            serde_json::from_str::<ModelPref>("\"large-v3-q5_0\"").unwrap(),
            ModelPref::LargeV3Q5
        );
        assert!(serde_json::from_str::<ModelPref>("\"large\"").is_err());
    }

    #[test]
    fn recorded_model_names_parse_back() {
        // `Choice::model` is what the transcript and `junctur status` show; it must parse back, and
        // `name` (used to decide whether a transcript can be reused) must agree with `pick`.
        for pref in [
            ModelPref::LargeV3Turbo,
            ModelPref::LargeV3,
            ModelPref::LargeV3Q5,
        ] {
            let (c, _) = pick(pref, None, Gpu::Off);
            assert_eq!(c.model.parse::<ModelPref>(), Ok(pref));
            assert_eq!(c.model, pref.name());
        }
        assert_eq!(ModelPref::Auto.name(), "large-v3-turbo");
    }
}
