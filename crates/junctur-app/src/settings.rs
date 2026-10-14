//! The app's Settings (2i): what the Settings dialog shows and saves, kept in the user's
//! `junctur/config.json` (core `config`), the file `junctur analyze` reads, so the CLI and the app
//! transcribe with the same model and cut captions with the same defaults.
//!
//! It also holds the word list Whisper is told (5h), which a word fixed in the transcript joins
//! ([`learn`]).
//!
//! Its own module because it is the one place the app writes the config file. The analysis only
//! reads it when a run starts (`analysis.rs`, through `config::analysis_options`), so a changed
//! model applies to the next run and never starts one; the open recording learns of it through 2d's
//! out-of-date notice.

use junctur_core::config::{self, Config};
use junctur_core::edit::{Captions, ExistingClips};
use junctur_core::transcribe::ModelPref;
use junctur_core::wordlist;
use serde::{Deserialize, Serialize};

/// A Whisper model the dialog offers.
///
/// The picker (2v) shows it by a readable name, a line under it saying how fast, how big or where it
/// runs, and its family's icon; the Whisper model it is and the longer note go in the (i). The names
/// live here beside the keys rather than in the UI, so the list has one source, and the config file
/// keeps holding the keys it always held.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    /// What the config file holds (`ModelPref::key`).
    pub key: &'static str,
    /// What the user reads: "Fast", "Best".
    pub name: &'static str,
    /// The line under the name: speed, size or where it runs.
    pub summary: &'static str,
    /// The icon the picker draws, one per model family ("auto", "turbo", "large"), so the two
    /// large-v3 entries share one.
    pub family: &'static str,
    /// The Whisper model it loads (`ModelPref::name`), as Properties and the transcript name it.
    pub model: String,
    pub note: &'static str,
}

