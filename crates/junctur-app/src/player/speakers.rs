//! Speakers in the list in play: renamed, recoloured, a voice remembered or forgotten, heard again,
//! and their turns for the Speakers lane.

use super::{Core, OPENED_MEANWHILE};
use std::path::Path;

use junctur_core::edit::Speakers;
use junctur_core::pipeline::{self, Found};
use serde::Serialize;

use crate::lock::lock;
use crate::voices;

impl Core {
    /// Renames speaker `id` in the edit list and saves it (first taking any outside change). A
    /// speaker that is a remembered person renames the person, in every recording.
    pub fn rename_speaker(&self, file: Option<&str>, id: u32, name: &str) -> Result<(), String> {
        let name = junctur_core::edit::check_name(name)?;
        match file.map(|f| voices::rename(f, id, name)) {
            Some(Ok(Some(found))) => self.respeak(file, &found),
            Some(Err(e)) => Err(e),
            _ => self.change(file, |ed, list, _| ed.rename(list, id, name)),
        }
    }

    /// Sets speaker `id`'s caption colour in the edit list and saves it; a remembered person's
    /// colour is theirs in every recording.
    pub fn recolor_speaker(&self, file: Option<&str>, id: u32, color: &str) -> Result<(), String> {
        match file.map(|f| voices::recolor(f, id, color)) {
            Some(Ok(Some(found))) => self.respeak(file, &found),
            Some(Err(e)) => Err(e),
            _ => self.change(file, |ed, list, _| ed.recolor(list, id, color)),
        }
    }

    /// Remembers `name`'s voice from the runs of transcript words in `runs` (`first..=last` each,
    /// one enrolment) and labels the speakers again; returns a line for the user. Works from the
    /// windows the speakers stage kept, so nothing is decoded.
    pub fn enroll_voice(
        &self,
        file: Option<&str>,
        runs: &[(usize, usize)],
        name: &str,
    ) -> Result<String, String> {
        // Joined first: overlapping runs would feed the same seconds into the voiceprint twice.
        let runs = crate::editor::join_runs(runs)?;
        let file = file.ok_or("no recording is open")?;
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        // After taking the words: had another recording been opened before, they would be its.
        if self.file().as_deref() != Some(file) {
            return Err(OPENED_MEANWHILE.into());
        }
        let times = lock(&self.ctl)
            .edit
            .list
            .as_ref()
            .map_or_else(Vec::new, |l| l.times.clone());
        let stretches = runs
            .iter()
            .map(|&(first, last)| words.stretch(first, last, &times))
            .collect::<Option<Vec<_>>>()
            .ok_or("no such words in the transcript")?;
        let (found, line) = voices::enroll(file, name, &stretches)?;
        self.respeak(Some(file), &found)?;
        Ok(line)
    }

    /// Hears the open recording's speakers again from the audio, cut at the seconds pinned to
    /// people. Slow. The speakers go into the list in play only if it is still the heard
    /// recording's (else its list on disk), the re-hearing guard held so no enrollment slips in.
    pub fn reanalyze_speakers(&self, file: Option<&str>) -> Result<(), String> {
        let file = file.ok_or("no recording is open")?;
        let (found, _rehearing) = voices::rehear(file)?;
        let mut here = false;
        let changed = self.change(Some(file), |ed, list, _| {
            if !junctur_core::output::same_file(&list.source, Path::new(file)) {
                return Ok(None);
            }
            here = true;
            let speakers = pipeline::speakers_for(Some(&found), Some(&list.speakers));
            ed.respeak(list, speakers)
        });
        match changed {
            Err(e) if e == OPENED_MEANWHILE => {}
            r => r?,
        }
        if !here {
            voices::respeak_on_disk(file, &found)?;
        }
        Ok(())
    }

    /// Forgets voice `name` and labels the open recording's speakers again without it.
    pub fn forget_voice(&self, file: Option<&str>, name: &str) -> Result<(), String> {
        match voices::forget(file, name)? {
            Some(found) => self.respeak(file, &found),
            None => Ok(()),
        }
    }

    /// Puts newly labelled speakers into the list in play (names of speakers nobody enrolled are
    /// kept by id, as a re-cut does) and saves it; the kept ranges and the undo history stay.
    fn respeak(&self, file: Option<&str>, found: &Found) -> Result<(), String> {
        self.change(file, |ed, list, _| {
            let speakers = pipeline::speakers_for(Some(found), Some(&list.speakers));
            ed.respeak(list, speakers)
        })
    }

    /// The edit list's speaker turns, for the Speakers lane.
    pub fn turns(&self) -> TurnsView {
        let list = lock(&self.ctl).edit.list.clone();
        list.map_or_else(TurnsView::default, |l| TurnsView::new(&l.speakers))
    }
}

/// Who talks when, as three arrays (source seconds, speaker ids from 1), sorted by start: a 30 h
/// recording has thousands of turns, sent once per change of `speakersKey`.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct TurnsView {
    pub start: Vec<f64>,
    pub end: Vec<f64>,
    pub speaker: Vec<u32>,
}

impl TurnsView {
    pub fn new(s: &Speakers) -> Self {
        let mut v = Self::default();
        for t in &s.turns {
            v.start.push(t.start);
            v.end.push(t.end);
            v.speaker.push(t.speaker);
        }
        v
    }
}
