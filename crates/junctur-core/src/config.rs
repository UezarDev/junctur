//! The user's config file (`JUNCTUR_CONFIG`, else `junctur/config.json` in the OS config folder)
//! and the pipeline options the CLI and the app both start from. A broken or named-but-missing file
//! is an error, so a typo never silently falls back to a different model.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::edit::Captions;
use crate::labels;
use crate::pipeline::PipelineOptions;
use crate::transcribe::ModelPref;
use crate::{hidden, voices, Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Whisper model used when the command line names none (`auto` when this is unset too).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisper_model: Option<ModelPref>,
    /// The caption settings of a recording whose edit list has none of its own ([`captions`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captions: Option<Captions>,
    /// Names and terms Whisper is told before every chunk ([`crate::wordlist`]); read when a
    /// transcription starts, so a change applies to the next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    /// Flag a word for review when its lowest token probability is under this many percent; 0 flags
    /// none. Any whole number reads (300 clamps to 100), so one bad value cannot break the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag_below: Option<u64>,
    /// Labels for the parts where nobody talks ([`labels`]): off, generic, nothing added, unless set.
    #[serde(default, skip_serializing_if = "is_default")]
    pub labels: labels::Settings,
}

fn is_default(s: &labels::Settings) -> bool {
    s == &labels::Settings::default()
}

/// The default of [`Config::flag_below`], in percent: flags about one word in twenty, nearly all
/// of them misheard or made-up names.
pub const FLAG_BELOW: u8 = 25;

impl Config {
    /// [`Self::flag_below`], or its default; at most 100 (every word with a confidence).
    pub fn flag_below(&self) -> u8 {
        self.flag_below.map_or(FLAG_BELOW, |p| p.min(100) as u8)
    }
}

/// The user's Junctur folder in the OS config folder (`%APPDATA%\junctur`), where the config file,
/// the voices and the app's recent recordings live; `None` when there is no config folder.
pub fn folder() -> Option<PathBuf> {
    crate::paths::config_home().map(|b| b.join("junctur"))
}

/// The files of [`folder`] core reads, each with the variable that points it elsewhere.
pub const FILES: [(&str, &str); 2] = [
    ("config.json", "JUNCTUR_CONFIG"),
    ("voices.json", voices::ENV),
];

/// Takes into [`folder`] those of `files` (name, overriding variable) that a packaged app's hidden
/// copy holds and it lacks ([`hidden`]), so settings saved from a sandboxed run reach every run.
/// A file whose variable is set is left alone. Called once at startup, never by a path lookup.
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

/// Where the config file this environment points at is (whether or not it exists yet), for saving.
pub fn path() -> Result<PathBuf> {
    locate(std::env::var_os("JUNCTUR_CONFIG"), folder())?
        .map(|(p, _)| p)
        .ok_or_else(|| {
            Error::Config("no config folder to keep settings in (set JUNCTUR_CONFIG)".into())
        })
}

/// Writes `c` as the whole config file this environment points at; returns where.
pub fn save(c: &Config) -> Result<PathBuf> {
    let path = path()?;
    save_to(&path, c)?;
    Ok(path)
}

fn save_to(path: &Path, c: &Config) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_vec_pretty(c)?;
    text.push(b'\n');
    crate::output::write_replacing(path, text)?;
    Ok(())
}

/// The caption settings a list cuts its cues with: its own (`own`), else the config file's
/// defaults, else the built-in ones. `cfg` is called only for a list without its own, so a broken
/// config file cannot block one that has them.
pub fn captions(own: Option<Captions>, cfg: impl FnOnce() -> Result<Config>) -> Result<Captions> {
    match own {
        Some(c) => Ok(c),
        None => Ok(cfg()?.captions.unwrap_or_default()),
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

/// The model and word list a transcription runs with: the model as [`whisper_model`] says; the
/// word list is the config file's, empty when an explicit model meets a file that cannot be read
/// (that file cannot block the choice, and its list is not known).
fn transcription(flag: Option<&str>, cfg: Result<Config>) -> Result<(ModelPref, Vec<String>)> {
    match (flag, cfg) {
        (Some(_), cfg) => Ok((
            whisper_model(flag, || Ok(Config::default()))?,
            cfg.map(|c| c.words).unwrap_or_default(),
        )),
        (None, cfg) => {
            let c = cfg?;
            Ok((c.whisper_model.unwrap_or_default(), c.words))
        }
    }
}

/// The options `junctur analyze` runs with before its own flags, and the app's analysis with: the
/// model and word list ([`transcription`]; the config file is not read for them without a transcript,
/// since no model is loaded), the labels settings (off when the file cannot be read: an extra, never a
/// reason to stop) and the remembered voices (not read without speakers, so a broken voices file cannot
/// block the rest). Device, cut settings and `force` stay the defaults for the caller to set.
pub fn analysis_options(
    model_flag: Option<&str>,
    no_transcript: bool,
    no_speakers: bool,
) -> Result<PipelineOptions> {
    let (model, words) = if no_transcript {
        (ModelPref::Auto, Vec::new())
    } else {
        transcription(model_flag, load())?
    };
    Ok(PipelineOptions {
        model,
        words,
        labels: load().map(|c| c.labels).unwrap_or_default(),
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
/// Caption defaults are held to the edit list's own limits.
fn parse(bytes: &[u8]) -> std::result::Result<Config, String> {
    let c: Config = serde_json::from_slice(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
        .map_err(|e| e.to_string())?;
    if let Some(cap) = &c.captions {
        cap.check().map_err(|e| format!("captions: {e}"))?;
    }
    Ok(c)
}

#[cfg(test)]
mod tests;
