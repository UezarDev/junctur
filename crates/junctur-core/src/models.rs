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

/// Whisper large-v3 for whisper.cpp (ggml, MIT), f16. ~3.1 GB; wants ~4.5 GB of free VRAM.
pub const WHISPER_LARGE_V3: Model = Model {
    file: "ggml-large-v3.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3.bin",
    sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    license: "MIT",
};

/// The same large-v3 weights quantised to 5 bits (1.1 GB): fits beside another app on a 12 GB card.
pub const WHISPER_LARGE_V3_Q5: Model = Model {
    file: "ggml-large-v3-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-q5_0.bin",
    sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
    license: "MIT",
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
    let got = sha256_file(&part)?;
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

/// Streamed: the Whisper models are gigabytes, too large to read whole.
fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match f.read(&mut buf)? {
            0 => break,
            n => h.update(&buf[..n]),
        }
    }
    Ok(hex(&h.finalize()))
}

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
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
