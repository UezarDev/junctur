//! The project's speakers file outside a run: enrolling a voice, hearing or labelling the speakers
//! again, and putting them into the edit list.

use super::{
    not_analysed, speakers, speakers_stage, steps, Found, Method, PipelineOptions, Project,
};
use std::path::{Path, PathBuf};

use crate::edit::{self, EditList, Speakers};
use crate::tracks::{self};
use crate::voices::{Voice, Voices};
use crate::Result;

/// Remembers `name`'s voice from `stretches` of this recording (source seconds; one word will do)
/// into `voices`, pins them to `name` ([`Found::pin`]) and labels the speakers with it. Nothing is
/// written ([`enroll_and_save`] writes both). Returns the speakers and the seconds of speech this
/// added to the voice.
pub fn enroll(
    p: &Project,
    voices: &mut Voices,
    name: &str,
    stretches: &[(f64, f64)],
) -> Result<(Found, f64)> {
    let mut found = p.speakers().ok_or_else(not_analysed)?;
    let (mut sample, mut seconds) = speakers::voice_sample(&found, &p.dir, stretches)?;
    if sample.is_empty() {
        let track = found.heard.as_ref().map_or(0, |h| h.track);
        (sample, seconds) = speakers::embed_stretches(&p.file, track, stretches, &p.cache_dir)?;
    }
    let refs: Vec<&[f32]> = sample.iter().map(Vec::as_slice).collect();
    let mut changed = voices.clone();
    changed.enroll(name, &refs, seconds)?;
    let named = changed.get(name).map_or(name, |v| v.name.as_str());
    found.pin(named, stretches);
    let again = speakers::relabel(&found, &p.dir, &changed.usable_owned())?;
    *voices = changed;
    Ok((again, seconds))
}

/// [`enroll`], then saves the voices to `voices_path`, then the speakers: new labels never stand on
/// voices that were not saved.
pub fn enroll_and_save(
    p: &Project,
    voices: &mut Voices,
    voices_path: &Path,
    name: &str,
    stretches: &[(f64, f64)],
) -> Result<(Found, f64)> {
    let (found, added) = enroll(p, voices, name, stretches)?;
    voices.save(voices_path)?;
    save_speakers(p, &found)?;
    Ok((found, added))
}

/// Hears the project's speakers again from the audio ("Analyse speakers again"), the windows
/// cut at the pins the user made ([`speakers::seeded`]), and labels them with `voices`; the other
/// stages' results are reused, never rerun (they need nothing new: `--force` would redo Whisper
/// for minutes). Saves `speakers.json`; the edit list is the caller's ([`respeak`]).
pub fn rehear(p: &Project, voices: &[Voice], log: &mut dyn FnMut(&str)) -> Result<Found> {
    let speech = p.speech().ok_or_else(not_analysed)?;
    let report = tracks::analyze(
        &p.file,
        &tracks::AnalyzeOptions {
            cache_dir: Some(p.cache_dir.clone()),
            ..Default::default()
        },
    )?;
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();
    let opts = PipelineOptions {
        force: true,
        voices: voices.to_vec(),
        ..Default::default()
    };
    speakers_stage(p, &mut jobs, &report, &speech, &opts, log, steps::none())
}

/// The project's speakers labelled again with `voices` from what the stage heard (no audio),
/// without saving them.
pub fn labelled(p: &Project, voices: &[Voice]) -> Result<Found> {
    let found = p.speakers().ok_or_else(not_analysed)?;
    speakers::relabel(&found, &p.dir, voices)
}

/// [`labelled`] after a remembered person was renamed `from` -> `to` in `voices`: the seconds
/// pinned to them follow the new name. Speakers heard by an older Junctur cannot be
/// labelled again, but their pins still follow the name, so that the next hearing gives those
/// seconds to the renamed person instead of dropping them; the rest is kept as it is.
pub fn labelled_renamed(p: &Project, voices: &[Voice], from: &str, to: &str) -> Result<Found> {
    let mut found = p.speakers().ok_or_else(not_analysed)?;
    found.rename_pins(from, to);
    if found.heard_by_an_older_junctur() {
        return Ok(found);
    }
    speakers::relabel(&found, &p.dir, voices)
}

