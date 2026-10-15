//! Where analysis results are cached, and under what key.
//!
//! Never beside the input: sources are read-only and their folder may not be writable.
//! Order: caller's dir, then `JUNCTUR_CACHE_DIR`, then the OS user cache dir.

use std::path::{Path, PathBuf};

use crate::Result;

/// Bump when the analysis or the report shape changes, so old entries stop matching.
pub const CACHE_VERSION: u32 = 2;

pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("JUNCTUR_CACHE_DIR") {
        return PathBuf::from(d);
    }
    let base = crate::paths::cache_home().unwrap_or_else(std::env::temp_dir);
    base.join("junctur").join("cache")
}

/// The file's modification time in nanoseconds since 1970, 0 when the OS has none.
pub fn mtime_nanos(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// Key from the file's identity (canonical path, size, mtime) and the analysis settings. A changed
/// or replaced file gets a new key; content is not hashed, since that would read all of a 30h file.
pub fn key(file: &Path, settings: &str) -> Result<String> {
    let meta = std::fs::metadata(file)?;
    let canon = std::fs::canonicalize(file)?;
    let mtime = mtime_nanos(&meta);
    let id = format!(
        "v{CACHE_VERSION}|{}|{}|{mtime}|{settings}",
        canon.to_string_lossy(),
        meta.len()
    );
    Ok(crate::hash::Fnv::new().bytes(id.as_bytes()).hex())
}

pub fn entry_path(dir: &Path, key: &str) -> PathBuf {
    dir.join("tracks").join(format!("{key}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_depends_on_settings() {
        let f = std::env::current_exe().unwrap();
        assert_ne!(key(&f, "a").unwrap(), key(&f, "b").unwrap());
        assert_eq!(key(&f, "a").unwrap(), key(&f, "a").unwrap());
    }
}
