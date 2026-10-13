//! The ONNX models this engine uses, fetched on first use into the user cache.
//!
//! Models are never committed (size, and their licenses are not ours to relicense). Each is pinned by
//! SHA-256, so a changed upstream file fails loudly instead of silently changing results.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// pyannote segmentation-3.0 (MIT, CNRS), who talks and who talks at once per 17 ms of a 10 s window
/// (5b). pyannote's own repository is gated behind a Hugging Face account; this is k2-fsa's ONNX
/// export of the same weights, ungated, with pyannote's MIT license beside it, pinned to a commit.
pub const SEGMENTATION: Model = Model {
    file: "pyannote_segmentation_3_0.onnx",
    url: "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/9403a6902bb58e3d5ae8c7e77c3422de279db2e0/model.onnx",
    sha256: "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079",
    license: "MIT",
};

/// Whisper large-v3 for whisper.cpp (ggml, MIT), f16. ~3.1 GB; wants ~4.8 GB of free VRAM (measured in 1b).
pub const WHISPER_LARGE_V3: Model = Model {
    file: "ggml-large-v3.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3.bin",
    sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    license: "MIT",
};

/// Whisper large-v3-turbo (ggml, MIT), f16, 1.6 GB: large-v3's encoder with 4 decoder layers instead of
/// 32. The default model: faster and lighter on VRAM than even the 5-bit large-v3 (measured in 1f).
pub const WHISPER_LARGE_V3_TURBO: Model = Model {
    file: "ggml-large-v3-turbo.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin",
    sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    license: "MIT",
};

/// The same large-v3 weights quantised to 5 bits (1.1 GB): fits beside another app on a 12 GB card.
pub const WHISPER_LARGE_V3_Q5: Model = Model {
    file: "ggml-large-v3-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-q5_0.bin",
    sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
    license: "MIT",
};

/// M2M100 418M (MIT, Meta), the caption translation model (5k): one many-to-many model for 100
/// languages. Xenova's ONNX export with its weights quantised to 8 bits, 288 MB of encoder and
/// 344 MB of decoder (the decoder with and without its key/value cache merged in one file), pinned
/// to a commit; the full-precision files are 1.1 and 1.3 GB.
pub const TRANSLATE_ENCODER: Model = Model {
    file: "m2m100_418m_encoder_int8.onnx",
    url: "https://huggingface.co/Xenova/m2m100_418M/resolve/9c374f0b7aca709787cea97b047bfbbd1559d177/onnx/encoder_model_quantized.onnx",
    sha256: "13a94e354a9140764eb81102d77d3ec6952d796e6f113c651eeb3c3443da0386",
    license: "MIT",
};

pub const TRANSLATE_DECODER: Model = Model {
    file: "m2m100_418m_decoder_int8.onnx",
    url: "https://huggingface.co/Xenova/m2m100_418M/resolve/9c374f0b7aca709787cea97b047bfbbd1559d177/onnx/decoder_model_merged_quantized.onnx",
    sha256: "007654bcabb6cea6fd3bde34ce933137b431330b3755781145d7b6906270b45a",
    license: "MIT",
};

/// Its SentencePiece vocabulary as a Hugging Face `tokenizer.json` (8 MB), from the same commit.
pub const TRANSLATE_TOKENIZER: Model = Model {
    file: "m2m100_418m_tokenizer.json",
    url: "https://huggingface.co/Xenova/m2m100_418M/resolve/9c374f0b7aca709787cea97b047bfbbd1559d177/tokenizer.json",
    sha256: "03d9e111731c2d71f39a2c2a88499743e4c251385d07f0384b4349a23ba54363",
    license: "MIT",
};

pub fn dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("models")
}

/// Path to `model`, downloading it first if missing. Downloads go to a `.part` file that is renamed
/// only after the checksum matches, so an interrupted fetch never looks like a model.
pub fn ensure(model: &Model, cache_dir: &Path) -> Result<PathBuf> {
    ensure_or_stop(model, cache_dir, &AtomicBool::new(false))
}

