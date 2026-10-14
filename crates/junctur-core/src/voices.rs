//! Voices the user has told Junctur once: a name, a caption colour and a voiceprint per person, used
//! to label every recording by name ([`crate::diarize::label`]). A file apart from the hand-written
//! `config.json`, so a hand edit of one and an app save of the other never meet.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diarize::{normalize, EMBEDDINGS_MARK};
use crate::edit::{check_name, default_color, parse_color, PALETTE_SIZE};
use crate::{models, Error, Result};

/// Names the voices file instead of the default one (tests and the app's test mode).
pub const ENV: &str = "JUNCTUR_VOICES";

/// Under this much speech a voiceprint is rough: the app and the CLI say so. Not a floor: the
/// seconds picked are also pinned to the person in that recording, which holds however rough.
pub const SHORT_SECONDS: f64 = 3.0;
/// A sample's least weight in seconds, so even an empty one counts.
const MIN_SAMPLE_S: f64 = 0.01;
/// Degrees between generated hues: the golden angle, so no two come close however many there are.
const GOLDEN_ANGLE: f64 = 137.508;
/// The generated colours' HSL saturation and lightness: bright enough on dark video.
const SATURATION: f64 = 0.65;
const LIGHTNESS: f64 = 0.62;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    pub name: String,
    /// Caption colour, `#RRGGBB`: the person's in every recording.
    pub color: String,
    /// Seconds of speech the print was made from.
    pub seconds: f64,
    /// [`print_model`] when it was made: a print is only comparable with embeddings of the very same
    /// weights, made on the same device; [`Voices::load`] drops any other.
    pub model: String,
    pub print: Vec<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Voices {
    pub voices: Vec<Voice>,
}

/// What a voiceprint made by this build is comparable with: the embedding model's SHA-256
/// ([`models::SPEAKER_EMBEDDING`]) and how its embeddings are made ([`EMBEDDINGS_MARK`]).
pub fn print_model() -> String {
    format!("{}+{EMBEDDINGS_MARK}", models::SPEAKER_EMBEDDING.sha256)
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
    Ok(v.usable_owned())
}

/// [`remembered`], but a voices file that cannot be read is told to `say` and labelling goes on
/// without voices: speakers are an extra, and a broken file must not stop `junctur analyze` or
/// `junctur cut` (the file is left as it is for the user to fix).
pub fn remembered_or_none(say: &mut dyn FnMut(&str)) -> Vec<Voice> {
    remembered().unwrap_or_else(|e| {
        say(&format!(
            "{e}; labelling speakers without remembered voices"
        ));
        Vec::new()
    })
}

impl Voices {
    /// The voices at `path`; none when the file does not exist. A file that does not parse is an
    /// error, never silently empty (saving over it would lose every voice). A voice whose print this
    /// build cannot compare with is dropped before anything else is checked. A hand-edited name or
    /// colour the edit file could not hold is an error too, naming the voice.
    pub fn load(path: &Path) -> Result<Self> {
        let bad =
            |e: &dyn std::fmt::Display| Error::Edit(format!("voices file {}: {e}", path.display()));
        let mut v: Self = match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&b))
                .map_err(|e| bad(&e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let model = print_model();
        v.voices.retain(|x| x.model == model);
        for x in &v.voices {
            check_name(&x.name).map_err(|e| bad(&format!("{:?}: {e}", x.name)))?;
            if parse_color(&x.color).is_none() {
                return Err(bad(&format!(
                    "{}: {:?} is not a colour like #F2B33D",
                    x.name, x.color
                )));
            }
        }
        Ok(v)
    }

    /// Writes the file whole ([`crate::output::write_replacing`]).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::output::write_replacing(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// The voices whose prints this build's embedding model can compare with.
    pub fn usable(&self) -> Vec<&Voice> {
        self.voices
            .iter()
            .filter(|v| v.model == print_model() && !v.print.is_empty())
            .collect()
    }

    /// [`usable`](Self::usable), cloned.
    pub fn usable_owned(&self) -> Vec<Voice> {
        self.usable().into_iter().cloned().collect()
    }

    fn find(&self, name: &str) -> Option<usize> {
        let name = name.trim().to_lowercase();
        self.voices
            .iter()
            .position(|v| v.name.to_lowercase() == name)
    }

    pub fn get(&self, name: &str) -> Option<&Voice> {
        self.find(name).map(|i| &self.voices[i])
    }

    /// Remembers `name`'s voice from these window embeddings (`seconds` of speech). A name already
    /// known gets the sample added (its print becomes the mean of both, weighted by seconds), so a
    /// voice is built up from several selections, as a person rarely talks alone for the half minute
    /// a good print wants. A new person gets the first free palette colour; no sample is refused.
    pub fn enroll(&mut self, name: &str, embeddings: &[&[f32]], seconds: f64) -> Result<&Voice> {
        let name = check_name(name).map_err(Error::Edit)?.to_string();
        if embeddings.is_empty() {
            return Err(Error::Edit(format!(
                "there is no speech to remember {name}'s voice from there"
            )));
        }
        // A zero-length sample would weigh nothing against a voice built from more.
        let seconds = seconds.max(MIN_SAMPLE_S);
        let print = voiceprint(embeddings);
        let model = print_model();
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
                let taken = |c: &str| self.voices.iter().any(|v| v.color.eq_ignore_ascii_case(c));
                // One pass over the palette; past it (a ninth person), a colour of their own.
                let color = (1..=PALETTE_SIZE)
                    .map(default_color)
                    .find(|c| !taken(c))
                    .unwrap_or_else(|| {
                        (self.voices.len()..)
                            .map(generated_color)
                            .find(|c| !taken(c))
                            .expect("some colour")
                    });
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

/// The `n`th of an endless run of distinct bright colours (golden-angle hues), for people past
/// the palette.
fn generated_color(n: usize) -> String {
    let h = (n as f64 * GOLDEN_ANGLE) % 360.0;
    let (s, l) = (SATURATION, LIGHTNESS);
    let c = (1.0 - (2.0 * l - 1.0f64).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02X}{:02X}{:02X}", to(r), to(g), to(b))
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
mod tests;
