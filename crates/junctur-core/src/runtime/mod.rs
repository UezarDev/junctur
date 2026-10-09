//! ONNX Runtime sessions, and which device they run on.
//!
//! Model code asks for a session and gets back the device that actually took it. Each accelerator is a
//! [`Backend`], so adding DirectML, ROCm or CoreML is a new impl here, not a change in the model code.

#[cfg(feature = "cuda")]
mod cuda;

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
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
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

/// Devices compiled into this build, for the report.
pub fn compiled() -> &'static str {
    if cfg!(feature = "cuda") {
        "cuda, cpu"
    } else {
        "cpu"
    }
}
