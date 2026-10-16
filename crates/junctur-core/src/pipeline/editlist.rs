//! The `edit` stage's list: drafted from the cached analysis, and written only when what it
//! replaces is not the user's or has been backed up.

use std::cell::OnceCell;
use std::path::Path;
use std::time::Instant;

use super::{
    not_analysed, speakers_for, speech, tracks_options, Forget, Found, KeepList, Project, Speech,
    StageName, StageState, EDIT_FILE,
};
use crate::edit::{self, CutSettings, EditList, Speakers};
use crate::tracks::{self, TrackReport};
use crate::transcribe::{self, Transcript};
use crate::{Error, Result};

/// What the `edit` stage does with the list on disk: what becomes of one holding the user's edits,
/// over which stretch, and what of the user's it drops ([`KeepList`], `PipelineOptions::within`,
/// [`Forget`]).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Rewrite {
    pub(super) keep: KeepList,
    pub(super) within: Option<(f64, f64)>,
    pub(super) forget: Forget,
}

/// [`rewrite`] with nothing but `keep` asked: a plain re-analysis's and a resumed run's write.
#[cfg(test)]
pub(super) fn keep_or_write(
    p: &Project,
    d: Draft,
    found: Option<&Found>,
    keep: KeepList,
    log: &mut dyn FnMut(&str),
) -> Result<EditList> {
    let how = Rewrite {
        keep,
        ..Rewrite::default()
    };
    rewrite(p, d, found, &how, log)
}

/// What `how` changes in a list besides its speakers: the kept ranges inside its stretch made the
/// fresh cut's (`fresh`) when the cut is not kept, and the fixes and word times dropped (inside it,
/// or everywhere) when forgotten.
fn reshape(e: &mut EditList, fresh: &[edit::Range], how: &Rewrite) {
    if let (KeepList::No, Some((a, b))) = (how.keep, how.within) {
        e.keep = edit::replace_span(&e.keep, fresh, a, b);
    }
    if how.forget.fixes {
        let inside = |start: f64| how.within.is_none_or(|(a, b)| start >= a && start < b);
        e.fixes.retain(|f| !inside(f.start));
        e.times.retain(|t| !inside(t.start));
    }
}

/// The speakers a list gets from `found` over the ones it had, `None` keeping them: named as it
/// named them, or afresh when `forget.names`.
fn respoken(found: Option<&Found>, had: &Speakers, forget: Forget) -> Option<Speakers> {
    found?;
    Some(speakers_for(found, (!forget.names).then_some(had)))
}