/// [`ensure`], whose download ends with [`Error::Cancelled`] soon after `stop` is set: the
/// translation model (5k) is fetched by a run the user can cancel, and its 630 MB would otherwise
/// hold the cancel (and any run waiting on it) for minutes.
pub fn ensure_or_stop(model: &Model, cache_dir: &Path, stop: &AtomicBool) -> Result<PathBuf> {
    let dir = dir(cache_dir);
    let path = dir.join(model.file);
    if path.is_file() {
        if verified(model, &path)? {
            return Ok(path);
        }
        // Another file under the model's name (a changed pin, a partial copy): fetched again.
        eprintln!(
            "junctur: {} is not the pinned model; fetching it again",
            model.file
        );
        std::fs::remove_file(&path)?;
    }
    if stop.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    std::fs::create_dir_all(&dir)?;
    let part = dir.join(format!("{}.part", model.file));
    eprintln!("junctur: downloading {} ...", model.file);
    // System curl (Windows 10+ ships it), like ffmpeg: an external program rather than a TLS stack.
    let mut curl = Command::new("curl")
        .args(["-sSLf", "--retry", "2", "-o"])
        .arg(&part)
        .arg(model.url)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| Error::Download(format!("could not start curl: {e}")))?;
    let status = loop {
        if let Some(s) = curl.try_wait()? {
            break s;
        }
        if stop.load(Ordering::Relaxed) {
            let _ = curl.kill();
            let _ = curl.wait();
            let _ = std::fs::remove_file(&part);
            return Err(Error::Cancelled);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
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
    mark(model, &path)?;
    Ok(path)
}

/// Where the size and time of a model file last checked against its pin are kept.
fn marker(path: &Path) -> PathBuf {
    let mut m = path.as_os_str().to_owned();
    m.push(".verified");
    PathBuf::from(m)
}

/// `size mtime sha256` of `path`, as the marker holds it.
fn stamp(model: &Model, path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Ok(format!("{} {mtime} {}", meta.len(), model.sha256))
}

fn mark(model: &Model, path: &Path) -> Result<()> {
    std::fs::write(marker(path), stamp(model, path)?)?;
    Ok(())
}

/// Whether the file at `path` is `model` as pinned. Hashing a 3 GB Whisper model on every run
/// would cost seconds, so a file is hashed once and its size and time remembered beside it; a
/// file that changes since, or a pin that changes, is hashed again.
fn verified(model: &Model, path: &Path) -> Result<bool> {
    if std::fs::read_to_string(marker(path)).ok() == Some(stamp(model, path)?) {
        return Ok(true);
    }
    let ok = sha256_file(path)? == model.sha256;
    if ok {
        mark(model, path)?;
    }
    Ok(ok)
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
    fn a_model_file_is_checked_once_and_again_when_it_changes() {
        let d = std::env::temp_dir().join(format!("junctur-models-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("m.onnx");
        std::fs::write(&path, b"abc").unwrap();
        let m = Model {
            file: "m.onnx",
            url: "",
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            license: "",
        };
        assert!(verified(&m, &path).unwrap());
        assert!(marker(&path).is_file());
        assert!(verified(&m, &path).unwrap(), "from the marker");
        std::fs::write(&path, b"abd!").unwrap();
        assert!(!verified(&m, &path).unwrap(), "changed since");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_stopped_fetch_is_cancelled_and_a_fetched_model_is_not() {
        let d = std::env::temp_dir().join(format!("junctur-models-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let m = Model {
            file: "m.onnx",
            url: "http://127.0.0.1:9/never",
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            license: "",
        };
        let stop = AtomicBool::new(true);
        assert!(matches!(
            ensure_or_stop(&m, &d, &stop),
            Err(Error::Cancelled)
        ));
        assert!(!dir(&d).join("m.onnx.part").exists());
        // One already there is found whatever the stop says.
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::write(dir(&d).join("m.onnx"), b"abc").unwrap();
        assert!(ensure_or_stop(&m, &d, &stop).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sha256_reference_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
