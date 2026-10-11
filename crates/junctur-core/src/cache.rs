//! Where analysis results are cached, and under what key.
//!
//! Never beside the input: sources are read-only and their folder may not be writable.
//! Order: caller's dir, then `JUNCTUR_CACHE_DIR`, then the OS user cache dir.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::Result;

/// Bump when the analysis or the report shape changes, so old entries stop matching.
pub const CACHE_VERSION: u32 = 1;

pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("JUNCTUR_CACHE_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("junctur").join("cache")
}

/// FNV-1a 64. Hand-rolled because std's `DefaultHasher` is not stable across Rust releases, and a
/// cache key must survive a toolchain upgrade.
fn fnv(bytes: &[u8], mut h: u64) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Key from the file's identity (canonical path, size, mtime) and the analysis settings. A changed
/// or replaced file gets a new key; content is not hashed, since that would read all of a 30h file.
pub fn key(file: &Path, settings: &str) -> Result<String> {
    let meta = std::fs::metadata(file)?;
    let canon = std::fs::canonicalize(file)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let id = format!(
        "v{CACHE_VERSION}|{}|{}|{mtime}|{settings}",
        canon.to_string_lossy(),
        meta.len()
    );
    Ok(format!(
        "{:016x}",
        fnv(id.as_bytes(), 0xcbf2_9ce4_8422_2325)
    ))
}

/// `kind` keeps each stage's entries in its own folder.
pub fn entry_path(dir: &Path, kind: &str, key: &str) -> PathBuf {
    dir.join(kind).join(format!("{key}.json"))
}

/// A cached value, or None when it is missing or no longer parses.
pub fn read<T: DeserializeOwned>(entry: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(entry).ok()?).ok()
}

/// Store a value. A cache that cannot be written is not a failed run, so only encoding fails.
pub fn write<T: Serialize>(entry: &Path, value: &T) -> Result<()> {
    if let Some(parent) = entry.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(entry, serde_json::to_vec_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_matches_reference_vector() {
        assert_eq!(fnv(b"a", 0xcbf2_9ce4_8422_2325), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn key_depends_on_settings() {
        let f = std::env::current_exe().unwrap();
        assert_ne!(key(&f, "a").unwrap(), key(&f, "b").unwrap());
        assert_eq!(key(&f, "a").unwrap(), key(&f, "a").unwrap());
    }
}
