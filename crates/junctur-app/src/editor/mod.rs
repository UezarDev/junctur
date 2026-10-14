//! Text-based and range editing of the edit list: each edit is saved to `edit.junctur` at once and
//! can be undone in the session. Apart from the player so the editing rules are tested without mpv.

mod fixing;
mod styling;
pub mod words;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use junctur_core::edit::{
    self, frame_ranges, frames_ceil, EditList, Envelope, Fix, Range, Retime, Saved, SpreadIndex,
};

use crate::cuts::Fps;
pub use words::{Retyped, Side, TranscriptView, Words};

/// What a word edit under a frame is refused with: [`Editor::edit_all`] tells it from the others.
const UNDER_A_FRAME: &str = "that is shorter than a frame: select a little more";

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

/// An edit that was saved.
#[derive(Debug)]
pub struct Edited {
    pub list: EditList,
    pub saved: Saved,
    /// A word edit placed its boundaries by the pads alone: the project has no speech envelope.
    pub unsnapped: bool,
    /// Stretches of a several-stretch edit left out as under a frame ([`Editor::edit_all`]).
    pub skipped: usize,
    /// Fixes a word fix also made on the words heard like it ([`Editor::fix_word`]), and what
    /// those words were heard as.
    pub others: Vec<Fix>,
    pub heard: Vec<String>,
}

/// Runs of transcript words (`(first, last)`, inclusive) as the engine takes them from the page:
/// sorted and joined where they overlap or touch, so no word is cut, kept or fed into a voiceprint
/// twice, whatever the page sent. No runs, or a run that ends before it starts, is refused.
pub fn join_runs(runs: &[(usize, usize)]) -> Result<Vec<(usize, usize)>, String> {
    if runs.is_empty() {
        return Err("select the words first".into());
    }
    if let Some(&(first, last)) = runs.iter().find(|(first, last)| first > last) {
        return Err(format!("words {first} to {last} run backwards"));
    }
    let mut sorted = runs.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(sorted.len());
    for (first, last) in sorted {
        match out.last_mut() {
            Some(prev) if first <= prev.1.saturating_add(1) => prev.1 = prev.1.max(last),
            _ => out.push((first, last)),
        }
    }
    Ok(out)
}

/// What an undo step puts back: the kept ranges and, since a moved cut edge moves words with it,
/// the retimed words. Everything else in the list (names, fixes, captions) is not undone.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    keep: Vec<Range>,
    times: Vec<Retime>,
}

