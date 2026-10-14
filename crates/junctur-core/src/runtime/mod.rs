//! ONNX Runtime sessions, and which device they run on.
//!
//! Model code asks for a session and gets back the device that actually took it. Each accelerator is a
//! [`Backend`], so adding DirectML, ROCm or CoreML is a new impl here, not a change in the model code.

#[cfg(feature = "cuda")]
mod cuda;
pub mod vram;

use std::path::Path;

use ort::session::Session;
use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    Cpu,
    Cuda,
}

/// Which devices the caller allows. `Auto` tries every compiled-in accelerator, then CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevicePref {
    #[default]
    Auto,
    Cpu,
}

/// One accelerator. `open` returns `Ok(None)` when the device is not usable here, so the caller
/// moves on to the next one; an `Err` is a model that this device cannot run.
pub trait Backend {
    fn device(&self) -> Device;
    fn open(&self, model: &Path, threads: usize) -> Result<Option<Session>>;
}

struct Cpu;

impl Backend for Cpu {
    fn device(&self) -> Device {
        Device::Cpu
    }
    fn open(&self, model: &Path, threads: usize) -> Result<Option<Session>> {
        let s = Session::builder()?
            .with_intra_threads(threads)?
            .commit_from_file(model)?;
        Ok(Some(s))
    }
}

/// Put the CUDA and cuDNN DLL folders on PATH. Needed before anything loads a CUDA DLL lazily
/// (ort sessions, and whisper.cpp, whose CUDA DLLs are delay-loaded); a no-op without `cuda`.
pub fn prepare_cuda() {
    #[cfg(feature = "cuda")]
    cuda::prepare();
}

/// The CUDA DLL whisper.cpp delay-loads (see `build.rs`) when it is nowhere it would be loaded from.
/// whisper.cpp does not fail on a missing cuBLAS, the process dies, so Whisper must not be put on
/// the GPU then. Only Windows CUDA builds delay-load; everywhere else nothing is ever missing.
pub fn whisper_cuda_missing() -> Option<&'static str> {
    #[cfg(all(feature = "cuda", windows))]
    {
        cuda::whisper_dll_missing()
    }
    #[cfg(not(all(feature = "cuda", windows)))]
    {
        None
    }
}

fn backends(pref: DevicePref) -> Vec<Box<dyn Backend>> {
    let mut list: Vec<Box<dyn Backend>> = Vec::new();
    #[cfg(feature = "cuda")]
    if pref == DevicePref::Auto {
        list.push(Box::new(cuda::Cuda));
    }
    let _ = pref;
    list.push(Box::new(Cpu));
    list
}

/// Open `model` on the first backend that takes it. A GPU that fails is not fatal: the session
/// falls back to CPU, and the returned device says which one ran.
pub fn open(model: &Path, pref: DevicePref) -> Result<(Session, Device)> {
    open_with_threads(model, pref, cores())
}

/// This machine's cores, or 4 when the OS will not say.
fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// [`open`] with `threads` CPU threads per operator, for a small model run on several threads of
/// our own, where every session taking every core would only make them contend.
pub fn open_with_threads(
    model: &Path,
    pref: DevicePref,
    threads: usize,
) -> Result<(Session, Device)> {
    let mut last = None;
    for b in backends(pref) {
        match b.open(model, threads) {
            Ok(Some(s)) => return Ok((s, b.device())),
            Ok(None) => {}
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| crate::Error::Model("no backend could open the model".into())))
}

/// Open `model` on the CPU and on nothing else, `threads` per operator (every core when `None`).
/// The only way the Tracks, Speech and Speakers stages open a model: only transcription uses the
/// GPU, and CAM++ on ONNX Runtime's CUDA provider makes a speaker of every sentence.
pub fn open_on_cpu(model: &Path, threads: Option<usize>) -> Result<Session> {
    let threads = threads.unwrap_or_else(cores);
    Cpu.open(model, threads)?
        .ok_or_else(|| crate::Error::Model("the CPU could not open the model".into()))
}

/// Devices compiled into this build, for the report.
pub fn compiled() -> &'static str {
    if cfg!(feature = "cuda") {
        "cuda, cpu"
    } else {
        "cpu"
    }
}
