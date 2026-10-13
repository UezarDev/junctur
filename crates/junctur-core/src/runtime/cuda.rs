//! CUDA backend. ort's prebuilt Windows runtime is built against CUDA 13 and cuDNN 9, which it
//! loads from the DLL search path; the installers do not put cuDNN there, so we find both and
//! prepend them to PATH once before the first session.

use std::path::{Path, PathBuf};
use std::sync::Once;

use ort::ep::{ExecutionProvider, CUDA};
use ort::session::Session;

use super::{Backend, Device};
use crate::Result;

pub struct Cuda;

impl Backend for Cuda {
    fn device(&self) -> Device {
        Device::Cuda
    }

    fn open(&self, model: &Path, threads: usize) -> Result<Option<Session>> {
        prepare();
        let ep = CUDA::default();
        if !ep.is_available().unwrap_or(false) {
            return Ok(None);
        }
        let mut b = Session::builder()?.with_intra_threads(threads)?;
        // Registration fails when the CUDA or cuDNN DLLs cannot load; that is "no GPU", not an error.
        if ep.register(&mut b).is_err() {
            return Ok(None);
        }
        Ok(Some(b.commit_from_file(model)?))
    }
}

/// Extend PATH once per process.
pub fn prepare() {
    static PATH: Once = Once::new();
    PATH.call_once(extend_path);
}

fn extend_path() {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in ["JUNCTUR_CUDA_DIR", "CUDA_PATH_V13_0", "CUDA_PATH"] {
        if let Some(d) = std::env::var_os(var) {
            let d = PathBuf::from(d);
            dirs.push(d.join("bin").join("x64"));
            dirs.push(d.join("bin"));
        }
    }
    if let Some(d) = std::env::var_os("JUNCTUR_CUDNN_DIR") {
        dirs.push(PathBuf::from(d));
    }
    dirs.extend(cudnn_dirs(Path::new(r"C:\Program Files\NVIDIA\CUDNN")));
    let current = std::env::var_os("PATH").unwrap_or_default();
    dirs.extend(std::env::split_paths(&current));
    if let Ok(joined) = std::env::join_paths(dirs.into_iter().filter(|d| d.is_dir())) {
        std::env::set_var("PATH", joined);
    }
}

/// whisper.cpp's one imported CUDA DLL; the name `build.rs` delay-loads.
#[cfg(windows)]
const WHISPER_DLL: &str = "cublas64_13.dll";

/// [`WHISPER_DLL`] when it is neither beside the executable nor on PATH (after [`prepare`]). The
/// delay-load also searches System32, the Windows folder and the current directory; those are not
/// checked, because the CUDA installer puts the DLL in none of them. A copy placed only there is
/// reported missing, and Whisper runs on the CPU: the safe side of the guess.
#[cfg(windows)]
pub fn whisper_dll_missing() -> Option<&'static str> {
    prepare();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = exe
        .into_iter()
        .chain(std::env::split_paths(&path))
        .any(|d| d.join(WHISPER_DLL).is_file());
    (!found).then_some(WHISPER_DLL)
}

/// `<root>\v9.x\bin\13.x` folders of a cuDNN install, newest first.
fn cudnn_dirs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for v in std::fs::read_dir(root).into_iter().flatten().flatten() {
        for c in std::fs::read_dir(v.path().join("bin"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if c.file_name().to_string_lossy().starts_with("13.") {
                found.push(c.path());
            }
        }
    }
    found.sort();
    found.reverse();
    found
}
