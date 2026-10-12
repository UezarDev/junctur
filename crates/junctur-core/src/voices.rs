//! Voices the user has told Junctur once (5b): a name, a caption colour and a voiceprint per person,
//! used to label every recording by name ([`crate::diarize::label`]).
//!
//! Its own file, `junctur/voices.json` in the OS config folder (or the file `JUNCTUR_VOICES` names),
//! beside the user's `config.json` rather than in it: `config.json` is hand-written, read by the CLI
//! and the app's analysis only, and strict about unknown keys, while this file is written by the app and the CLI and holds
//! 512 numbers per person. Kept apart, a hand edit of one and a save of the other never meet. The
//! engine reads it only when its caller passes the path ([`default_path`] says where it usually is).
//!
//! A voiceprint is the unit mean of a person's CAM++ window embeddings. It is only comparable with
//! embeddings of the same model, so each voice records its model and others are ignored.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diarize::normalize;
use crate::edit::{check_name, default_color, parse_color};
use crate::{models, Error, Result};

/// Names the voices file instead of the default one (tests and the app's test mode).
pub const ENV: &str = "JUNCTUR_VOICES";

/// Enrolling from less than this much speech gives a voiceprint too noisy to trust.
pub const MIN_SECONDS: f64 = 3.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    pub name: String,
    /// Caption colour, `#RRGGBB`: the person's in every recording.
    pub color: String,
    /// Seconds of speech the print was made from.
    pub seconds: f64,
    /// The embedding model's file name ([`models::SPEAKER_EMBEDDING`]).
    pub model: String,
    pub print: Vec<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Voices {
    pub voices: Vec<Voice>,
}

/// The voices file this environment points at: `JUNCTUR_VOICES` if set (empty is an error), else
/// `junctur/voices.json` in the OS config folder; `None` when there is no config folder at all.
pub fn default_path() -> Result<Option<PathBuf>> {
    match std::env::var_os(ENV) {
        Some(v) if v.is_empty() => Err(Error::Edit(format!("{ENV} is set but empty"))),
        Some(v) => Ok(Some(PathBuf::from(v))),
        None => Ok(crate::config::folder().map(|f| f.join("voices.json"))),
    }
}

/// The voices to label speakers by, from the voices file this environment points at: none when
/// there is no file yet; an error when there is no config folder to keep one in.
pub fn remembered() -> Result<Vec<Voice>> {
    let path = default_path()?
        .ok_or_else(|| Error::Config(format!("no config folder to keep voices in (set {ENV})")))?;
    let v = Voices::load(&path)?;
    Ok(v.usable().into_iter().cloned().collect())
}