/// The `edit` stage's write. Unless `how.keep` is [`KeepList::No`], a list holding the user's edits
/// is kept with only its speakers replaced, through [`edit::amend`]; one that does not parse is
/// re-cut with [`KeepList::Edits`] and fails the stage with [`KeepList::Whole`]. A run over a
/// stretch always amends the list there, re-cutting only inside the stretch. What `how.forget`
/// drops goes either way, a list of the user's backed up first when it loses more than its
/// speakers. Decided again under the writer lock ([`Draft::recheck`]), since `d` was drafted
/// outside it.
pub(super) fn rewrite(
    p: &Project,
    d: Draft,
    found: Option<&Found>,
    how: &Rewrite,
    log: &mut dyn FnMut(&str),
) -> Result<EditList> {
    if how.forget.names && found.is_none() {
        // Asked for, not silently skipped: the names stay only when the user can see why.
        return Err(Error::Edit(
            "the speakers could not be heard, so they cannot be named afresh: the edit list is left as it was".into(),
        ));
    }
    let _writing = edit::hold();
    let mut d = d.recheck(p);
    let partial = how.within.is_some();
    if (how.keep != KeepList::No && d.users) || (partial && d.old.is_some()) {
        let fresh = d.list.keep.clone();
        let change = |e: &mut EditList| {
            if let Some(s) = respoken(found, &e.speakers, how.forget) {
                e.speakers = s;
            }
            reshape(e, &fresh, how);
        };
        // A list of the user's that loses more than its speakers (its cut in the stretch, its
        // fixes, its names) is kept first, as a fresh cut keeps it.
        let more = how.forget.any() || (partial && how.keep == KeepList::No);
        if d.users && more {
            let old = d.old.as_deref().unwrap_or_default();
            let changes = edit::parse(edit::unstamped(old)).is_ok_and(|mut e| {
                let mut only_speakers = e.clone();
                if let Some(s) = respoken(found, &e.speakers, Forget::default()) {
                    only_speakers.speakers = s;
                }
                change(&mut e);
                e != only_speakers
            });
            if changes {
                back_up(&p.dir, old)?;
            }
        }
        let amended = edit::amend(&p.edit_path(), change)?;
        match amended {
            edit::Amended::Written(e) => {
                log(if partial {
                    "edit list: analysed again in the stretch, the rest kept"
                } else {
                    "edit list: it holds your edits, kept with the speakers matched again"
                });
                return Ok(e);
            }
            edit::Amended::Same(e) => {
                log("edit list: it holds your edits, kept as it is");
                return Ok(e);
            }
            edit::Amended::Left => match p.edit() {
                _ if partial && how.keep == KeepList::No => {
                    return Err(Error::Edit(
                        "the edit list was edited outside Junctur, so a stretch of it cannot be cut again: analyse the whole timeline, which keeps it as a backup".into(),
                    ));
                }
                Ok(kept) => {
                    log(if how.forget.any() {
                        "edit list: edited outside Junctur, kept as it is with the speakers, names and fixed words it had"
                    } else {
                        "edit list: edited outside Junctur, kept as it is with the speakers it had"
                    });
                    return Ok(kept);
                }
                Err(e) if how.keep == KeepList::Whole => {
                    return Err(Error::Edit(format!(
                        "{e}. It is left as it is: fix it, or analyse again with Cuts unticked (Correct), which keeps it as a backup"
                    )));
                }
                Err(_) => {}
            },
            edit::Amended::Missing => {}
        }
    }
    // The draft already carries the old list's names and fixes ([`draft`]); what is forgotten goes.
    // (Its stretch, if any, is cut again from the draft itself: the same ranges.)
    if let Some(s) = respoken(found, &Speakers::default(), how.forget).filter(|_| how.forget.names)
    {
        // A list Junctur would write but for a rename is not the user's ([`is_generated`]), so
        // the re-cut would not keep it: its names are kept here instead of lost.
        if !d.users && s != d.list.speakers {
            if let Some(old) = &d.old {
                back_up(&p.dir, old)?;
            }
        }
        d.list.speakers = s;
    }
    let fresh = d.list.keep.clone();
    reshape(&mut d.list, &fresh, how);
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
    write_draft(
        p,
        draft(p, report, speech, speakers, transcript, settings, false),
    )
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

/// What [`cut`] would write, and whether it would back up what is there. `whole` keeps the whole
/// recording instead of the automatic cut (7c's "no cuts up front", [`super::PipelineOptions::whole`]);
/// either way a list Junctur made by the other rule is still its own, not the user's.
pub(super) fn draft(
    p: &Project,
    report: &TrackReport,
    speech: &Speech,
    speakers: Option<&Found>,
    transcript: Option<&Transcript>,
    settings: &CutSettings,
    whole: bool,
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
    let all = [edit::Range {
        start: 0.0,
        end: report.duration,
    }];
    let make_as = |settings: &CutSettings, starts: &[f64], whole: bool| EditList {
        source: p.file.clone(),
        duration: report.duration,
        settings: *settings,
        keep: if whole && report.duration > 0.0 {
            all.to_vec()
        } else {
            edit::keep_ranges(&regions, &env, report.duration, settings)
        },
        speakers: speakers.clone(),
        captions: captions.clone(),
        fixes: fixes.clone(),
        times: times.clone(),
        windows: edit::windows(&regions, starts, report.duration, settings.window),
    };
    let make = |settings: &CutSettings, starts: &[f64]| make_as(settings, starts, whole);
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
                    .flat_map(|w| [(w, false), (w, true)])
                    .map(|(w, whole)| {
                        let mut e = make_as(s, w, whole);
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
        false,
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
