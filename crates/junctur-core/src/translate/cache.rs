//! The translations made for a recording, kept in its project folder (`translations.json`), so a
//! second export, a send to Resolve or the preview never translates a sentence again. Keyed by the
//! target language and the sentence as the captions would show it untranslated: a cut that changes
//! a sentence's words makes a new key, and the old one stays, so undoing the cut finds it again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{models, Result};

pub const FILE: &str = "translations.json";

/// Bump when what a translation is made from changes (the sentence rules, the decoding), so old
/// ones stop being used.
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Translations {
    version: u32,
    /// The model file that made them: another model's are not mixed in.
    model: String,
    /// Target language, then sentence, then its translation.
    to: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for Translations {
    fn default() -> Self {
        Self {
            version: VERSION,
            model: models::TRANSLATE_DECODER.file.into(),
            to: BTreeMap::new(),
        }
    }
}

pub fn path(project: &Path) -> PathBuf {
    project.join(FILE)
}

impl Translations {
    /// The project's translations; none when there is no file, or one from another version or
    /// model (made again as they are asked for), or one that does not read.
    pub fn load(project: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path(project)) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(t) if t.version == VERSION && t.model == Self::default().model => t,
            _ => Self::default(),
        }
    }

    /// Written whole to a temporary file and renamed over the old one, so an app reading it for the
    /// preview never sees half a file.
    pub fn save(&self, project: &Path) -> Result<()> {
        let to = path(project);
        let part = project.join(format!("{FILE}.part"));
        std::fs::write(&part, serde_json::to_string(self)?)?;
        std::fs::rename(&part, &to)?;
        Ok(())
    }

    pub fn get(&self, to: &str, sentence: &str) -> Option<&str> {
        self.to.get(to)?.get(sentence).map(String::as_str)
    }

    pub fn insert(&mut self, to: &str, sentence: &str, shown: &str) {
        self.to
            .entry(to.to_string())
            .or_default()
            .insert(sentence.to_string(), shown.to_string());
    }

    /// The sentences of `sentences` with no translation into `to` yet, each once, in order.
    pub fn missing<'a>(
        &self,
        to: &str,
        sentences: impl IntoIterator<Item = &'a str>,
    ) -> Vec<&'a str> {
        let mut seen = std::collections::HashSet::new();
        sentences
            .into_iter()
            .filter(|s| self.get(to, s).is_none() && seen.insert(*s))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("junctur-tr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn translations_round_trip_and_another_model_is_not_used() {
        let d = dir("cache");
        assert_eq!(Translations::load(&d), Translations::default());
        let mut t = Translations::default();
        t.insert("fr", "Hola, ¿qué tal?", "Bonjour, comment ça va ?");
        t.insert("en", "Hola, ¿qué tal?", "Hi, how are you?");
        t.save(&d).unwrap();
        let back = Translations::load(&d);
        assert_eq!(back, t);
        assert_eq!(
            back.get("fr", "Hola, ¿qué tal?"),
            Some("Bonjour, comment ça va ?")
        );
        assert_eq!(back.get("de", "Hola, ¿qué tal?"), None);
        assert!(!d.join(format!("{FILE}.part")).exists());
        // Another model's or version's file, or a broken one, starts empty.
        let other = serde_json::to_string(&Translations {
            model: "other.onnx".into(),
            ..t.clone()
        })
        .unwrap();
        std::fs::write(path(&d), other).unwrap();
        assert_eq!(Translations::load(&d), Translations::default());
        std::fs::write(path(&d), "{").unwrap();
        assert_eq!(Translations::load(&d), Translations::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_sentences_are_listed_once_in_order() {
        let mut t = Translations::default();
        t.insert("fr", "a", "A");
        assert_eq!(t.missing("fr", ["b", "a", "c", "b"]), ["b", "c"]);
        assert_eq!(t.missing("de", ["a"]), ["a"]);
    }
}
