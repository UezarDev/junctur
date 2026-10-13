//! The app's Settings (2i): what the Settings dialog shows and saves, kept in the user's
//! `junctur/config.json` (core `config`), the file `junctur analyze` reads, so the CLI and the app
//! transcribe with the same model and cut captions with the same defaults.
//!
//! Its own module because it is the one place the app writes the config file. The analysis only
//! reads it when a run starts (`analysis.rs`, through `config::analysis_options`), so a changed
//! model applies to the next run and never starts one; the open recording learns of it through 2d's
//! out-of-date notice.

use junctur_core::config::{self, Config};
use junctur_core::edit::Captions;
use junctur_core::transcribe::ModelPref;
use serde::{Deserialize, Serialize};

/// A Whisper model the dialog offers.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    /// What the config file holds (`ModelPref::key`).
    pub key: &'static str,
    pub label: &'static str,
    pub note: &'static str,
}

/// Every model, the default first. Turbo is the default and large-v3 opt-in (1f).
const MODELS: [(ModelPref, &str, &str); 4] = [
    (
        ModelPref::Auto,
        "Automatic (large-v3-turbo)",
        "large-v3-turbo, on the GPU when 2.1 GB of its memory is free, else on the CPU.",
    ),
    (
        ModelPref::LargeV3Turbo,
        "large-v3-turbo",
        "The fastest; close to large-v3, weaker at telling Spanish from English on short lines.",
    ),
    (
        ModelPref::LargeV3,
        "large-v3 (best)",
        "The best quality, and slower: wants about 4.8 GB of free GPU memory.",
    ),
    (
        ModelPref::LargeV3Q5,
        "large-v3 q5_0",
        "large-v3 quantised to 5 bits: its decoder in about 2.2 GB of GPU memory, a little more than turbo.",
    ),
];

/// What the dialog edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// A [`ModelChoice::key`].
    pub whisper_model: String,
    /// The caption settings a recording without its own starts from.
    pub captions: Captions,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub settings: Settings,
    pub models: Vec<ModelChoice>,
    /// The config file these come from and go to; `None` with no config folder at all.
    pub path: Option<String>,
    /// Why the file could not be read; the dialog shows the defaults then, and saving replaces it.
    pub error: Option<String>,
}

fn models() -> Vec<ModelChoice> {
    MODELS
        .iter()
        .map(|&(m, label, note)| ModelChoice {
            key: m.key(),
            label,
            note,
        })
        .collect()
}

fn from_config(c: &Config) -> Settings {
    Settings {
        whisper_model: c.whisper_model.unwrap_or_default().key().to_string(),
        captions: c.captions.clone().unwrap_or_default(),
    }
}

/// Defaults are not written, so the file holds only what the user moved off them.
fn to_config(s: &Settings) -> Result<Config, String> {
    let model: ModelPref = s.whisper_model.parse()?;
    s.captions
        .check()
        .map_err(|e| format!("caption defaults: {e}"))?;
    Ok(Config {
        whisper_model: (model != ModelPref::Auto).then_some(model),
        captions: (s.captions != Captions::default()).then(|| s.captions.clone()),
    })
}

fn view(c: Result<Config, String>) -> SettingsView {
    let (cfg, error) = match c {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(e)),
    };
    SettingsView {
        settings: from_config(&cfg),
        models: models(),
        path: config::path().ok().map(|p| p.display().to_string()),
        error,
    }
}

/// The settings as the config file has them now.
pub fn read() -> SettingsView {
    view(config::load().map_err(|e| e.to_string()))
}

/// Writes `s` as the config file, whole (it holds nothing else), and returns what it now says.
pub fn save(s: &Settings) -> Result<SettingsView, String> {
    let c = to_config(s)?;
    config::save(&c).map_err(|e| e.to_string())?;
    Ok(view(Ok(c)))
}

/// The Captions panel's "Save as default": only the caption defaults change. A config file that
/// cannot be read is not replaced from here (the dialog says why and can replace it).
pub fn save_caption_defaults(captions: Captions) -> Result<SettingsView, String> {
    let now = config::load().map_err(|e| e.to_string())?;
    save(&Settings {
        captions,
        ..from_config(&now)
    })
}

/// The caption defaults the player starts with; the built-in ones when the file cannot be read
/// (Settings says why).
pub fn caption_defaults() -> Captions {
    config::load()
        .ok()
        .and_then(|c| c.captions)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_is_one_the_config_reads_and_turbo_comes_first() {
        let m = models();
        assert_eq!(m[0].key, "auto");
        for c in &m {
            assert_eq!(c.key.parse::<ModelPref>().unwrap().key(), c.key);
        }
        assert_eq!(
            m.len(),
            ModelPref::NAMES
                .iter()
                .filter(|(n, p)| p.key() == *n)
                .count()
        );
    }

    #[test]
    fn defaults_are_not_written_and_the_rest_round_trips() {
        let plain = from_config(&Config::default());
        assert_eq!(plain.whisper_model, "auto");
        assert_eq!(to_config(&plain).unwrap(), Config::default());
        let s = Settings {
            whisper_model: "large-v3".into(),
            captions: Captions {
                on: true,
                ..Default::default()
            },
        };
        let c = to_config(&s).unwrap();
        assert_eq!(c.whisper_model, Some(ModelPref::LargeV3));
        assert_eq!(from_config(&c), s);
    }

    #[test]
    fn a_bad_model_or_out_of_limits_captions_are_refused() {
        let s = Settings {
            whisper_model: "large-v4".into(),
            captions: Captions::default(),
        };
        assert!(to_config(&s).is_err());
        let s = Settings {
            whisper_model: "auto".into(),
            captions: Captions {
                chars: 2,
                ..Default::default()
            },
        };
        assert!(to_config(&s).unwrap_err().contains("caption defaults"));
    }
}