impl Snapshot {
    fn of(list: &EditList) -> Self {
        Self {
            keep: list.keep.clone(),
            times: list.times.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Editor {
    /// The edit list being edited; `None` when no project is open.
    path: Option<PathBuf>,
    /// The text of the app's last save: what is on disk unless someone else wrote since.
    written: Option<String>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Read on first use, by the player outside this editor's lock (see `Core::prepare`); `None`
    /// is retried, since the pipeline may write them while the app is open.
    pub words: Option<Arc<Words>>,
    pub envelope: Option<Arc<Envelope>>,
    /// While the recording transcribes: the words are read up to this many seconds, where
    /// every track is transcribed; `None` reads the finished transcript.
    pub through: Option<f64>,
    /// What a spread reads of `words`, kept beside the words it was built from. Weak, so words
    /// read again are not kept alive for it and it is never taken for a later transcript's. The
    /// pointer names the words because `Words` behind its `Arc` is never changed in place.
    spread_index: Option<(Weak<Words>, Arc<SpreadIndex>)>,
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

    /// The project folder, where the transcript and envelope are.
    pub fn project_dir(&self) -> Option<&Path> {
        self.path.as_deref()?.parent()
    }

    /// Applies `step` to `list` (the list in play), saves the result and returns it; `None` when
    /// the step changes nothing (nothing is written). History moves only once the save succeeded,
    /// so a failed write leaves both the file and the session as they were.
    pub fn edit(
        &mut self,
        list: &EditList,
        step: Step,
        fps: Fps,
    ) -> Result<Option<Edited>, String> {
        let (to, unsnapped) = match step {
            Step::Do(op) => {
                let (keep, unsnapped) = self.apply(list, op, fps)?;
                (
                    Snapshot {
                        keep,
                        times: list.times.clone(),
                    },
                    unsnapped,
                )
            }
            Step::Undo => (self.undo.last().cloned().ok_or("nothing to undo")?, false),
            Step::Redo => (self.redo.last().cloned().ok_or("nothing to redo")?, false),
        };
        self.commit(list, step, to, unsnapped, fps)
    }

    /// Makes frame edit `start..end` (`keep`) and, when it moves edge `from` of a kept range to
    /// its other end, moves the words beside that edge with it ([`Words::moved_edge`]), as **one**
    /// undo step. Any other edit (review's clips are not always the list's ranges) is the frame
    /// edit alone, as is one on a recording without a transcript.
    pub fn move_edge(
        &mut self,
        list: &EditList,
        (start, end, keep): (u64, u64, bool),
        from: u64,
        fps: Fps,
    ) -> Result<Option<Edited>, String> {
        let op = Op::Frames { start, end, keep };
        let (kept, _) = self.apply(list, op, fps)?;
        let to = match (start == from, end == from) {
            (true, false) => Some(end),
            (false, true) => Some(start),
            _ => None,
        };
        let total = frames_ceil(list.duration, fps.num, fps.den);
        let ranges = frame_ranges(&list.keep, fps.num, fps.den, total);
        // The kept range whose edge `from` is and which edge the edit moves, with the far end of
        // the cut beside it (the next range's start, the previous one's end, or the file's).
        let moved = to.and_then(|to| {
            ranges.iter().enumerate().find_map(|(k, &(a, b))| {
                if b == from && b < total && keep == (to > from) {
                    let beyond = ranges.get(k + 1).map_or(total, |r| r.0);
                    Some(((a, b), Side::End, to, beyond))
                } else if a == from && a > 0 && keep == (to < from) {
                    let beyond = k.checked_sub(1).map_or(0, |k| ranges[k].1);
                    Some(((a, b), Side::Start, to, beyond))
                } else {
                    None
                }
            })
        });
        let times = match (moved, self.words.as_deref()) {
            (Some((range, side, to, beyond)), Some(words)) => {
                words.moved_edge(&list.times, range, side, to, beyond, fps)
            }
            _ => list.times.clone(),
        };
        let to = Snapshot { keep: kept, times };
        self.commit(list, Step::Do(op), to, false, fps)
    }

    /// Applies `ops` in order to `list` and saves the result as **one** undo step; each op sees the
    /// ranges the ones before it left. `None` when together they change nothing, or there are none.
    /// A stretch under a frame among others is left out and counted ([`Edited::skipped`]), so one
    /// stray short word does not refuse the lot; any other refusal refuses the whole batch.
    pub fn edit_all(
        &mut self,
        list: &EditList,
        ops: &[Op],
        fps: Fps,
    ) -> Result<Option<Edited>, String> {
        let Some(&first) = ops.first() else {
            return Ok(None);
        };
        let mut now = list.clone();
        let mut unsnapped = false;
        let mut skipped = 0;
        for &op in ops {
            match self.apply(&now, op, fps) {
                Ok((keep, u)) => {
                    now.keep = keep;
                    unsnapped |= u;
                }
                Err(e) if e == UNDER_A_FRAME && ops.len() > 1 => skipped += 1,
                Err(e) => return Err(e),
            }
        }
        if skipped == ops.len() {
            return Err(format!(
                "each of the {skipped} selected stretches is shorter than a frame: select a little more"
            ));
        }
        if skipped > 0 && now.keep == list.keep {
            return Err(format!(
                "{skipped} of the {} selected stretches {} shorter than a frame and the rest change nothing",
                ops.len(),
                if skipped == 1 { "is" } else { "are" }
            ));
        }
        let to = Snapshot {
            keep: now.keep,
            times: list.times.clone(),
        };
        let edited = self.commit(list, Step::Do(first), to, unsnapped, fps)?;
        Ok(edited.map(|e| Edited { skipped, ..e }))
    }

    /// Saves `to` as the list's new ranges and retimed words after `step` and moves the history
    /// (see [`Self::edit`]).
    fn commit(
        &mut self,
        list: &EditList,
        step: Step,
        to: Snapshot,
        unsnapped: bool,
        fps: Fps,
    ) -> Result<Option<Edited>, String> {
        let before = Snapshot::of(list);
        if to == before {
            // Nothing to write; an undo or redo step that is already in play is still used up,
            // so the next one moves on.
            self.shift(step, before);
            return Ok(None);
        }
        let Snapshot { keep, times } = to;
        if let Step::Do(_) = step {
            // Spans under half a frame are joined back on the frame grid: say so rather than
            // write a change nobody can see or hear.
            let total = frames_ceil(list.duration, fps.num, fps.den);
            let frames = |k: &[Range]| frame_ranges(k, fps.num, fps.den, total);
            if frames(&keep) == frames(&list.keep) {
                return Err("that changes no whole frame: select at least a frame".into());
            }
        }
        let path = self.path.clone().ok_or("no edit list is open")?;
        let new = EditList {
            keep,
            times,
            ..list.clone()
        };
        let saved =
            edit::save(&path, &new).map_err(|e| format!("saving {}: {e}", path.display()))?;
        self.written = Some(saved.text.clone());
        if let Step::Do(_) = step {
            self.undo.push(before);
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
            self.redo.clear();
        } else {
            self.shift(step, before);
        }
        Ok(Some(Edited {
            list: new,
            saved,
            unsnapped,
            skipped: 0,
            others: Vec::new(),
            heard: Vec::new(),
        }))
    }

    /// Moves an undo (or redo) step to the other stack, `before` being what it replaced.
    fn shift(&mut self, step: Step, before: Snapshot) {
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

    /// The kept ranges after `op`, and whether word boundaries went unsnapped (no envelope).
    fn apply(&self, list: &EditList, op: Op, fps: Fps) -> Result<(Vec<Range>, bool), String> {
        // Less than a frame apart is the same frame boundary rounded two ways (`frame_span`).
        let join = fps.seconds(1) / 2.0;
        let (a, b, keep, unsnapped) = match op {
            Op::Frames { start, end, keep } => {
                let (a, b) = edit::frame_span(start, end, keep, fps.num, fps.den);
                (a, b, keep, false)
            }
            Op::Words { first, last, keep } => {
                let words = self
                    .words
                    .as_ref()
                    .ok_or("this recording has no transcript")?;
                let env = self.envelope.clone().unwrap_or_default();
                let (a, b) = words
                    .span(
                        first,
                        last,
                        keep,
                        &env,
                        &list.settings,
                        list.duration,
                        &list.times,
                    )
                    .ok_or("no such words in the transcript")?;
                (a, b, keep, self.envelope.is_none())
            }
        };
        let b = b.min(list.duration);
        // A word span under a frame would be joined back on the frame grid: say so instead.
        // (A frame span is a whole number of frames, written a millisecond inside each end.)
        if matches!(op, Op::Words { .. }) && b > a && b - a < fps.seconds(1) {
            return Err(UNDER_A_FRAME.into());
        }
        let kept = if b <= a {
            list.keep.clone()
        } else if keep {
            edit::keep_span(&list.keep, a, b, join)
        } else {
            edit::cut_span(&list.keep, a, b, join)
        };
        Ok((kept, unsnapped))
    }
}

#[cfg(test)]
mod tests;
