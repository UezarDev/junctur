//! The windows' embeddings the speakers stage keeps beside `speakers.json`, so labelling again
//! needs no audio.

use std::path::{Path, PathBuf};

use super::Heard;
use crate::hash::fnv;
use crate::Result;

/// Where hearing keeps the window embeddings, in the project folder.
pub(super) const EMBEDDINGS_FILE: &str = "windows.f32";

/// Why a recording heard by voice has no windows to label or sample from: heard by an older Junctur
/// that did not keep them, or whose windows may be the GPU's and not CAM++'s.
pub(super) const HEARD_BY_AN_OLDER_JUNCTUR: &str = "this recording's speakers were found by an older Junctur: analyse them again first (`junctur analyze`, or \"Analyse speakers again\" in the app)";

/// `windows.f32` in `dir` when it is the one `heard` was written with, by this build's hearing
/// ([`hearing_key`]): windows from another one, such as GPU-made ones, are never labelled
/// with or taken as a voice sample (a voice remembered from them would match nobody).
pub(super) fn heard_embeddings(heard: &Heard, dir: &Path) -> Option<Vec<Vec<f32>>> {
    if !heard.is_current() {
        return None;
    }
    let b = std::fs::read(embeddings_path(dir)).ok()?;
    if format!("{:016x}", fnv(&b)) != heard.embeddings {
        return None;
    }
    let e = decode_embeddings(&b, heard.dim)?;
    (e.len() == heard.windows.len()).then_some(e)
}

pub(super) fn embeddings_path(dir: &Path) -> PathBuf {
    dir.join(EMBEDDINGS_FILE)
}

/// Raw little-endian f32, [`Heard::dim`] per window, like `envelope.f32`. Returns the file's
/// [`Heard::embeddings`] key.
pub(super) fn save_embeddings(dir: &Path, e: &[Vec<f32>]) -> Result<String> {
    let bytes: Vec<u8> = e.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(embeddings_path(dir), &bytes)?;
    Ok(format!("{:016x}", fnv(&bytes)))
}

/// Embeddings of `dim` numbers from raw bytes; `None` when not a whole number of them.
pub(super) fn decode_embeddings(b: &[u8], dim: usize) -> Option<Vec<Vec<f32>>> {
    if dim == 0 {
        return b.is_empty().then(Vec::new);
    }
    if !b.len().is_multiple_of(dim * 4) {
        return None;
    }
    Some(
        b.chunks_exact(dim * 4)
            .map(|w| {
                w.chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect()
            })
            .collect(),
    )
}
