//! The ONNX models this engine uses, fetched on first use into the user cache.
//!
//! Models are never committed (size, and their licenses are not ours to relicense). Each is pinned by
//! SHA-256, so a changed upstream file fails loudly instead of silently changing results.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub struct Model {
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// Shown in `--help`-style listings and the phase notes.
    pub license: &'static str,
}

/// Silero VAD v5 (MIT). 16 kHz, 512-sample frames.
pub const SILERO_VAD: Model = Model {
    file: "silero_vad_v5.onnx",
    url: "https://github.com/snakers4/silero-vad/raw/v5.1.2/src/silero_vad/data/silero_vad.onnx",
    sha256: "2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f",
    license: "MIT",
};

/// UVR MDX-Net "Kim Vocal 2", vocals from a stereo 44.1 kHz mix.
pub const MDX_VOCALS: Model = Model {
    file: "Kim_Vocal_2.onnx",
    url: "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/Kim_Vocal_2.onnx",
    sha256: "ce74ef3b6a6024ce44211a07be9cf8bc6d87728cc852a68ab34eb8e58cde9c8b",
    license: "no explicit license on the weights (UVR model repo, code MIT)",
};

/// 3D-Speaker CAM++ speaker embeddings, VoxCeleb (Apache-2.0), as exported by sherpa-onnx.
pub const SPEAKER_EMBEDDING: Model = Model {
    file: "campplus_en_voxceleb_16k.onnx",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
    sha256: "357a834f702b80161e5b981182c038e18553c1f2ca752ed6cec2052365d4129b",
    license: "Apache-2.0",
};

pub fn dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("models")
}

/// Path to `model`, downloading it first if missing. Downloads go to a `.part` file that is renamed
/// only after the checksum matches, so an interrupted fetch never looks like a model.
pub fn ensure(model: &Model, cache_dir: &Path) -> Result<PathBuf> {
    let dir = dir(cache_dir);
    let path = dir.join(model.file);
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    let part = dir.join(format!("{}.part", model.file));
    eprintln!("junctur: downloading {} ...", model.file);
    // System curl (Windows 10+ ships it), like ffmpeg: an external program rather than a TLS stack.
    let status = Command::new("curl")
        .args(["-sSLf", "--retry", "2", "-o"])
        .arg(&part)
        .arg(model.url)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| Error::Download(format!("could not start curl: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Download(format!("{} ({status})", model.url)));
    }
    let got = sha256_hex(&std::fs::read(&part)?);
    if got != model.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Download(format!(
            "{}: checksum {got}, expected {}",
            model.file, model.sha256
        )));
    }
    std::fs::rename(&part, &path)?;
    Ok(path)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_reference_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
