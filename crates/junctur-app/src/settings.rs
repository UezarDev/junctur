//! The app's Settings dialog, kept in the user's `junctur/config.json`, the file `junctur analyze`
//! also reads, with the word list Whisper is told ([`learn`]). Its own module because it is the one
//! place the app writes the config file; a changed model applies from the next run.

use junctur_core::config::{self, Config};
use junctur_core::edit::{Captions, ExistingClips};
use junctur_core::labels::{self, Kind};
use junctur_core::transcribe::ModelPref;
use junctur_core::wordlist;
use serde::{Deserialize, Serialize};

/// A Whisper model the dialog offers, with the names the picker shows. They live here beside the
/// keys rather than in the UI, so the list has one source.
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

/// Every model, the default first: preference, family, name, summary, note.
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
    /// The word list, in its order: the first entries win when Whisper's prompt is full.
    #[serde(default)]
    pub words: Vec<String>,
    /// The transcript panel flags a word Whisper was less sure of than this many percent; 0 flags
    /// none.
    #[serde(default = "flag_below")]
    pub flag_below: u8,
    /// Labels for the parts where nobody talks: off by default; the kind and the labels added.
    #[serde(default)]
    pub labels: labels::Settings,
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
    /// Each kind of video the labels offer, with its built-in labels.
    pub label_kinds: Vec<LabelKind>,
}

/// A kind of video for the labels, as the dialog lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelKind {
    /// What the config file holds.
    pub key: Kind,
    pub name: &'static str,
    /// Its built-in labels, as shown.
    pub builtin: Vec<&'static str>,
}

fn label_kinds() -> Vec<LabelKind> {
    Kind::ALL
        .into_iter()
        .map(|k| LabelKind {
            key: k,
            name: match k {
                Kind::Generic => "Any game",
                Kind::League => "League of Legends",
            },
            builtin: k.builtin().into_iter().map(|(text, _)| text).collect(),
        })
        .collect()
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
        labels: c.labels.clone(),
    }
}

/// Defaults are not written, so the file holds only what the user moved off them.
fn to_config(s: &Settings) -> Result<Config, String> {
    let model: ModelPref = s.whisper_model.parse()?;
    s.captions
        .check()
        .map_err(|e| format!("caption defaults: {e}"))?;
    // What to do with clips in the way in Resolve is remembered per recording only: "Save as
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
        labels: labels::Settings {
            added: labels::Added {
                generic: added(&s.labels.added.generic),
                league: added(&s.labels.added.league),
            },
            ..s.labels.clone()
        },
    })
}

/// Added labels as the box gave them, trimmed, blank lines and repeats dropped.
fn added(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(l)) {
            out.push(l.to_string());
        }
    }
    out
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
        label_kinds: label_kinds(),
    }
}

/// The settings as the config file has them now.
pub fn read() -> SettingsView {
    view(config::load().map_err(|e| e.to_string()))
}

/// One reader-and-writer of the config file at a time: two read-change-saves running together would
/// each save over the other's change. Every write here holds it from the read on.
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

/// Adds `entries` (what a fix teaches, worked out by the player) at the top of the word list in the
/// order given ([`wordlist::add_first`]) and saves it once; the entries added (none already there).
/// A config file that cannot be read is not replaced from here.
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
mod tests;
