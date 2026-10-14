//! Helpers the integration tests share; each test file uses some of them.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// A fresh, empty `junctur-<kind>-<name>-<pid>` in the temp folder, as the unit tests' `scratch`.
pub fn workdir(kind: &str, name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("junctur-{kind}-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Runs ffmpeg quietly, overwriting, and fails the test when it fails.
pub fn ffmpeg(args: &[&str]) {
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg on PATH")
        .success();
    assert!(ok, "ffmpeg failed: {args:?}");
}
