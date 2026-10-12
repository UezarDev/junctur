//! Text-based and range editing of the edit list: each edit changes the kept ranges, is saved to
//! `edit.junctur` at once (the file on disk is always a valid list), and can be undone in the
//! session. Kept apart from the player so the editing rules are tested without mpv; the player
//! hands it the list in play and takes back the new one ([`crate::player`]).

pub mod words;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use junctur_core::edit::{self, EditList, Envelope, Range};
use junctur_core::pipeline;

use crate::cuts::Fps;
pub use words::{TranscriptView, Words};

/// Undo steps kept. A 30 h list has ~10 000 ranges (160 kB a step); this caps memory near 30 MB.
const HISTORY: usize = 200;

/// One change to the kept ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Words `first..=last` of the transcript (as [`Words`] orders them), boundaries snapped into
    /// the pauses either side.
    Words {
        first: usize,
        last: usize,
        keep: bool,
    },
    /// Source frames `[start, end)`, exactly.
    Frames { start: u64, end: u64, keep: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Do(Op),
    Undo,
    Redo,
}

#[derive(Debug, Default)]
pub struct Editor {
    /// The edit list being edited; `None` when no project is open.
    path: Option<PathBuf>,
    /// The text of the app's last save: what is on disk unless someone else wrote since.
    written: Option<String>,
    undo: Vec<Vec<Range>>,
    redo: Vec<Vec<Range>>,
    /// Loaded on first use; the inner `None` is "the project has none".
    words: Option<Option<Arc<Words>>>,
    envelope: Option<Arc<Envelope>>,
}

impl Editor {
    /// A new file was opened: its edit list at `path` (if it has a project), no history.
    pub fn reset(&mut self, path: Option<PathBuf>) {
        *self = Self {
            path,
            ..Self::default()
        };
    }

