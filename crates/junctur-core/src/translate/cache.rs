//! The translations made for a recording, kept in its project folder (`translations.json`), so a
//! second export, a send to Resolve or the preview never translates a sentence again. Keyed by the
//! target language and the sentence as the captions would show it untranslated: a cut that changes
//! a sentence's words makes a new key, and the old one stays, so undoing the cut finds it again.
//! Each keeps the language it was translated from: a sentence heard again in another language (a
//! new transcript) is made again ([`Translations::needs`]), while showing one needs only its text,
//! which is all the cue cutting has.
//!
//! The CLI and the app may both translate one recording at once, so a save merges what is on disk
//! with its own, through a temporary file of its own; a file that is there but does not read is
//! never written over ([`Translations::read`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::{models, Error, Result};

pub const FILE: &str = "translations.json";

/// Bump when what a translation is made from changes (the sentence rules, the decoding), so old
/// ones stop being used. 2: each keeps its source language (5k repair).
const VERSION: u32 = 2;

/// A sentence's translation and the language it was translated from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    from: String,
    shown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Translations {
    version: u32,
    /// The model file that made them: another model's are not mixed in.
    model: String,
    /// Target language, then sentence, then its translation.
    to: BTreeMap<String, BTreeMap<String, Entry>>,
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
    /// The project's translations: none when there is no file, or one from another version or
    /// model (made again as they are asked for); an error for a file that is there and does not
    /// read, which a run must not write over.
    pub fn read(project: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path(project)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let unread = |e: serde_json::Error| {
            Error::Edit(format!("{} does not read: {e}", path(project).display()))
        };
        #[derive(Deserialize)]
        struct Stamp {
            version: u32,
            model: String,
        }
        let stamp: Stamp = serde_json::from_str(&text).map_err(unread)?;
        if stamp.version != VERSION || stamp.model != Self::default().model {
            return Ok(Self::default());
        }
        serde_json::from_str(&text).map_err(unread)
    }

    /// [`Translations::read`] for showing them: a file that does not read shows none.
    pub fn load(project: &Path) -> Self {
        Self::read(project).unwrap_or_default()
    }

    /// Written to a temporary file of this process's own and renamed over the old one, so a reader
    /// never sees half a file. What another process saved meanwhile is kept: the file on disk is
    /// read again and this one's entries laid over it. A file that does not read is an error.
    pub fn save(&self, project: &Path) -> Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut all = Self::read(project)?;
        for (to, sentences) in &self.to {
            let mine = all.to.entry(to.clone()).or_default();
            for (s, e) in sentences {
                mine.insert(s.clone(), e.clone());
            }
        }
        let part = project.join(format!(
            "{FILE}.{}-{}.part",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&part, serde_json::to_string(&all)?)?;
        if let Err(e) = std::fs::rename(&part, path(project)) {
            let _ = std::fs::remove_file(&part);
            return Err(e.into());
        }
        Ok(())
    }

    /// The translation into `to` of `sentence`, whatever it was translated from.
    pub fn get(&self, to: &str, sentence: &str) -> Option<&str> {
        self.entry(to, sentence).map(|e| e.shown.as_str())
    }

    fn entry(&self, to: &str, sentence: &str) -> Option<&Entry> {
        self.to.get(to)?.get(sentence)
    }

    pub fn insert(&mut self, from: &str, to: &str, sentence: &str, shown: &str) {
        self.to.entry(to.to_string()).or_default().insert(
            sentence.to_string(),
            Entry {
                from: from.to_string(),
                shown: shown.to_string(),
            },
        );
    }

    /// Whether `sentence`, heard in `from`, still needs a translation into `to`: none yet, or one
    /// made from another language.
    pub fn needs(&self, from: &str, to: &str, sentence: &str) -> bool {
        self.entry(to, sentence).is_none_or(|e| e.from != from)
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
        t.insert("es", "fr", "Hola, ¿qué tal?", "Bonjour, comment ça va ?");
        t.insert("es", "en", "Hola, ¿qué tal?", "Hi, how are you?");
        t.save(&d).unwrap();
        let back = Translations::load(&d);
        assert_eq!(back, t);
        assert_eq!(
            back.get("fr", "Hola, ¿qué tal?"),
            Some("Bonjour, comment ça va ?")
        );
        assert_eq!(back.get("de", "Hola, ¿qué tal?"), None);
        let files = std::fs::read_dir(&d).unwrap().count();
        assert_eq!(files, 1, "no temporary file left");
        // Another model's or version's file starts empty; a broken one reads as an error, and
        // is not saved over.
        let other = serde_json::to_string(&Translations {
            model: "other.onnx".into(),
            ..t.clone()
        })
        .unwrap();
        std::fs::write(path(&d), other).unwrap();
        assert_eq!(Translations::read(&d).unwrap(), Translations::default());
        std::fs::write(path(&d), "{").unwrap();
        assert!(Translations::read(&d).is_err());
        assert_eq!(Translations::load(&d), Translations::default());
        assert!(t.save(&d).is_err());
        assert_eq!(std::fs::read_to_string(path(&d)).unwrap(), "{");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_save_keeps_what_another_saved_meanwhile() {
        let d = dir("merge");
        let mut a = Translations::default();
        a.insert("es", "fr", "uno", "un");
        let mut b = Translations::default();
        b.insert("es", "fr", "dos", "deux");
        a.save(&d).unwrap();
        b.save(&d).unwrap();
        let back = Translations::load(&d);
        assert_eq!(
            (back.get("fr", "uno"), back.get("fr", "dos")),
            (Some("un"), Some("deux"))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_sentences_are_listed_once_and_a_new_source_language_is_made_again() {
        let mut t = Translations::default();
        t.insert("es", "fr", "a", "A");
        assert_eq!(t.missing("fr", ["b", "a", "c", "b"]), ["b", "c"]);
        assert_eq!(t.missing("de", ["a"]), ["a"]);
        assert!(!t.needs("es", "fr", "a"));
        assert!(t.needs("en", "fr", "a"), "heard in another language now");
        assert!(t.needs("es", "fr", "b"));
    }
}