impl Voices {
    /// The voices at `path`; none when the file does not exist. A file that does not parse is an
    /// error, never silently empty (saving over it would lose every voice).
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&b))
                .map_err(|e| Error::Edit(format!("voices file {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes the file whole through a temporary file, so a crash never leaves half of it.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The voices whose prints this build's embedding model can compare with.
    pub fn usable(&self) -> Vec<&Voice> {
        self.voices
            .iter()
            .filter(|v| v.model == models::SPEAKER_EMBEDDING.file && !v.print.is_empty())
            .collect()
    }

    fn find(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        self.voices
            .iter()
            .position(|v| v.name.eq_ignore_ascii_case(name))
    }

    pub fn get(&self, name: &str) -> Option<&Voice> {
        self.find(name).map(|i| &self.voices[i])
    }

    /// Remembers `name`'s voice from these window embeddings (`seconds` of speech). A name already
    /// known gets the sample added (its print becomes the mean of both, weighted by seconds), so a
    /// voice is built up from several selections: in a conversation a person rarely talks alone for
    /// the half minute a good print wants (5b's measurement), and one selection is one stretch.
    /// Forgetting starts over. A new person gets the first palette colour no other voice has.
    pub fn enroll(&mut self, name: &str, embeddings: &[&[f32]], seconds: f64) -> Result<&Voice> {
        let name = check_name(name).map_err(Error::Edit)?.to_string();
        if seconds < MIN_SECONDS || embeddings.is_empty() {
            return Err(Error::Edit(format!(
                "select at least {MIN_SECONDS:.0} s where only {name} talks to remember the voice (this has {seconds:.1} s)"
            )));
        }
        let print = voiceprint(embeddings);
        let model = models::SPEAKER_EMBEDDING.file.to_string();
        let i = match self.find(&name) {
            Some(i)
                if self.voices[i].model == model && self.voices[i].print.len() == print.len() =>
            {
                let v = &mut self.voices[i];
                let mut sum: Vec<f32> = v
                    .print
                    .iter()
                    .zip(&print)
                    .map(|(a, b)| a * v.seconds as f32 + b * seconds as f32)
                    .collect();
                normalize(&mut sum);
                v.print = sum;
                v.seconds += seconds;
                i
            }
            // Known, but from another model: the old print cannot be added to; start it again.
            Some(i) => {
                let v = &mut self.voices[i];
                v.print = print;
                v.seconds = seconds;
                v.model = model;
                i
            }
            None => {
                let color = (1..)
                    .map(default_color)
                    .find(|c| self.voices.iter().all(|v| &v.color != c))
                    .expect("some colour");
                self.voices.push(Voice {
                    name,
                    color,
                    seconds,
                    model,
                    print,
                });
                self.voices.len() - 1
            }
        };
        Ok(&self.voices[i])
    }

    /// Forgets `name`; false when there is no such voice.
    pub fn forget(&mut self, name: &str) -> bool {
        match self.find(name) {
            Some(i) => {
                self.voices.remove(i);
                true
            }
            None => false,
        }
    }

    /// Renames voice `from` to `to` (checked like a speaker's name); false when there is no `from`.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<bool> {
        let to = check_name(to).map_err(Error::Edit)?.to_string();
        let Some(i) = self.find(from) else {
            return Ok(false);
        };
        if self.find(&to).is_some_and(|j| j != i) {
            return Err(Error::Edit(format!("a voice is already called {to}")));
        }
        self.voices[i].name = to;
        Ok(true)
    }

    /// Sets voice `name`'s caption colour; false when there is no such voice.
    pub fn recolor(&mut self, name: &str, color: &str) -> Result<bool> {
        let color = parse_color(color.trim())
            .ok_or_else(|| Error::Edit(format!("{color:?} is not a colour like #F2B33D")))?;
        Ok(match self.find(name) {
            Some(i) => {
                self.voices[i].color = color;
                true
            }
            None => false,
        })
    }
}

/// The unit mean of unit embeddings.
pub fn voiceprint(embeddings: &[&[f32]]) -> Vec<f32> {
    let dim = embeddings.first().map_or(0, |e| e.len());
    let mut sum = vec![0.0f32; dim];
    for e in embeddings {
        sum.iter_mut().zip(e.iter()).for_each(|(s, x)| *s += x);
    }
    normalize(&mut sum);
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("junctur-voices-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn enroll_again_keeps_the_colour_and_new_people_get_free_colours() {
        let mut v = Voices::default();
        let e = [1.0f32, 0.0];
        let ee: [&[f32]; 1] = [&e];
        assert_eq!(v.enroll("Uezar", &ee, 5.0).unwrap().color, "#F2B33D");
        v.recolor("uezar", "#123456").unwrap();
        assert_eq!(v.enroll("Ana", &ee, 5.0).unwrap().color, "#F2B33D");
        // Enrolled again by name (any case): one voice, the samples weighted by seconds, same
        // colour and name.
        let f = [0.0f32, 1.0];
        let ff: [&[f32]; 1] = [&f];
        assert_eq!(v.enroll("UEZAR", &ff, 5.0).unwrap().color, "#123456");
        assert_eq!(v.voices.len(), 2);
        let u = v.get("uezar").unwrap();
        assert_eq!((u.name.as_str(), u.seconds), ("Uezar", 10.0));
        assert!((u.print[0] - u.print[1]).abs() < 1e-6 && (u.print[0] - 0.70710677).abs() < 1e-5);
        // Too little speech, or a name the edit file cannot hold.
        assert!(v.enroll("Bea", &ee, 1.0).is_err());
        assert!(v.enroll("color=x", &ee, 5.0).is_err());
        assert!(v.enroll("Bea", &[], 5.0).is_err());
        assert!(v.forget("ana") && !v.forget("ana"));
        assert!(v.rename("UEZAR", "Uez").unwrap());
        assert!(!v.rename("nobody", "x").unwrap());
        assert!(v.recolor("Uez", "red").is_err());
    }

    #[test]
    fn the_file_round_trips_and_a_broken_one_is_an_error() {
        let d = scratch("file");
        let path = d.join("junctur").join("voices.json");
        assert_eq!(Voices::load(&path).unwrap(), Voices::default());
        let mut v = Voices::default();
        let e = [0.6f32, 0.8];
        v.enroll("Ana", &[&e], 3.0).unwrap();
        v.save(&path).unwrap();
        assert_eq!(Voices::load(&path).unwrap(), v);
        assert_eq!(v.usable().len(), 1);
        // A print from another model is kept but not used.
        v.voices[0].model = "other.onnx".into();
        assert!(v.usable().is_empty());
        std::fs::write(&path, "{ nope").unwrap();
        assert!(Voices::load(&path).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_voiceprint_is_the_unit_mean() {
        let (a, b) = ([1.0f32, 0.0], [0.0f32, 1.0]);
        let p = voiceprint(&[&a, &b]);
        assert!((p[0] - p[1]).abs() < 1e-6 && (p[0] * p[0] + p[1] * p[1] - 1.0).abs() < 1e-5);
    }
}