/// Writes `speakers.json` whole ([`crate::output::write_replacing`]): it holds the user's pins, and
/// a write torn by a crash would lose them.
pub(super) fn write_speakers(p: &Project, found: &Found) -> Result<()> {
    crate::output::write_replacing(&p.speakers_path(), serde_json::to_vec_pretty(found)?)?;
    Ok(())
}

/// Renames `path` to the first free `<name>.unreadable`, `<name>.unreadable2`, ... beside it;
/// returns the new path.
pub(super) fn set_aside(path: &Path) -> Result<PathBuf> {
    let free = crate::output::first_free(path, "unreadable");
    std::fs::rename(path, &free)?;
    Ok(free)
}

/// Saves `found` as the project's speakers (`speakers.json`), when it differs from what is there.
pub fn save_speakers(p: &Project, found: &Found) -> Result<()> {
    if p.speakers().as_ref() != Some(found) {
        write_speakers(p, found)?;
    }
    Ok(())
}

/// Puts `found`'s speakers into the project's edit list in place of the ones it names, keeping
/// everything else (kept ranges, captions, the names of speakers who are not enrolled voices), and
/// saves it as the app's editor does ([`edit::save`]: atomic, a hand edit backed up first) when
/// that changes it. For the CLI after enrolling. `None` when there is no readable edit list;
/// else the list and where a replaced hand edit was kept, if one was.
pub fn respeak(p: &Project, found: &Found) -> Result<Option<(EditList, Option<PathBuf>)>> {
    // From the read to the save: word counts the app's own analysis writes meanwhile
    // ([`edit::amend`]) would otherwise be saved over.
    let _writing = edit::hold();
    let Ok(mut e) = p.edit() else {
        return Ok(None);
    };
    let speakers = speakers_for(Some(found), Some(&e.speakers));
    let mut backup = None;
    if speakers != e.speakers {
        e.speakers = speakers;
        backup = edit::save(&p.edit_path(), &e)?.backup;
    }
    Ok(Some((e, backup)))
}

/// One line on what the speakers stage found, for the log.
pub fn describe(found: &Found) -> String {
    match found.method {
        Method::Tracks => format!("{} speakers, one per speech track", found.tracks.len()),
        Method::Voices => {
            let known: Vec<&str> = found
                .people
                .iter()
                .flatten()
                .map(|p| p.name.as_str())
                .collect();
            let overlaps = found.heard.as_ref().map_or(0, |h| h.overlaps.len());
            format!(
                "{} speaker(s) told apart by voice{}, {} turns, {overlaps} stretches of two talking at once",
                found.tracks.len(),
                if known.is_empty() {
                    String::new()
                } else {
                    format!(" (recognised: {})", known.join(", "))
                },
                found.turns.len()
            )
        }
        Method::None => "no speakers: no track carries speech".into(),
    }
}

/// Labels the project's speakers again with `voices` (after enrolling or forgetting one), from
/// what the stage heard, and saves the result; no audio is read. The edit list is not touched: the
/// caller puts the new speakers into the list it holds ([`speakers_for`]).
pub fn relabel(p: &Project, voices: &[Voice]) -> Result<Found> {
    let again = labelled(p, voices)?;
    save_speakers(p, &again)?;
    Ok(again)
}

/// The speakers a re-cut writes: found ones named as `previous` named them, or with nothing
/// found (skipped, or the stage failed) the ones `previous` had, so neither erases a rename.
pub fn speakers_for(found: Option<&Found>, previous: Option<&Speakers>) -> Speakers {
    match found {
        Some(f) => Speakers::found(f.turns.clone(), &f.tracks, &f.people, previous, &f.known),
        None => previous.cloned().unwrap_or_default(),
    }
}
