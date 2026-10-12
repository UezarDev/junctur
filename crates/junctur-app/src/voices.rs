//! The remembered voices as the app uses them (5b): the voices file (where the CLI keeps it too),
//! enrolling from a stretch of the open recording, forgetting, and a rename or recolour of a speaker
//! that is a remembered person reaching that person's voice.
//!
//! Its own module because none of this is the edit list's: the voices are the user's across every
//! recording, and relabelling reads the project's `speakers.json` and window embeddings, which the
//! editor never touches. What comes back is the project's new speakers ([`Found`]); the player puts
//! them into the list in play through the editor, like any other change to it.

use std::path::{Path, PathBuf};

use junctur_core::pipeline::{self, Found, Project};
use junctur_core::voices::{self, Voice, Voices};
use serde::Serialize;

/// A voice as the Captions panel lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceView {
    pub name: String,
    pub color: String,
    pub seconds: f64,
}

/// The voices file: `JUNCTUR_VOICES` (test mode) or `junctur/voices.json` in the config folder.
fn path() -> Result<PathBuf, String> {
    voices::default_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "there is no config folder to keep voices in".into())
}

fn load() -> Result<(PathBuf, Voices), String> {
    let path = path()?;
    let v = Voices::load(&path).map_err(|e| e.to_string())?;
    Ok((path, v))
}

fn usable(v: &Voices) -> Vec<Voice> {
    v.usable().into_iter().cloned().collect()
}

/// The project of the recording at `file`, where the CLI and the app keep it.
fn project(file: &str) -> Result<Project, String> {
    Project::open(Path::new(file), None, None).map_err(|e| e.to_string())
}

pub fn list() -> Result<Vec<VoiceView>, String> {
    let (_, v) = load()?;
    Ok(v.voices
        .into_iter()
        .map(|v| VoiceView {
            name: v.name,
            color: v.color,
            seconds: v.seconds,
        })
        .collect())
}

/// Remembers `name`'s voice from source seconds `a..b` of `file` and labels its speakers again.
/// Returns the new speakers and a line for the user.
pub fn enroll(file: &str, name: &str, (a, b): (f64, f64)) -> Result<(Found, String), String> {
    let p = project(file)?;
    let (path, mut v) = load()?;
    let before = v.get(name).map_or(0.0, |x| x.seconds);
    let found = pipeline::enroll(&p, &mut v, name, &[(a, b)]).map_err(|e| e.to_string())?;
    v.save(&path).map_err(|e| e.to_string())?;
    let voice = v.get(name).expect("just enrolled");
    let added = voice.seconds - before;
    let mut line = if before > 0.0 {
        format!(
            "Added {added:.1} s to {}'s voice ({:.1} s in all).",
            voice.name, voice.seconds
        )
    } else {
        format!("Remembered {}'s voice from {added:.1} s.", voice.name)
    };
    if voice.seconds < 20.0 {
        line.push_str(" More helps: select other stretches where only they talk and add them too.");
    }
    Ok((found, line))
}

/// Forgets `name`; with a recording open, labels its speakers again without that voice.
pub fn forget(file: Option<&str>, name: &str) -> Result<Option<Found>, String> {
    let (path, mut v) = load()?;
    if !v.forget(name) {
        return Err(format!("no voice is called {name}"));
    }
    v.save(&path).map_err(|e| e.to_string())?;
    relabel(file, &v)
}

/// The open recording's speakers labelled again with `v`; `None` with no recording, or one whose
/// speakers cannot be relabelled (tracks, or heard before 5b), which keeps its speakers.
fn relabel(file: Option<&str>, v: &Voices) -> Result<Option<Found>, String> {
    let Some(file) = file else {
        return Ok(None);
    };
    let p = project(file)?;
    match p.speakers() {
        Some(f) if f.heard.is_some() => pipeline::relabel(&p, &usable(v))
            .map(Some)
            .map_err(|e| e.to_string()),
        _ => Ok(None),
    }
}

/// The remembered person speaker `id` of `file` is, if it is one.
fn person(file: &str, id: u32) -> Option<String> {
    let found = project(file).ok()?.speakers()?;
    let p = found.people.get(id.checked_sub(1)? as usize)?.as_ref()?;
    Some(p.name.clone())
}

/// When speaker `id` is a remembered person, renames the person (their voice) and returns the
/// speakers labelled again; `None` when the speaker is nobody remembered (the caller renames it
/// in the edit list only).
pub fn rename(file: &str, id: u32, name: &str) -> Result<Option<Found>, String> {
    let Some(was) = person(file, id) else {
        return Ok(None);
    };
    let (path, mut v) = load()?;
    if !v.rename(&was, name).map_err(|e| e.to_string())? {
        return Ok(None);
    }
    v.save(&path).map_err(|e| e.to_string())?;
    relabel(Some(file), &v)
}

/// [`rename`] for the caption colour.
pub fn recolor(file: &str, id: u32, color: &str) -> Result<Option<Found>, String> {
    let Some(who) = person(file, id) else {
        return Ok(None);
    };
    let (path, mut v) = load()?;
    if !v.recolor(&who, color).map_err(|e| e.to_string())? {
        return Ok(None);
    }
    v.save(&path).map_err(|e| e.to_string())?;
    relabel(Some(file), &v)
}