/// Every model, the default first: preference, family, name, summary, note. Turbo is the default and
/// large-v3 opt-in (1f).
const MODELS: [(ModelPref, &str, &str, &str, &str); 4] = [
    (
        ModelPref::Auto,
        "auto",
        "Automatic",
        "Fast, on the GPU when it has room",
        "On the GPU when 2.1 GB of its memory is free, else on the CPU.",
    ),
    (
        ModelPref::LargeV3Turbo,
        "turbo",
        "Fast",
        "The quickest, about 2.1 GB of GPU memory",
        "The fastest; close to large-v3, weaker at telling Spanish from English on short lines.",
    ),
    (
        ModelPref::LargeV3,
        "large",
        "Best",
        "The most accurate, slower, about 4.8 GB",
        "The best quality, and slower: wants about 4.8 GB of free GPU memory.",
    ),
    (
        ModelPref::LargeV3Q5,
        "large",
        "Best, smaller",
        "Close to Best in about 2.2 GB",
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
    /// The word list (5h), in its order: the first entries win when Whisper's prompt is full.
    #[serde(default)]
    pub words: Vec<String>,
    /// The transcript panel flags a word Whisper was less sure of than this many percent (5o); 0
    /// flags none.
    #[serde(default = "flag_below")]
    pub flag_below: u8,
}

fn flag_below() -> u8 {
    config::FLAG_BELOW
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
        .map(|&(m, family, name, summary, note)| ModelChoice {
            key: m.key(),
            name,
            summary,
            family,
            model: m.name(),
            note,
        })
        .collect()
}

fn from_config(c: &Config) -> Settings {
    Settings {
        whisper_model: c.whisper_model.unwrap_or_default().key().to_string(),
        captions: c.captions.clone().unwrap_or_default(),
        words: c.words.clone(),
        flag_below: c.flag_below(),
    }
}

/// Defaults are not written, so the file holds only what the user moved off them.
fn to_config(s: &Settings) -> Result<Config, String> {
    let model: ModelPref = s.whisper_model.parse()?;
    s.captions
        .check()
        .map_err(|e| format!("caption defaults: {e}"))?;
    // What to do with clips in the way in Resolve (5n) is remembered per recording only: "Save as
    // default" from a recording that remembered one starts the next asking.
    let captions = Captions {
        existing_clips: ExistingClips::Ask,
        ..s.captions.clone()
    };
    if s.flag_below > 100 {
        return Err("flag words under at most 100 %".into());
    }
    Ok(Config {
        whisper_model: (model != ModelPref::Auto).then_some(model),
        captions: (captions != Captions::default()).then_some(captions),
        words: wordlist::normalize(&s.words),
        flag_below: (s.flag_below != config::FLAG_BELOW).then_some(u64::from(s.flag_below)),
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

/// One reader-and-writer of the config file at a time in the app (5h repair): a word learned from a
/// fix (read, add, save) and the Captions panel's "Save as default" (read, change, save) running
/// together would each save over the other's change. Every write here holds it from the read on.
static CONFIG: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn config_lock() -> std::sync::MutexGuard<'static, ()> {
    CONFIG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Writes `s` as the config file, whole (it holds nothing else), and returns what it now says.
pub fn save(s: &Settings) -> Result<SettingsView, String> {
    let _one = config_lock();
    save_now(s)
}

fn save_now(s: &Settings) -> Result<SettingsView, String> {
    let c = to_config(s)?;
    config::save(&c).map_err(|e| e.to_string())?;
    Ok(view(Ok(c)))
}

/// The Captions panel's "Save as default": only the caption defaults change. A config file that
/// cannot be read is not replaced from here (the dialog says why and can replace it).
pub fn save_caption_defaults(captions: Captions) -> Result<SettingsView, String> {
    let _one = config_lock();
    let now = config::load().map_err(|e| e.to_string())?;
    save_now(&Settings {
        captions,
        ..from_config(&now)
    })
}

/// Adds `entries` (what a fix teaches: `wordlist::learned_in`, `learned_all_in`, worked out by the
/// player, which has the recording's words, 5aa) at the top of the word list in the order given
/// ([`wordlist::add_first`]) and saves it once; the entries added (none already there). A config file
/// that cannot be read is not replaced from here.
pub fn learn(entries: &[String]) -> Result<Vec<String>, String> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let _one = config_lock();
    let mut c = config::load().map_err(|e| e.to_string())?;
    // Last first, so the first given ends up on top.
    let mut added: Vec<String> = entries
        .iter()
        .rev()
        .filter(|e| wordlist::add_first(&mut c.words, e))
        .cloned()
        .collect();
    if added.is_empty() {
        return Ok(added);
    }
    config::save(&c).map_err(|e| e.to_string())?;
    added.reverse();
    Ok(added)
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
    fn every_model_has_its_own_name_and_an_icon_the_picker_draws() {
        let m = models();
        for (i, c) in m.iter().enumerate() {
            assert!(
                ["auto", "turbo", "large"].contains(&c.family),
                "{}",
                c.family
            );
            assert!(!c.summary.is_empty() && !c.model.is_empty());
            assert!(m[..i].iter().all(|o| o.name != c.name), "{}", c.name);
        }
        assert_eq!(m[0].name, "Automatic");
        assert_eq!(m[0].model, "large-v3-turbo");
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
            words: vec!["Uezar".into(), "Tsunheto".into()],
            flag_below: 25,
        };
        let c = to_config(&s).unwrap();
        assert_eq!(c.whisper_model, Some(ModelPref::LargeV3));
        assert_eq!(from_config(&c), s);
    }

    #[test]
    fn a_recordings_remembered_choice_for_clips_in_resolve_never_becomes_a_default() {
        // 5n: "Save as default" from a recording that remembered "new track" saves asking.
        let s = Settings {
            whisper_model: "auto".into(),
            captions: Captions {
                on: true,
                existing_clips: ExistingClips::NewTrack,
                ..Default::default()
            },
            words: vec![],
            flag_below: config::FLAG_BELOW,
        };
        let saved = to_config(&s).unwrap().captions.unwrap();
        assert!(saved.on);
        assert_eq!(saved.existing_clips, ExistingClips::Ask);
        // Only the choice differed from the defaults: nothing is written.
        let only = Settings {
            captions: Captions {
                existing_clips: ExistingClips::Replace,
                ..Default::default()
            },
            ..s
        };
        assert_eq!(to_config(&only).unwrap().captions, None);
    }

    #[test]
    fn the_word_list_is_saved_trimmed_without_blanks_or_repeats() {
        let s = Settings {
            whisper_model: "auto".into(),
            captions: Captions::default(),
            words: vec![" Uezar ".into(), "".into(), "Delay".into(), "Uezar".into()],
            flag_below: config::FLAG_BELOW,
        };
        assert_eq!(
            to_config(&s).unwrap().words,
            vec!["Uezar".to_string(), "Delay".to_string()]
        );
        // An empty list is not written.
        let none = Settings {
            words: vec!["  ".into()],
            ..s
        };
        assert_eq!(to_config(&none).unwrap(), Config::default());
    }

    #[test]
    fn a_bad_model_or_out_of_limits_captions_are_refused() {
        let s = Settings {
            whisper_model: "large-v4".into(),
            captions: Captions::default(),
            words: vec![],
            flag_below: config::FLAG_BELOW,
        };
        assert!(to_config(&s).is_err());
        let s = Settings {
            whisper_model: "auto".into(),
            captions: Captions {
                chars: 2,
                ..Default::default()
            },
            words: vec![],
            flag_below: config::FLAG_BELOW,
        };
        assert!(to_config(&s).unwrap_err().contains("caption defaults"));
    }

    /// 5o: the flag threshold is written only off its default, 0 (none flagged) included, and a
    /// percentage over 100 is refused.
    #[test]
    fn the_flag_threshold_is_saved_off_its_default_only() {
        let s = from_config(&Config::default());
        assert_eq!(s.flag_below, config::FLAG_BELOW);
        assert_eq!(to_config(&s).unwrap().flag_below, None);
        let off = Settings {
            flag_below: 0,
            ..s.clone()
        };
        assert_eq!(to_config(&off).unwrap().flag_below, Some(0));
        assert_eq!(from_config(&to_config(&off).unwrap()).flag_below, 0);
        let bad = Settings {
            flag_below: 101,
            ..s.clone()
        };
        assert!(to_config(&bad).is_err());
        // Settings sent without it (a page from before 5o): the default.
        let mut v = serde_json::to_value(Settings { flag_below: 7, ..s }).unwrap();
        v.as_object_mut().unwrap().remove("flagBelow").unwrap();
        let old: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(old.flag_below, config::FLAG_BELOW);
    }
}