    /// The edit list on disk changed and is not the app's own save: the history no longer applies
    /// (undoing would write over the other change).
    pub fn changed_elsewhere(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// `text` is what the app itself saved last.
    pub fn is_own(&self, text: &str) -> bool {
        self.written.as_deref() == Some(text)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn project_dir(&self) -> Option<&Path> {
        self.path.as_deref()?.parent()
    }

    /// The transcript's words, read on first use.
    pub fn words(&mut self) -> Option<Arc<Words>> {
        if self.words.is_none() {
            let loaded = self.project_dir().and_then(Words::load).map(Arc::new);
            self.words = Some(loaded);
        }
        self.words.clone().flatten()
    }

    fn envelope(&mut self) -> Arc<Envelope> {
        if self.envelope.is_none() {
            let env = self.project_dir().and_then(pipeline::envelope);
            self.envelope = Some(Arc::new(env.unwrap_or_default()));
        }
        self.envelope.clone().unwrap_or_default()
    }

    /// Applies `step` to `list` (the list in play), saves the result and returns it; `None` when
    /// the step changes nothing (nothing is written). History moves only once the save succeeded,
    /// so a failed write leaves both the file and the session as they were.
    pub fn edit(
        &mut self,
        list: &EditList,
        step: Step,
        fps: Fps,
    ) -> Result<Option<EditList>, String> {
        let keep = match step {
            Step::Do(op) => self.apply(list, op, fps)?,
            Step::Undo => self.undo.last().cloned().ok_or("nothing to undo")?,
            Step::Redo => self.redo.last().cloned().ok_or("nothing to redo")?,
        };
        if keep == list.keep {
            // Nothing to write; an undo or redo step that is already in play is still used up,
            // so the next one moves on.
            self.shift(step, list.keep.clone());
            return Ok(None);
        }
        let path = self.path.clone().ok_or("no edit list is open")?;
        let new = EditList {
            keep,
            ..list.clone()
        };
        let text = edit::save(&path, &new, self.written.as_deref())
            .map_err(|e| format!("saving {}: {e}", path.display()))?;
        self.written = Some(text);
        let before = list.keep.clone();
        if let Step::Do(_) = step {
            self.undo.push(before);
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
            self.redo.clear();
        } else {
            self.shift(step, before);
        }
        Ok(Some(new))
    }

    /// Moves an undo (or redo) step to the other stack, `before` being the ranges it replaced.
    fn shift(&mut self, step: Step, before: Vec<Range>) {
        match step {
            Step::Do(_) => {}
            Step::Undo => {
                self.undo.pop();
                self.redo.push(before);
            }
            Step::Redo => {
                self.redo.pop();
                self.undo.push(before);
            }
        }
    }

    fn apply(&mut self, list: &EditList, op: Op, fps: Fps) -> Result<Vec<Range>, String> {
        // Less than a frame apart is the same frame boundary rounded two ways (see `frame_span`).
        let join = fps.seconds(1) / 2.0;
        let (a, b, keep) = match op {
            Op::Frames { start, end, keep } => {
                let (a, b) = frame_span(start, end, keep, fps);
                (a, b, keep)
            }
            Op::Words { first, last, keep } => {
                let words = self.words().ok_or("this recording has no transcript")?;
                let env = self.envelope();
                let (a, b) = words
                    .span(first, last, keep, &env, &list.settings, list.duration)
                    .ok_or("no such words in the transcript")?;
                (a, b, keep)
            }
        };
        let b = b.min(list.duration);
        Ok(if b <= a {
            list.keep.clone()
        } else if keep {
            edit::keep_span(&list.keep, a, b, join)
        } else {
            edit::cut_span(&list.keep, a, b, join)
        })
    }
}

/// Seconds for source frames `[start, end)` that survive the file's millisecond times and name
/// exactly those frames again (`edit::frame_ranges` floors starts and ceils ends). `k/fps` itself
/// does not: 31/30 s writes as 1.033, which floors to frame 30. A kept span starts at the first
/// millisecond inside its first frame and ends at the last one before its end; a cut span is the
/// other way round, because the ranges either side of it take its ends.
fn frame_span(start: u64, end: u64, keep: bool, fps: Fps) -> (f64, f64) {
    let up = |f: u64| (fps.seconds(f) * 1000.0 - 1e-6).ceil() / 1000.0;
    let down = |f: u64| (fps.seconds(f) * 1000.0 + 1e-6).floor() / 1000.0;
    if keep {
        (up(start), down(end))
    } else {
        (down(start), up(end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use junctur_core::edit::{frame_ranges, frames_ceil, CutSettings};
    use junctur_core::transcribe::Word;

    const FPS: Fps = Fps { num: 30, den: 1 };

    fn r(start: f64, end: f64) -> Range {
        Range { start, end }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("junctur-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn list(keep: Vec<Range>) -> EditList {
        EditList {
            source: "rec.mp4".into(),
            duration: 60.0,
            settings: CutSettings::default(),
            keep,
            windows: vec![],
        }
    }

    fn frames(l: &EditList, fps: Fps) -> Vec<(u64, u64)> {
        let total = frames_ceil(l.duration, fps.num, fps.den);
        frame_ranges(&l.keep, fps.num, fps.den, total)
    }

    fn reread(path: &Path) -> EditList {
        edit::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn frame_edits_are_frame_exact_through_the_file() {
        let d = scratch("frames");
        let path = d.join("edit.junctur");
        for fps in [
            FPS,
            Fps {
                num: 30000,
                den: 1001,
            },
            Fps { num: 60, den: 1 },
        ] {
            let _ = std::fs::remove_file(&path);
            let mut e = Editor::default();
            e.reset(Some(path.clone()));
            let mut l = list(vec![r(0.0, 60.0)]);
            // Every cut and keep, at awkward frames, must leave exactly the frames asked for.
            for (start, end, keep) in [
                (31, 97, false),
                (40, 41, true),
                (500, 503, false),
                (95, 600, true),
            ] {
                l = e
                    .edit(&l, Step::Do(Op::Frames { start, end, keep }), fps)
                    .unwrap()
                    .unwrap_or(l);
                assert_eq!(reread(&path), l, "the file holds the list in play");
            }
            let total = frames_ceil(60.0, fps.num, fps.den);
            assert_eq!(
                frames(&l, fps),
                vec![(0, 31), (40, 41), (95, total)],
                "{fps:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn undo_redo_and_no_op_edits() {
        let d = scratch("undo");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let l0 = list(vec![r(1.0, 10.0)]);
        let cut = Step::Do(Op::Frames {
            start: 90,
            end: 120,
            keep: false,
        });
        let l1 = e.edit(&l0, cut, FPS).unwrap().unwrap();
        assert_eq!(l1.keep, vec![r(1.0, 3.0), r(4.0, 10.0)]);
        assert!(e.can_undo() && !e.can_redo());
        // Cutting what is already cut changes nothing and writes nothing.
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(e.edit(&l1, cut, FPS).unwrap(), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        let back = e.edit(&l1, Step::Undo, FPS).unwrap().unwrap();
        assert_eq!(back.keep, l0.keep);
        assert_eq!(reread(&path), back);
        assert!(!e.can_undo() && e.can_redo());
        let again = e.edit(&back, Step::Redo, FPS).unwrap().unwrap();
        assert_eq!(again, l1);
        assert!(e.edit(&again, Step::Redo, FPS).is_err(), "nothing to redo");
        // A new edit after an undo drops the redo branch.
        e.edit(&again, Step::Undo, FPS).unwrap();
        e.edit(
            &back,
            Step::Do(Op::Frames {
                start: 0,
                end: 60,
                keep: true,
            }),
            FPS,
        )
        .unwrap();
        assert!(!e.can_redo());
        e.changed_elsewhere();
        assert!(!e.can_undo());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn backs_up_a_list_it_did_not_write_once() {
        let d = scratch("bak");
        let path = d.join("edit.junctur");
        let l0 = list(vec![r(1.0, 10.0)]);
        std::fs::write(&path, l0.to_text()).unwrap();
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let op = |start| {
            Step::Do(Op::Frames {
                start,
                end: start + 3,
                keep: false,
            })
        };
        let l1 = e.edit(&l0, op(60), FPS).unwrap().unwrap();
        let l2 = e.edit(&l1, op(90), FPS).unwrap().unwrap();
        e.edit(&l2, Step::Undo, FPS).unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("edit.junctur.bak")).unwrap(),
            l0.to_text(),
            "the reduction's list is kept"
        );
        assert!(
            !d.join("edit.junctur.bak2").exists(),
            "the app's own saves are not"
        );
        assert!(e.is_own(&std::fs::read_to_string(&path).unwrap()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn word_edits_snap_and_need_a_transcript() {
        let d = scratch("words");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let l0 = list(vec![r(9.0, 14.0)]);
        let cut = Step::Do(Op::Words {
            first: 1,
            last: 1,
            keep: false,
        });
        assert!(e.edit(&l0, cut, FPS).unwrap_err().contains("no transcript"));
        let w = |start, end, text: &str| Word {
            start,
            end,
            text: text.into(),
            p: 1.0,
        };
        e.words = Some(Some(Arc::new(Words::from_words(vec![
            w(10.0, 10.4, "uno"),
            w(10.5, 11.0, "dos"),
            w(12.0, 12.5, "tres"),
        ]))));
        let l1 = e.edit(&l0, cut, FPS).unwrap().unwrap();
        // "dos" cut from the pause after "uno" to 0.25 s before "tres"; neither neighbour clipped.
        assert_eq!(l1.keep, vec![r(9.0, 10.5), r(11.75, 14.0)]);
        let l2 = e
            .edit(
                &l1,
                Step::Do(Op::Words {
                    first: 1,
                    last: 1,
                    keep: true,
                }),
                FPS,
            )
            .unwrap()
            .unwrap();
        // Kept again with its own pads: 0.35 s after it, short of the old cut's end at 11.75 s
        // (restoring the whole cut stretch is a frame edit of that stretch).
        assert_eq!(l2.keep, vec![r(9.0, 11.35), r(11.75, 14.0)]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
