//! The remembered voices as the app uses them (5b): the voices file (where the CLI keeps it too),
//! enrolling from a stretch of the open recording, forgetting, and a rename or recolour of a speaker
//! that is a remembered person reaching that person's voice. Since 5c the stretch enrolled from is
//! pinned to the person in that recording, and the speakers can be heard again from the audio on
//! request ([`rehear`]), the windows cut at those pins.
//!
//! Its own module because none of this is the edit list's: the voices are the user's across every
//! recording, and relabelling reads the project's `speakers.json` and window embeddings, which the
//! editor never touches. What comes back is the project's new speakers ([`Found`]); the player puts
//! them into the list in play through the editor, like any other change to it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use junctur_core::pipeline::{self, Found, Project};
use junctur_core::runtime::DevicePref;
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

/// One load-change-save of the voices file at a time in this app: two quick changes (an
/// enrollment and a rename, say) would otherwise both load the old file and the second save would
/// drop the first change. The CLI is another process and is not held off (rare enough to leave).
static LOCK: Mutex<()> = Mutex::new(());

/// The speakers are being heard again ([`rehear`]): it rewrites the window embeddings and
/// `speakers.json` that enrolling, forgetting and renaming a person read and write, so those wait
/// (are refused with a word) until it is done. One hearing at a time.
static REHEARING: AtomicBool = AtomicBool::new(false);

fn not_while_rehearing() -> Result<(), String> {
    if REHEARING.load(Ordering::SeqCst) {
        return Err(
            "the speakers are being analysed again: try this again when that is done".into(),
        );
    }
    Ok(())
}

/// Clears [`REHEARING`] however the hearing ends; [`rehear`] hands it to its caller, which holds it
/// until the speakers are in an edit list.
pub struct Rehearing;
impl Drop for Rehearing {
    fn drop(&mut self) {
        REHEARING.store(false, Ordering::SeqCst);
    }
}

/// Hears `file`'s speakers again from the audio (5c, "Analyse speakers again"), the windows cut at
/// the seconds pinned to people, labelled with the voices as they are; saves `speakers.json` and
/// returns the speakers, with the guard that keeps enrolling and forgetting off until the caller
/// drops it. Slow (the recording is read again: about as long as the speakers stage took), so the
/// caller runs it off the async runtime.
pub fn rehear(file: &str) -> Result<(Found, Rehearing), String> {
    if REHEARING.swap(true, Ordering::SeqCst) {
        return Err("the speakers are already being analysed again".into());
    }
    let done = Rehearing;
    let p = project(file)?;
    let v = {
        let _held = crate::lock::lock(&LOCK);
        load()?.1
    };
    let found = pipeline::rehear(&p, &usable(&v), DevicePref::Auto, &mut |l| {
        eprintln!("junctur-app: speakers again: {l}")
    })
    .map_err(|e| e.to_string())?;
    Ok((found, done))
}

