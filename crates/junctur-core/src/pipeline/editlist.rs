//! The `edit` stage's list: drafted from the cached analysis, and written only when what it
//! replaces is not the user's or has been backed up.

use std::cell::OnceCell;
use std::path::Path;
use std::time::Instant;

use super::{
    not_analysed, speakers_for, speech, tracks_options, Found, KeepList, Project, Speech,
    StageName, StageState, EDIT_FILE,
};
use crate::edit::{self, CutSettings, EditList, Speakers};
use crate::tracks::{self, TrackReport};
use crate::transcribe::{self, Transcript};
use crate::{Error, Result};

/// The `edit` stage's write: unless `keep` is [`KeepList::No`], a list holding the user's edits is
/// kept with only its speakers replaced, through [`edit::amend`]; one that does not parse is re-cut
/// with [`KeepList::Edits`] and fails the stage with [`KeepList::Whole`]. Decided again under the
/// writer lock ([`Draft::recheck`]), since `d` was drafted outside it.
pub(super) fn keep_or_write(
    p: &Project,
    d: Draft,
    found: Option<&Found>,
    keep: KeepList,
    log: &mut dyn FnMut(&str),
) -> Result<EditList> {
    let _writing = edit::hold();
    let d = d.recheck(p);
    if keep != KeepList::No && d.users {
        let amended = edit::amend(&p.edit_path(), |e| {
            if found.is_some() {
                e.speakers = speakers_for(found, Some(&e.speakers));
            }
        })?;
        match amended {
            edit::Amended::Written(e) => {
                log("edit list: it holds your edits, kept with the speakers matched again");
                return Ok(e);
            }
            edit::Amended::Same(e) => {
                log("edit list: it holds your edits, kept as it is");
                return Ok(e);
            }
            edit::Amended::Left => match p.edit() {
                Ok(kept) => {
                    log(
                        "edit list: edited outside Junctur, kept as it is with the speakers it had",
                    );
                    return Ok(kept);
                }
                Err(e) if keep == KeepList::Whole => {
                    return Err(Error::Edit(format!(
                        "{e}. It is left as it is: fix it, or start over with a fresh cut, which keeps it as a backup"
                    )));
                }
                Err(_) => {}
            },
            edit::Amended::Missing => {}
        }
    }
    write_draft(p, d)
}

/// (Re)write the edit list from cached analysis. A hand-edited list is kept as a backup before
/// being replaced ([`back_up`]). Speaker names already in the list are kept by id, so a re-cut never
/// undoes a rename; without `speakers` (skipped, or the stage failed) the list keeps the speakers it
/// had.
pub fn cut(
    p: &Project,
    report: &TrackReport,
    speech: &Speech,
    speakers: Option<&Found>,
    transcript: Option<&Transcript>,
    settings: &CutSettings,
) -> Result<EditList> {
    write_draft(p, draft(p, report, speech, speakers, transcript, settings))
}

/// Writes a re-cut, backing up the list it replaces when that is the user's: decided again from the
/// list on disk under the edit-list writer lock ([`Draft::recheck`]).
fn write_draft(p: &Project, d: Draft) -> Result<EditList> {
    let _writing = edit::hold();
    let d = d.recheck(p);
    if let (Some(old), true) = (&d.old, d.users) {
        back_up(&p.dir, old)?;
    }
    // Whole or not at all: a list cut short by a close would open as an analysed recording whose
    // list does not parse. And only a list that reads back, as every edit-list writer.
    let text = d.list.checked_text().map_err(Error::Edit)?;
    crate::output::write_replacing(&p.edit_path(), text)?;
    Ok(d.list)
}

/// A re-cut not yet written: the new list, the text on disk it replaces, and whether that text is
/// the user's (a re-cut backs it up).
pub(super) struct Draft {
    pub(super) list: EditList,
    pub(super) old: Option<String>,
    pub(super) users: bool,
}

impl Draft {
    /// The draft against the list on disk now. Called under [`edit::hold`]: a list written since
    /// it was drafted (an app save, which takes the same lock) is the user's unless it is the very
    /// list this draft would write, so a re-cut backs it up instead of losing it, and never backs up
    /// the stale text it was drafted from instead.
    fn recheck(mut self, p: &Project) -> Draft {
        let now = std::fs::read_to_string(p.edit_path()).ok();
        if now != self.old {
            self.users = now
                .as_deref()
                .is_some_and(|t| edit::unstamped(t) != self.list.to_text());
            self.old = now;
        }
        self
    }
}

