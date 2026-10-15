//! The edit list in play: outside changes taken from disk, the user's edits applied, saved and
//! handed to playback, and the transcript and envelope the editor reads them against.

use std::path::PathBuf;
use std::sync::Arc;

use junctur_core::edit::{EditList, Speakers};
use junctur_core::pipeline;
use junctur_core::transcribe;

use super::{same_form, Core, Ctl, EditSource, OPENED_MEANWHILE};
use crate::cuts::{self, Edit, EditState, EditorStatus, Fps};
use crate::editor::{Edited, Editor, Op, Step, TranscriptView, Words};
use crate::lock::lock;

/// What the edit bar should say after a saved edit: a backup made (and why), or word boundaries
/// placed without the envelope.
fn notice(e: &Edited) -> Option<String> {
    let name = |p: &std::path::Path| {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    let said = match (&e.saved.backup, e.saved.replaced_unreadable) {
        (Some(b), true) => Some(format!(
            "edit.junctur did not parse; it was kept as {} and replaced",
            name(b)
        )),
        (None, true) => Some("edit.junctur did not parse; replaced it (a backup holds it)".into()),
        (Some(b), false) => Some(format!("the previous edit.junctur was kept as {}", name(b))),
        (None, false) if e.unsnapped => Some(
            "no speech envelope in this project: word cuts use the pads, not snapped to quiet"
                .into(),
        ),
        _ => None,
    };
    // A several-stretch edit that left short stretches out says so.
    let skipped = match e.skipped {
        0 => None,
        1 => Some("1 selected stretch is shorter than a frame and was left out".to_string()),
        n => Some(format!(
            "{n} selected stretches are shorter than a frame and were left out"
        )),
    };
    match (said, skipped) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}

/// A retimed word as the edit list writes it, all to the millisecond, so the history's comparison
/// with the file is not thrown by a last bit.
fn edit_key(t: &junctur_core::edit::Retime) -> (usize, i64, usize, i64, i64) {
    let ms = |s: f64| (s * 1000.0).round() as i64;
    (t.track, ms(t.start), t.nth, ms(t.from), ms(t.to))
}

impl Core {
    /// The edit list changed on disk: takes it and, when that changes what plays, reloads at the
    /// same source position. The file is read outside the ctl lock. The app's own save is already
    /// in play, so finding it here does nothing (no reload loop, no jump of the playhead); any
    /// other change ends the undo history, which would otherwise undo over it.
    pub fn reload_edit(&self) {
        let mut ed = lock(&self.editor);
        if let Err(e) = self.sync(&mut ed, "watch") {
            eprintln!("junctur-app: reload after an edit-list change: {e}");
        }
    }

    /// Takes the edit list on disk when someone other than the app changed it (called by the watch
    /// and before every edit, so an outside change is never overwritten unseen). New kept ranges or
    /// word times end the undo history; a list that does not parse keeps the ranges in play.
    fn sync(&self, ed: &mut Editor, by: &str) -> Result<(), String> {
        let (file, path, fps) = {
            let c = lock(&self.ctl);
            let (Some(file), Some(path)) = (c.file.clone(), c.edit_path.clone()) else {
                return Ok(());
            };
            (file, path, c.fps)
        };
        if std::fs::read_to_string(&path).is_ok_and(|t| ed.is_own(&t)) {
            return Ok(());
        }
        let (fps, read) = cuts::fetch(&file, &path, fps);
        let mut c = lock(&self.ctl);
        if c.file.as_deref() != Some(file.as_str()) {
            return Ok(()); // another file was opened meanwhile
        }
        c.fps = c.fps.or(fps);
        let new_list = read.as_ref().and_then(|e| e.list.as_deref());
        let in_play = c.edit.list.as_deref();
        if new_list.is_some() && new_list != in_play {
            eprintln!("junctur-app: edit.junctur changed on disk, taken ({by})");
        }
        // The history holds the ranges and the retimed words: an outside change to either would be
        // undone over. Compared as the file writes them, so a list read back equals the one saved.
        let undone = |l: &EditList| {
            let times: Vec<_> = l.times.iter().map(edit_key).collect();
            (l.keep.clone(), times)
        };
        if new_list.is_some() && new_list.map(undone) != in_play.map(undone) {
            ed.changed_elsewhere();
            // A first list (the app's own analysis just wrote it) had no history to clear.
            let notice =
                c.edit.list.is_some().then(|| {
                    "edit.junctur changed on disk: took it, undo history cleared".to_string()
                });
            c.status = EditorStatus {
                notice,
                ..EditorStatus::default()
            };
        }
        let merged = cuts::merge(&c.edit, read);
        self.take(&mut c, merged)
    }

    /// Plays `new` in place of the edit list in play: when its frames differ, maps positions with
    /// it and reloads the kept ranges at the same source position (snapped forward if now cut),
    /// keeping pause. Then reports the edit state.
    fn take(&self, c: &mut Ctl, new: Edit) -> Result<(), String> {
        let before = c.edit.timeline.clone();
        c.edit = new;
        let same = match (&before, &c.edit.timeline) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let mut result = Ok(());
        if !same {
            lock(&self.state).set_reference(c.edit.timeline.clone());
            self.emit_state();
            let want = c.wanted();
            if !same_form(&want, &c.requested) {
                result = self.reload(c, want);
            }
        }
        self.emit_edit(c);
        result
    }

    /// Applies an edit, undo or redo to the edit list (first taking any outside change on disk),
    /// saves it and plays the result.
    pub fn edit(&self, file: Option<&str>, step: Step) -> Result<(), String> {
        if let Step::Do(Op::Words { .. }) = step {
            self.prepare();
        }
        self.change(file, |ed, list, fps| ed.edit(list, step, fps))
    }

    /// Moves a cut's edge: frame edit `frames` (start, end, keep) moving edge `from`, and the
    /// words beside it with it, as one undo step. See [`Editor::move_edge`].
    pub fn move_edge(
        &self,
        file: Option<&str>,
        frames: (u64, u64, bool),
        from: u64,
    ) -> Result<(), String> {
        self.prepare();
        self.change(file, |ed, list, fps| ed.move_edge(list, frames, from, fps))
    }

    /// Cuts (`keep` false) or keeps every run of transcript words in `runs` (`first..=last` each)
    /// as one undo step. See [`Editor::edit_all`].
    pub fn edit_words(
        &self,
        file: Option<&str>,
        runs: &[(usize, usize)],
        keep: bool,
    ) -> Result<(), String> {
        let ops: Vec<Op> = crate::editor::join_runs(runs)?
            .into_iter()
            .map(|(first, last)| Op::Words { first, last, keep })
            .collect();
        self.prepare();
        self.change(file, |ed, list, fps| ed.edit_all(list, &ops, fps))
    }

    /// The list in play, its caption settings, its words when there is a transcript, and the
    /// frame rate: what a send to Resolve needs, which goes without captions too.
    pub(super) fn edit_source(&self) -> Result<EditSource, String> {
        self.prepare();
        let words = lock(&self.editor).words.clone();
        let (list, fps) = {
            let c = lock(&self.ctl);
            let list = c.edit.list.clone().ok_or("no edit list yet")?;
            (list, c.fps.ok_or("the frame rate is not known")?)
        };
        let captions = list
            .captions
            .clone()
            .unwrap_or_else(|| lock(&self.caption_defaults).clone());
        Ok((list, captions, words, fps))
    }

    /// Takes any outside change on disk, lets `f` change and save the list in play, and plays the
    /// result. `file`: the recording the command was let edit; when another is open since, nothing
    /// is changed ([`OPENED_MEANWHILE`]), as its runs are not held off.
    pub(super) fn change(
        &self,
        file: Option<&str>,
        f: impl FnOnce(&mut Editor, &EditList, Fps) -> Result<Option<Edited>, String>,
    ) -> Result<(), String> {
        // From reading what is on disk to saving over it, so word counts the analysis writes
        // (`edit::amend`) never land between. Taken before the editor, which the watch takes alone.
        let _writing = junctur_core::edit::hold();
        let mut ed = lock(&self.editor);
        let synced = self.sync(&mut ed, "before an edit");
        let (list, fps) = {
            let c = lock(&self.ctl);
            if c.file.as_deref() != file {
                return Err(OPENED_MEANWHILE.into());
            }
            let list = c.edit.list.clone().ok_or_else(|| {
                c.edit.error.clone().unwrap_or_else(|| {
                    "no edit list yet: run the pipeline on this recording".into()
                })
            })?;
            (list, c.fps.ok_or("the frame rate is not known")?)
        };
        synced.map_err(|e| format!("the preview did not reload: {e}"))?;
        let edited = f(&mut ed, &list, fps)?;
        let mut c = lock(&self.ctl);
        let mut status = EditorStatus {
            can_undo: ed.can_undo(),
            can_redo: ed.can_redo(),
            notice: None,
        };
        let Some(edited) = edited else {
            c.status = status;
            self.emit_edit(&c);
            return Ok(());
        };
        status.notice = notice(&edited);
        c.status = status;
        let merged = cuts::merge(&c.edit, Some(cuts::from_list(edited.list, fps)));
        self.take(&mut c, merged)
            .map_err(|e| format!("saved, but the preview did not reload: {e}"))
    }

    /// Reads the transcript and the speech envelope, if not yet in the editor, outside its lock
    /// (a 30 h transcript or envelope is tens of MB) so the watch and other edits are not held up.
    /// While the recording transcribes the words are the ones so far, up to where every track is
    /// done ([`Words::so_far`]).
    pub(super) fn prepare(&self) {
        let (dir, words, env, through) = {
            let ed = lock(&self.editor);
            let Some(dir) = ed.project_dir().map(PathBuf::from) else {
                return;
            };
            (dir, ed.words.is_none(), ed.envelope.is_none(), ed.through)
        };
        let w = words
            .then(|| match through {
                Some(t) => {
                    let mut r = lock(&self.so_far);
                    let segs = r.read(&dir).unwrap_or_default();
                    Some(Words::so_far(&segs, t))
                }
                None => {
                    // Whole now: the partial's chunks read so far are not needed again.
                    *lock(&self.so_far) = transcribe::SoFar::default();
                    Words::load(&dir)
                }
            })
            .flatten()
            .map(Arc::new);
        let e = env
            .then(|| pipeline::envelope(&dir).map(Arc::new))
            .flatten();
        let mut ed = lock(&self.editor);
        if ed.project_dir() == Some(dir.as_path()) {
            // A piece that landed meanwhile asks for the words again, up to further on.
            if ed.through == through {
                ed.words = ed.words.take().or(w);
            }
            ed.envelope = ed.envelope.take().or(e);
        }
    }

    /// The transcript's words, read on first use (and again while there are none, as the pipeline
    /// may write them), labelled with the speakers of the list in play; `None` without a project
    /// or transcript.
    pub fn transcript(&self) -> Option<TranscriptView> {
        self.prepare();
        let words = lock(&self.editor).words.clone()?;
        let list = lock(&self.ctl).edit.list.clone();
        let none = Speakers::default();
        let speakers = list.as_deref().map_or(&none, |l| &l.speakers);
        let fixes = list.as_deref().map_or(&[][..], |l| &l.fixes);
        let times = list.as_deref().map_or(&[][..], |l| &l.times);
        Some(TranscriptView::new(&words, speakers, fixes, times))
    }

    pub fn edit_state(&self) -> EditState {
        let c = lock(&self.ctl);
        let defaults = lock(&self.caption_defaults).clone();
        EditState::new(&c.edit, c.skipping.on(), c.fps, &c.status, &defaults)
    }
}
