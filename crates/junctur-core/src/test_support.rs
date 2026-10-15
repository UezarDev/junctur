//! Helpers the unit tests share.

use std::path::PathBuf;

/// A fresh, empty `junctur-<kind>-<name>-<pid>` in the temp folder.
pub fn scratch(kind: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("junctur-{kind}-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
