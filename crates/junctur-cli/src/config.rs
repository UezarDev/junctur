//! The optional user config file: the file named by `JUNCTUR_CONFIG`, else `junctur/config.json` in
//! the OS config folder. A missing default file means defaults; a broken file, or a `JUNCTUR_CONFIG`
//! that names nothing, is an error, so a typo never silently falls back to a different model.
//!
//! In the CLI rather than the engine: library callers pass their options directly, so the engine
//! never changes behaviour because of a file or variable its caller did not name.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use junctur_core::transcribe::ModelPref;
use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Whisper model used when the command line names none (`auto` when this is unset too).
    pub whisper_model: Option<ModelPref>,
}

/// Read the config file this environment points at.
pub fn load() -> Result<Config, String> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    match locate(std::env::var_os("JUNCTUR_CONFIG"), base)? {
        Some((path, named)) => load_from(&path, named),
        None => Ok(Config::default()),
    }
}

/// The file to read and whether the user named it. None when there is no config folder at all:
/// then there is no default file either (not a temp-folder stand-in someone else could write).
fn locate(var: Option<OsString>, base: Option<PathBuf>) -> Result<Option<(PathBuf, bool)>, String> {
    match var {
        Some(v) if v.is_empty() => Err("JUNCTUR_CONFIG is set but empty".into()),
        Some(v) => Ok(Some((PathBuf::from(v), true))),
        None => Ok(base.map(|b| (b.join("junctur").join("config.json"), false))),
    }
}

/// `named`: the user pointed at this file, so it must exist.
fn load_from(path: &Path, named: bool) -> Result<Config, String> {
    let bad = |e: &dyn std::fmt::Display| format!("config file {}: {e}", path.display());
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes).map_err(|e| bad(&e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !named => Ok(Config::default()),
        Err(e) => Err(bad(&e)),
    }
}

/// A UTF-8 byte order mark is skipped: Windows PowerShell 5.1's `Out-File`/`Set-Content` write one.
fn parse(bytes: &[u8]) -> Result<Config, serde_json::Error> {
    serde_json::from_slice(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_models_and_aliases() {
        let c = parse(br#"{"whisper_model":"large-v3"}"#).unwrap();
        assert_eq!(c.whisper_model, Some(ModelPref::LargeV3));
        let c = parse(br#"{"whisper_model":"turbo"}"#).unwrap();
        assert_eq!(c.whisper_model, Some(ModelPref::LargeV3Turbo));
        assert_eq!(parse(b"{}").unwrap(), Config::default());
    }

    #[test]
    fn skips_a_byte_order_mark() {
        let c = parse(b"\xEF\xBB\xBF{\"whisper_model\":\"best\"}").unwrap();
        assert_eq!(c.whisper_model, Some(ModelPref::LargeV3));
    }

    #[test]
    fn typos_are_errors() {
        assert!(parse(br#"{"whisper_model":"large-v4"}"#).is_err());
        assert!(parse(br#"{"whisper_modle":"large-v3"}"#).is_err());
        // PR #1's `models_dir` is not supported: models live in the cache folder.
        assert!(parse(br#"{"models_dir":"D:/m"}"#).is_err());
    }

    #[test]
    fn where_the_file_is() {
        let base = PathBuf::from("cfg");
        assert_eq!(
            locate(None, Some(base.clone())).unwrap(),
            Some((base.join("junctur").join("config.json"), false))
        );
        assert_eq!(locate(None, None).unwrap(), None);
        assert_eq!(
            locate(Some("x.json".into()), Some(base)).unwrap(),
            Some((PathBuf::from("x.json"), true))
        );
        assert!(locate(Some(OsString::new()), None).is_err());
    }

    #[test]
    fn missing_default_is_defaults_but_missing_named_file_and_broken_file_are_errors() {
        let d = std::env::temp_dir().join(format!("junctur-config-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let none = d.join("none.json");
        assert_eq!(load_from(&none, false).unwrap(), Config::default());
        let e = load_from(&none, true).unwrap_err();
        assert!(e.contains("none.json"), "{e}");
        let bad = d.join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        let e = load_from(&bad, false).unwrap_err();
        assert!(e.contains("bad.json"), "{e}");
        // Any other read failure (here: a folder) names the path too.
        let e = load_from(&d, false).unwrap_err();
        assert!(e.contains("junctur-config-"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