/// Puts `found` into the edit list on disk of `file`, a recording no longer open (as the CLI does
/// after enrolling).
pub fn respeak_on_disk(file: &str, found: &Found) -> Result<(), String> {
    pipeline::respeak(&project(file)?, found)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Labels `file`'s speakers with `v` (when it has speakers that can be), then saves the voices,
/// then the speakers: a failure before the save leaves both as they were, and labels are never
/// saved on voices that were not.
fn commit(
    path: &Path,
    v: &Voices,
    file: Option<&str>,
    renamed: Option<(&str, &str)>,
) -> Result<Option<Found>, String> {
    let labelled = match file {
        Some(f) => labelled(f, v, renamed)?,
        None => None,
    };
    v.save(path).map_err(|e| e.to_string())?;
    if let Some((p, found)) = &labelled {
        pipeline::save_speakers(p, found).map_err(|e| e.to_string())?;
    }
    Ok(labelled.map(|(_, f)| f))
}

/// The open recording's speakers labelled with `v`, not saved; `None` for one whose speakers
/// cannot be relabelled (tracks, or heard before 5b), which keeps its speakers. `renamed`: a
/// remembered person renamed (from, to), whose pinned seconds (5c) follow the new name.
fn labelled(
    file: &str,
    v: &Voices,
    renamed: Option<(&str, &str)>,
) -> Result<Option<(Project, Found)>, String> {
    let p = project(file)?;
    match p.speakers() {
        Some(f) if f.heard.is_some() => {
            let found = match renamed {
                Some((from, to)) => pipeline::labelled_renamed(&p, &usable(v), from, to),
                None => pipeline::labelled(&p, &usable(v)),
            }
            .map_err(|e| e.to_string())?;
            Ok(Some((p, found)))
        }
        _ => Ok(None),
    }
}

/// Remembers `name`'s voice from source seconds `a..b` of `file` (any length, 5c), pins those
/// seconds to them and labels its speakers again. Returns the new speakers and a line for the user.
pub fn enroll(file: &str, name: &str, (a, b): (f64, f64)) -> Result<(Found, String), String> {
    let _held = crate::lock::lock(&LOCK);
    not_while_rehearing()?;
    let p = project(file)?;
    let (path, mut v) = load()?;
    let before = v.get(name).map_or(0.0, |x| x.seconds);
    let (found, added) =
        pipeline::enroll(&p, &mut v, name, &[(a, b)]).map_err(|e| e.to_string())?;
    v.save(&path).map_err(|e| e.to_string())?;
    pipeline::save_speakers(&p, &found).map_err(|e| e.to_string())?;
    let voice = v.get(name).expect("just enrolled");
    Ok((
        found,
        enrolled_line(&voice.name, before, added, voice.seconds),
    ))
}

/// What the user is told after enrolling: how much was taken and, for a short voice, a hint, never
/// a refusal (5c: one word is enough to pin those seconds; recognising the person elsewhere wants
/// more).
fn enrolled_line(name: &str, before: f64, added: f64, total: f64) -> String {
    let mut line = if before > 0.0 {
        format!("Added {added:.1} s to {name}'s voice ({total:.1} s in all).")
    } else {
        format!("Remembered {name}'s voice from {added:.1} s.")
    };
    line.push_str(&format!(" Those seconds are {name}'s in this recording."));
    if total < voices::SHORT_SECONDS {
        line.push_str(&format!(
            " A very short clip: other recordings may not recognise {name} until you add more."
        ));
    } else if total < 20.0 {
        line.push_str(" More helps: select other stretches where only they talk and add them too.");
    }
    line
}

/// Forgets `name`; with a recording open, labels its speakers again without that voice.
/// Pins to `name` stay in the recording, unused, and come back if the name is remembered again.
pub fn forget(file: Option<&str>, name: &str) -> Result<Option<Found>, String> {
    let _held = crate::lock::lock(&LOCK);
    not_while_rehearing()?;
    let (path, mut v) = load()?;
    if !v.forget(name) {
        return Err(format!("no voice is called {name}"));
    }
    commit(&path, &v, file, None)
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
    let _held = crate::lock::lock(&LOCK);
    let Some(was) = person(file, id) else {
        return Ok(None);
    };
    not_while_rehearing()?;
    let (path, mut v) = load()?;
    if !v.rename(&was, name).map_err(|e| e.to_string())? {
        return Ok(None);
    }
    commit(&path, &v, Some(file), Some((&was, name)))
}

/// [`rename`] for the caption colour.
pub fn recolor(file: &str, id: u32, color: &str) -> Result<Option<Found>, String> {
    let _held = crate::lock::lock(&LOCK);
    let Some(who) = person(file, id) else {
        return Ok(None);
    };
    not_while_rehearing()?;
    let (path, mut v) = load()?;
    if !v.recolor(&who, color).map_err(|e| e.to_string())? {
        return Ok(None);
    }
    commit(&path, &v, Some(file), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_clip_is_remembered_with_a_hint_never_refused() {
        let short = enrolled_line("Ana", 0.0, 0.4, 0.4);
        assert!(
            short.starts_with("Remembered Ana's voice from 0.4 s."),
            "{short}"
        );
        assert!(short.contains("Those seconds are Ana's"), "{short}");
        assert!(short.contains("very short clip"), "{short}");
        let more = enrolled_line("Ana", 0.4, 6.0, 6.4);
        assert!(
            more.starts_with("Added 6.0 s to Ana's voice (6.4 s in all)."),
            "{more}"
        );
        assert!(
            more.contains("More helps") && !more.contains("very short"),
            "{more}"
        );
        assert!(!enrolled_line("Ana", 20.0, 10.0, 30.0).contains("More helps"));
    }
}
