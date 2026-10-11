//! The optional user config file, `junctur/config.json` in the OS config folder (or the file named by
//! `JUNCTUR_CONFIG`). A missing file means defaults; a broken one is an error, so a typo does not
//! silently fall back to a different model.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::whisper::Model;
use crate::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Whisper model used when the command line names none.
    pub whisper_model: Option<Model>,
    /// Folder holding the ggml model files; None means [`crate::whisper::default_models_dir`].
    pub models_dir: Option<PathBuf>,
}

pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("JUNCTUR_CONFIG") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("junctur").join("config.json")
}

pub fn load(path: &Path) -> Result<Config> {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes).map_err(|e| Error::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e.into()),
    }
}

fn parse(bytes: &[u8]) -> std::result::Result<Config, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_best_quality_model() {
        let c = parse(br#"{"whisper_model":"large-v3"}"#).unwrap();
        assert_eq!(c.whisper_model, Some(Model::LargeV3));
        assert_eq!(parse(b"{}").unwrap(), Config::default());
    }

    #[test]
    fn typos_are_errors() {
        assert!(parse(br#"{"whisper_model":"large-v4"}"#).is_err());
        assert!(parse(br#"{"whisper_modle":"large-v3"}"#).is_err());
    }

    #[test]
    fn missing_file_is_defaults() {
        let p = std::env::temp_dir().join("junctur-no-such-config.json");
        assert_eq!(load(&p).unwrap(), Config::default());
    }
}
