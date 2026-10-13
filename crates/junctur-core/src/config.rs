//! The user's settings: the optional config file (the file named by `JUNCTUR_CONFIG`, else
//! `junctur/config.json` in the OS config folder) and the pipeline options `junctur analyze` and the
//! app both start from ([`analysis_options`]). A missing default file means defaults; a broken file,
//! or a `JUNCTUR_CONFIG` that names nothing, is an error, so a typo never silently falls back to a
//! different model.
//!
//! The engine itself never reads these: [`crate::pipeline::run`] takes its options, and a caller that
//! wants the user's asks for them here. Moved from the CLI (2d) so the app builds its analysis from
//! the same rules instead of a copy.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::pipeline::PipelineOptions;
use crate::transcribe::ModelPref;
use crate::{hidden, voices, Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Whisper model used when the command line names none (`auto` when this is unset too).
    pub whisper_model: Option<ModelPref>,
}

/// The user's Junctur folder in the OS config folder (`%APPDATA%\junctur`), where the config file,
/// the voices (5b) and the app's recent recordings (2d) live; `None` when there is no config folder.
pub fn folder() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|b| b.join("junctur"))
}

/// The files of [`folder`] core reads, each with the variable that points it elsewhere.
pub const FILES: [(&str, &str); 2] = [
    ("config.json", "JUNCTUR_CONFIG"),
    ("voices.json", voices::ENV),
];

/// Takes into [`folder`] those of `files` (name, overriding variable) that a packaged app's hidden copy
/// of it holds and it lacks (2f, [`hidden`]), so settings and voices saved from a Claude-launched run
/// reach every run. A file whose variable is set is not the folder's and is left alone. Called once at
/// startup by the CLI and the app, never by a path lookup.
pub fn adopt_hidden(files: &[(&str, &str)]) {
    let Some(folder) = folder() else {
        return;
    };
    let wanted: Vec<&str> = files
        .iter()
        .filter(|(_, var)| std::env::var_os(var).is_none())
        .map(|(name, _)| *name)
        .collect();
    if !wanted.is_empty() {
        hidden::adopt_files_once(&folder, |rel| wanted.iter().any(|w| rel == Path::new(w)));
    }
}

/// Read the config file this environment points at.
pub fn load() -> Result<Config> {
    match locate(std::env::var_os("JUNCTUR_CONFIG"), folder())? {
        Some((path, named)) => load_from(&path, named),
        None => Ok(Config::default()),
    }
}

/// `flag` (a `--model` value), else the config file's `whisper_model`, else `auto`. `cfg` is called
/// only when there is no flag, so a broken config file cannot block an explicit choice.
pub fn whisper_model(
    flag: Option<&str>,
    cfg: impl FnOnce() -> Result<Config>,
) -> Result<ModelPref> {
    match flag {
        Some(m) => m
            .parse()
            .map_err(|e| Error::Config(format!("--model: {e}"))),
        None => Ok(cfg()?.whisper_model.unwrap_or_default()),
    }
}

/// The options `junctur analyze` runs with before its own flags, and the app's analysis with: the
/// model ([`whisper_model`]; the config file is not read without a transcript, since no model is
/// loaded) and the remembered voices (not read without speakers, so a broken voices file cannot
/// block the rest). Device, cut settings and `force` stay the defaults for the caller to set.
pub fn analysis_options(
    model_flag: Option<&str>,
    no_transcript: bool,
    no_speakers: bool,
) -> Result<PipelineOptions> {
    Ok(PipelineOptions {
        model: if no_transcript {
            ModelPref::Auto
        } else {
            whisper_model(model_flag, load)?
        },
        no_transcript,
        no_speakers,
        voices: if no_speakers {
            Vec::new()
        } else {
            voices::remembered()?
        },
        ..Default::default()
    })
}

/// The file to read and whether the user named it. None when there is no config folder at all:
/// then there is no default file either (not a temp-folder stand-in someone else could write).
fn locate(var: Option<OsString>, folder: Option<PathBuf>) -> Result<Option<(PathBuf, bool)>> {
    match var {
        Some(v) if v.is_empty() => Err(Error::Config("JUNCTUR_CONFIG is set but empty".into())),
        Some(v) => Ok(Some((PathBuf::from(v), true))),
        None => Ok(folder.map(|f| (f.join("config.json"), false))),
    }
}

/// `named`: the user pointed at this file, so it must exist.
fn load_from(path: &Path, named: bool) -> Result<Config> {
    let bad =
        |e: &dyn std::fmt::Display| Error::Config(format!("config file {}: {e}", path.display()));
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes).map_err(|e| bad(&e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !named => Ok(Config::default()),
        Err(e) => Err(bad(&e)),
    }
}

/// A UTF-8 byte order mark is skipped: Windows PowerShell 5.1's `Out-File`/`Set-Content` write one.
fn parse(bytes: &[u8]) -> std::result::Result<Config, serde_json::Error> {
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
    fn model_flag_beats_config_beats_auto() {
        let cfg = |m: Option<ModelPref>| move || Ok(Config { whisper_model: m });
        let broken = || Err(Error::Config("bad config".into()));
        assert_eq!(
            whisper_model(Some("turbo"), cfg(Some(ModelPref::LargeV3))).unwrap(),
            ModelPref::LargeV3Turbo
        );
        // An explicit choice never reads the config, so a broken one cannot block it.
        assert_eq!(
            whisper_model(Some("best"), broken).unwrap(),
            ModelPref::LargeV3
        );
        assert_eq!(
            whisper_model(None, cfg(Some(ModelPref::LargeV3Q5))).unwrap(),
            ModelPref::LargeV3Q5
        );
        assert_eq!(whisper_model(None, cfg(None)).unwrap(), ModelPref::Auto);
        assert!(whisper_model(None, broken).is_err());
        assert!(whisper_model(Some("large-v4"), cfg(None)).is_err());
    }

    #[test]
    fn where_the_file_is() {
        let base = PathBuf::from("cfg").join("junctur");
        assert_eq!(
            locate(None, Some(base.clone())).unwrap(),
            Some((base.join("config.json"), false))
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
        let e = load_from(&none, true).unwrap_err().to_string();
        assert!(e.contains("none.json"), "{e}");
        let bad = d.join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        let e = load_from(&bad, false).unwrap_err().to_string();
        assert!(e.contains("bad.json"), "{e}");
        // Any other read failure (here: a folder) names the path too.
        let e = load_from(&d, false).unwrap_err().to_string();
        assert!(e.contains("junctur-config-"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