/// What [`cut`] would write, and whether it would back up what is there.
pub(super) fn draft(
    p: &Project,
    report: &TrackReport,
    speech: &Speech,
    speakers: Option<&Found>,
    transcript: Option<&Transcript>,
    settings: &CutSettings,
) -> Draft {
    let env = speech::load_envelope(&p.envelope_path()).unwrap_or_default();
    let regions = speech.all_regions();
    let starts: Vec<f64> = transcript
        .map(|t| t.words().map(|w| w.start).collect())
        .unwrap_or_default();
    let old = std::fs::read_to_string(p.edit_path()).ok();
    let parsed = old.as_deref().and_then(|t| edit::parse(t).ok());
    let previous = parsed.as_ref().map(|e| e.speakers.clone());
    // Fixed words too: a re-cut changes what is kept, not what a word says.
    let fixes = parsed.as_ref().map_or_else(Vec::new, |e| e.fixes.clone());
    // And retimed words: where a word is said is not what a re-cut decides.
    let times = parsed.as_ref().map_or_else(Vec::new, |e| e.times.clone());
    // Caption settings are the user's, like the names: a re-cut keeps them.
    // None stays None: a list that followed the user's defaults still does.
    let captions = parsed.and_then(|e| e.captions);
    let speakers = speakers_for(speakers, previous.as_ref());
    let make = |settings: &CutSettings, starts: &[f64]| EditList {
        source: p.file.clone(),
        duration: report.duration,
        settings: *settings,
        keep: edit::keep_ranges(&regions, &env, report.duration, settings),
        speakers: speakers.clone(),
        captions: captions.clone(),
        fixes: fixes.clone(),
        times: times.clone(),
        windows: edit::windows(&regions, starts, report.duration, settings.window),
    };
    let list = make(settings, &starts);
    // The finished transcript's word starts, read only when a comparison needs them (a 30 h
    // transcript is tens of MB).
    let finished: OnceCell<Vec<f64>> = OnceCell::new();
    let users = old.as_deref().is_some_and(|old| {
        // A list written before speakers existed (or with `--no-speakers`) is compared as Junctur
        // would have written it then, without them: gaining speakers is not a reason to back it up.
        let had_speakers = previous.as_ref().is_some_and(|s| !s.is_empty());
        old != list.to_text()
            && !is_generated(old, |s| {
                // As this call would write it, and as a run writes it before its transcript (no
                // word counts) and once it is done (the finished transcript's).
                let done = finished.get_or_init(|| {
                    transcribe::load(&p.dir)
                        .map(|t| t.words().map(|w| w.start).collect())
                        .unwrap_or_default()
                });
                [&starts[..], &[], &done[..]]
                    .into_iter()
                    .map(|w| {
                        let mut e = make(s, w);
                        if !had_speakers {
                            e.speakers = Speakers::default();
                        }
                        e.to_text()
                    })
                    .collect()
            })
    });
    Draft { list, old, users }
}

/// Whether the edit list on disk holds anything of the user's: a re-run of the pipeline (whose
/// `edit` stage re-cuts) would back it up and replace it. Drafted from the cached analysis with the
/// list's own settings; nothing is written. False without a list; true for one that does not parse
/// or whose analysis is gone. For the app to ask before a re-analysis.
pub fn hand_edited(p: &Project) -> Result<bool> {
    let Ok(old) = std::fs::read_to_string(p.edit_path()) else {
        return Ok(false);
    };
    let Ok(list) = edit::parse(edit::unstamped(&old)) else {
        return Ok(true);
    };
    let Some(speech) = p.speech() else {
        return Ok(true);
    };
    let report = tracks::analyze(&p.file, &tracks_options(p, false))?;
    let transcript = transcribe::load(&p.dir);
    let speakers = p.speakers();
    let d = draft(
        p,
        &report,
        &speech,
        speakers.as_ref(),
        transcript.as_ref(),
        &list.settings,
    );
    Ok(d.users)
}

/// Whether `old`, the edit list about to be replaced, is Junctur's own: `regenerate` remakes a list
/// from the current analysis, and a list it reproduces from its own recorded settings has nothing of
/// the user's in it. An untouched app save is compared without its stamp: a list the app only
/// renamed a speaker in is reproduced by the re-cut (names are carried over). Anything else (a hand
/// edit, a list that no longer parses, one made from older analysis) is the user's.
pub(super) fn is_generated(old: &str, regenerate: impl Fn(&CutSettings) -> Vec<String>) -> bool {
    let body = edit::unstamped(old);
    edit::parse(body).is_ok_and(|e| regenerate(&e.settings).iter().any(|t| t == body))
}

/// Keeps `old` at the first free `edit.junctur.bak`, `edit.junctur.bak2`, ... ([`edit::backup_path`],
/// the names the editor's saves use too).
pub(super) fn back_up(dir: &Path, old: &str) -> Result<()> {
    std::fs::write(edit::backup_path(&dir.join(EDIT_FILE)), old)?;
    Ok(())
}

/// Re-cut from what earlier stages left, without running them.
pub fn recut(p: &Project, settings: &CutSettings) -> Result<EditList> {
    let speech = p.speech().ok_or_else(not_analysed)?;
    let report = tracks::analyze(&p.file, &tracks_options(p, false))?;
    let transcript = transcribe::load(&p.dir);
    let t0 = Instant::now();
    let e = cut(
        p,
        &report,
        &speech,
        p.speakers().as_ref(),
        transcript.as_ref(),
        settings,
    )?;
    let mut jobs = p.jobs();
    jobs.file = p.file.clone();
    jobs.set(
        &p.dir,
        StageName::Edit,
        StageState::Done,
        Some(t0.elapsed().as_secs_f64()),
        None,
    )?;
    Ok(e)
}
