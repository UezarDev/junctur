//! Text-based and range editing of the edit list: each edit changes the kept ranges, is saved to
//! `edit.junctur` at once (the file on disk is always a valid list), and can be undone in the
//! session. Kept apart from the player so the editing rules are tested without mpv; the player
//! hands it the list in play and takes back the new one ([`crate::player`]).

pub mod words;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use junctur_core::edit::{
    self, check_name, frame_ranges, frames_ceil, Captions, EditList, Envelope, Fix, Range, Retime,
    Saved, Spread, Which, WordAt,
};

use crate::cuts::Fps;
use junctur_core::transcribe::Word;
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
    /// Fixes a word fix also made on the words heard like it (5aa, [`Editor::fix_word`]), and what
    /// those words were heard as.
    pub others: Vec<Fix>,
    pub heard: Vec<String>,
}

/// Runs of transcript words (`(first, last)`, inclusive) as the engine takes them from the page:
/// sorted and joined where they overlap or touch, so no word is cut, kept or (above all) fed into
/// a voiceprint twice (2p repair: the page joins them too, but the engine does not rely on it).
/// No runs, or a run that ends before it starts, is refused.
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

/// What an undo step puts back: the kept ranges and, since a moved cut edge moves words with it
/// (2w), the retimed words. Everything else in the list (names, fixes, captions) is not undone.
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
    /// While the recording transcribes (2j): the words are read up to this many seconds, where
    /// every track is transcribed; `None` reads the finished transcript.
    pub through: Option<f64>,
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
    /// its other end (2w: a cut's edge dragged on the lanes or stepped in review or the range
    /// toolbar), moves the words beside that edge with it ([`Words::moved_edge`]), saved as **one**
    /// undo step. An edit that is not such a move (review's clips are not always the list's ranges,
    /// and a clip edge between two kept clips is no edge of the list) is the frame edit alone, as
    /// is one on a recording without a transcript. The words are the editor's, read before
    /// ([`crate::player`] prepares them).
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
        // The kept range whose edge `from` is, and which edge: one the edit moves (keeping frames
        // past a range's end or before its start, or cutting frames inside it from that edge).
        // With the far end of the cut beside the edge: the next range's start, the previous one's
        // end, or the file's.
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

    /// Applies every op of `ops` in order to `list` and saves the result as **one** undo step (2p:
    /// a selection of several stretches is cut or kept at once, and one undo puts all of it back).
    /// Each op sees the ranges the ones before it left, so overlapping ops land as they would one
    /// after another. `None` when together they change nothing; no ops is that.
    ///
    /// A stretch under a frame among others is left out and counted ([`Edited::skipped`], said in
    /// the notice; 2p repair): one stray short word in a selection of someone's lines should not
    /// refuse the lot. When every stretch is under a frame, or the ones left change nothing, it is
    /// refused saying so. Any other refusal (a word out of range, no transcript) refuses the whole
    /// batch and nothing is written.
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

    /// Renames speaker `id` and saves; `None` when the name is already that. Like every styling
    /// change (below) not an undo step: the history holds kept ranges (and retimed words, 2w) only, so undoing a cut keeps
    /// the new name (the list saved for an undo is the one in play with older ranges).
    pub fn rename(
        &mut self,
        list: &EditList,
        id: u32,
        name: &str,
    ) -> Result<Option<Edited>, String> {
        let name = check_name(name)?;
        self.restyle(list, |l| {
            l.speakers
                .rename(id, name)
                .then_some(())
                .ok_or(format!("there is no speaker {id}"))
        })
    }

    /// Sets speaker `id`'s caption colour (`#RRGGBB`) and saves.
    pub fn recolor(
        &mut self,
        list: &EditList,
        id: u32,
        color: &str,
    ) -> Result<Option<Edited>, String> {
        let color = edit::parse_color(color.trim())
            .ok_or_else(|| format!("{color:?} is not a colour like #F2B33D"))?;
        self.restyle(list, |l| {
            l.speakers
                .recolor(id, &color)
                .then_some(())
                .ok_or(format!("there is no speaker {id}"))
        })
    }

    /// Sets the caption settings (switches, style, how cues are cut) and saves.
    pub fn set_captions(
        &mut self,
        list: &EditList,
        captions: Captions,
    ) -> Result<Option<Edited>, String> {
        captions.check()?;
        self.restyle(list, |l| {
            l.captions = Some(captions);
            Ok(())
        })
    }

    /// Fixes the spelling of `word`, heard on `track` (5h), the `nth` of that track's words starting
    /// in its millisecond (5w, [`super::Words::nth`]), and saves; empty text or the heard text again
    /// takes the fix away. Every other word heard like it gets the same fix in the same save (5aa,
    /// [`edit::spread`]), [`Edited::others`] naming those. Like a rename, not an undo step: the
    /// history holds kept ranges.
    pub fn fix_word(
        &mut self,
        list: &EditList,
        track: usize,
        word: &Word,
        nth: usize,
        text: &str,
    ) -> Result<Option<Edited>, String> {
        let at = WordAt {
            start: word.start,
            nth,
        };
        let mut fixes = list.fixes.clone();
        edit::set_fix(&mut fixes, track, word.start, nth, &word.text, text)?;
        let s = self.spread(
            &mut fixes,
            Which::Words {
                track,
                first: at,
                last: at,
            },
        );
        self.set_fixes(list, fixes, s)
    }

    /// Marks `word`, heard on `track`, the `nth` of that track's words starting in its millisecond
    /// (5w), as checked by the user (5o: a fix line with the heard text, so the transcript panel no
    /// longer flags it) and saves; a word fixed already is left as it is. Like a fix, not an undo
    /// step.
    pub fn confirm_word(
        &mut self,
        list: &EditList,
        track: usize,
        word: &Word,
        nth: usize,
    ) -> Result<Option<Edited>, String> {
        self.restyle(list, |l| {
            edit::confirm_fix(&mut l.fixes, track, word.start, nth, &word.text).map(|_| ())
        })
    }

    /// Sets the text of the run `r` names (5x, [`super::Words::retyped`]) as one fix, and saves;
    /// the fixes of words inside it, and a span overlapping it, go. Empty text or the heard text again
    /// takes them all away. The same words heard elsewhere get the same text (5aa), as for
    /// [`Self::fix_word`]. Not an undo step.
    pub fn fix_words(
        &mut self,
        list: &EditList,
        r: &Retyped,
        text: &str,
    ) -> Result<Option<Edited>, String> {
        let mut fixes = list.fixes.clone();
        edit::set_span(&mut fixes, r.track, r.first, r.last, &r.heard, text)?;
        let s = self.spread(
            &mut fixes,
            Which::Words {
                track: r.track,
                first: r.first,
                last: r.last,
            },
        );
        self.set_fixes(list, fixes, s)
    }

    /// Gives every word heard like a fixed one the same fix (5aa: the Correct button's first step),
    /// and saves; what was added and the heard texts two fixes disagree on. Not an undo step.
    pub fn spread_all(&mut self, list: &EditList) -> Result<(Option<Edited>, Spread), String> {
        let mut fixes = list.fixes.clone();
        let s = self.spread(&mut fixes, Which::All);
        let edited = self.set_fixes(list, fixes, s.clone())?;
        Ok((edited, s))
    }

    /// Takes back the fixes a fix also made elsewhere (5aa: the note's Undo), those still as they
    /// were: their words keep what they were heard as (`heard`), as fixes of the heard text, so no
    /// later spread gives them the fix again ([`edit::keep_heard`]). Saved; not an undo step.
    pub fn keep_heard(
        &mut self,
        list: &EditList,
        others: &[Fix],
        heard: &[String],
    ) -> Result<(Option<Edited>, usize), String> {
        let mut fixes = list.fixes.clone();
        let n = edit::keep_heard(&mut fixes, others, heard);
        Ok((self.set_fixes(list, fixes, Spread::default())?, n))
    }

    /// Takes the fixes of `gone` back to what their words of `words` were heard as (5ae: the list of
    /// fixes' Remove), those still as listed ([`edit::remove`]). Saved; not an undo step. How many.
    pub fn remove_fixes(
        &mut self,
        list: &EditList,
        gone: &[Fix],
        words: &Words,
    ) -> Result<(Option<Edited>, usize), String> {
        let mut fixes = list.fixes.clone();
        let n = edit::remove(&mut fixes, gone, &words.in_order());
        Ok((self.set_fixes(list, fixes, Spread::default())?, n))
    }

    /// [`edit::spread`] over the transcript the editor has (none yet: nothing to spread over).
    fn spread(&self, fixes: &mut Vec<Fix>, which: Which) -> Spread {
        match &self.words {
            Some(w) => edit::spread(fixes, &w.in_order(), which),
            None => Spread::default(),
        }
    }

    /// Saves `fixes` as the list's, `s` saying what of them a spread added.
    fn set_fixes(
        &mut self,
        list: &EditList,
        fixes: Vec<Fix>,
        s: Spread,
    ) -> Result<Option<Edited>, String> {
        // Never a list the parser would refuse (5aa repair): a spread checks this itself, so this
        // is the last line, not the rule.
        if edit::clash(&fixes) {
            return Err("that would give a word two fixes: nothing was changed".into());
        }
        let edited = self.restyle(list, |l| {
            l.fixes = fixes;
            Ok(())
        })?;
        Ok(edited.map(|e| Edited {
            others: s.added,
            heard: s.heard,
            ..e
        }))
    }

    /// Puts `speakers` in place of the list's (5b: labelled again after a voice was enrolled or
    /// forgotten) and saves. Like a rename, not an undo step.
    pub fn respeak(
        &mut self,
        list: &EditList,
        speakers: junctur_core::edit::Speakers,
    ) -> Result<Option<Edited>, String> {
        self.restyle(list, |l| {
            l.speakers = speakers;
            Ok(())
        })
    }

    /// Applies `f` to a copy of `list` and saves it; `None` when it changed nothing.
    fn restyle(
        &mut self,
        list: &EditList,
        f: impl FnOnce(&mut EditList) -> Result<(), String>,
    ) -> Result<Option<Edited>, String> {
        let mut new = list.clone();
        f(&mut new)?;
        if new == *list {
            return Ok(None);
        }
        let path = self.path.clone().ok_or("no edit list is open")?;
        let saved =
            edit::save(&path, &new).map_err(|e| format!("saving {}: {e}", path.display()))?;
        self.written = Some(saved.text.clone());
        Ok(Some(Edited {
            list: new,
            saved,
            unsnapped: false,
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
mod tests {
    use super::*;
    use junctur_core::edit::CutSettings;
    use junctur_core::transcribe::Word;

    /// The list after a step that must change something.
    fn step(e: &mut Editor, l: &EditList, s: Step, fps: Fps) -> EditList {
        e.edit(l, s, fps).unwrap().expect("a change").list
    }

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
            speakers: Default::default(),
            captions: Default::default(),
            fixes: Vec::new(),
            times: Vec::new(),
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
                l = step(&mut e, &l, Step::Do(Op::Frames { start, end, keep }), fps);
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
        let l1 = step(&mut e, &l0, cut, FPS);
        assert_eq!(l1.keep, vec![r(1.0, 3.0), r(4.0, 10.0)]);
        assert!(e.can_undo() && !e.can_redo());
        // Cutting what is already cut changes nothing and writes nothing.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(e.edit(&l1, cut, FPS).unwrap().is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        let back = step(&mut e, &l1, Step::Undo, FPS);
        assert_eq!(back.keep, l0.keep);
        assert_eq!(reread(&path), back);
        assert!(!e.can_undo() && e.can_redo());
        let again = step(&mut e, &back, Step::Redo, FPS);
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
    fn several_stretches_are_one_undo_step_or_none() {
        let d = scratch("many");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let l0 = list(vec![r(0.0, 20.0)]);
        let cut = |start, end| Op::Frames {
            start,
            end,
            keep: false,
        };
        // No ops, or ops that change nothing, write nothing and leave no undo step.
        assert!(e.edit_all(&l0, &[], FPS).unwrap().is_none());
        assert!(!path.exists());
        // Two stretches apart and a third overlapping the first: one save, one undo step.
        let ops = [cut(30, 60), cut(300, 330), cut(45, 90)];
        let l1 = e.edit_all(&l0, &ops, FPS).unwrap().expect("a change").list;
        assert_eq!(l1.keep, vec![r(0.0, 1.0), r(3.0, 10.0), r(11.0, 20.0)]);
        assert_eq!(reread(&path), l1);
        assert!(e.edit_all(&l1, &ops, FPS).unwrap().is_none(), "already cut");
        let back = step(&mut e, &l1, Step::Undo, FPS);
        assert_eq!(back.keep, l0.keep, "one undo puts every stretch back");
        assert!(!e.can_undo() && e.can_redo());
        assert_eq!(step(&mut e, &back, Step::Redo, FPS), l1);

        // A stretch under a frame among others is left out and counted; the rest is one step.
        e.words = Some(Arc::new(Words::from_words(vec![
            Word {
                start: 12.0,
                end: 12.5,
                text: "uno".into(),
                p: 1.0,
                p_min: None,
            },
            Word {
                start: 12.51,
                end: 12.52,
                text: "y".into(),
                p: 1.0,
                p_min: None,
            },
            Word {
                start: 12.53,
                end: 13.0,
                text: "dos".into(),
                p: 1.0,
                p_min: None,
            },
        ])));
        let words = |first, last| Op::Words {
            first,
            last,
            keep: false,
        };
        let edited = e
            .edit_all(&l1, &[cut(450, 480), words(1, 1)], FPS)
            .unwrap()
            .expect("the frame cut is made");
        assert_eq!(edited.skipped, 1);
        let l2 = edited.list;
        assert_eq!(
            l2.keep,
            vec![r(0.0, 1.0), r(3.0, 10.0), r(11.0, 15.0), r(16.0, 20.0)]
        );
        // Only short stretches, or short ones and the rest already done: refused, nothing written.
        let text = std::fs::read_to_string(&path).unwrap();
        let all = e.edit_all(&l2, &[words(1, 1), words(1, 1)], FPS);
        assert!(all.unwrap_err().contains("each of the 2"));
        let rest = e.edit_all(&l2, &[cut(450, 480), words(1, 1)], FPS);
        assert!(rest.unwrap_err().contains("1 of the 2"));
        // A single short stretch keeps the plain refusal.
        assert_eq!(
            e.edit_all(&l2, &[words(1, 1)], FPS).unwrap_err(),
            UNDER_A_FRAME
        );
        // Any other refusal refuses the batch.
        assert!(e
            .edit_all(&l2, &[cut(510, 540), words(7, 9)], FPS)
            .unwrap_err()
            .contains("no such words"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn word_runs_are_sorted_joined_and_checked() {
        assert_eq!(
            join_runs(&[(20, 25), (3, 5), (4, 9), (10, 10), (30, 30)]).unwrap(),
            vec![(3, 10), (20, 25), (30, 30)],
            "overlapping and touching runs join, in order"
        );
        assert_eq!(join_runs(&[(7, 7), (7, 7)]).unwrap(), vec![(7, 7)]);
        assert_eq!(join_runs(&[(5, 9), (6, 7)]).unwrap(), vec![(5, 9)]);
        assert!(join_runs(&[]).is_err());
        assert!(join_runs(&[(1, 2), (9, 4)])
            .unwrap_err()
            .contains("backwards"));
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
        let l1 = step(&mut e, &l0, op(60), FPS);
        let l2 = step(&mut e, &l1, op(90), FPS);
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
            p_min: None,
        };
        e.words = Some(Arc::new(Words::from_words(vec![
            w(10.0, 10.4, "uno"),
            w(10.5, 11.0, "dos"),
            w(12.0, 12.5, "tres"),
            w(12.51, 12.52, "y"),
            w(12.53, 13.0, "z"),
        ])));
        let edited = e.edit(&l0, cut, FPS).unwrap().unwrap();
        assert!(edited.unsnapped, "no envelope: pads only, and said so");
        let l1 = edited.list;
        // "dos" cut from the pause after "uno" to 0.25 s before "tres"; neither neighbour clipped.
        assert_eq!(l1.keep, vec![r(9.0, 10.5), r(11.75, 14.0)]);
        e.envelope = Some(Arc::new(Envelope::default()));
        let keep = Step::Do(Op::Words {
            first: 1,
            last: 1,
            keep: true,
        });
        let edited = e.edit(&l1, keep, FPS).unwrap().unwrap();
        assert!(!edited.unsnapped);
        let l2 = edited.list;
        // Kept again with its own pads: 0.35 s after it, short of the old cut's end at 11.75 s
        // (restoring the whole cut stretch is a frame edit of that stretch).
        assert_eq!(l2.keep, vec![r(9.0, 11.35), r(11.75, 14.0)]);
        // "y" lasts 10 ms between two words with no room to pad: under a frame, refused rather
        // than joined back silently.
        let tiny = Step::Do(Op::Words {
            first: 3,
            last: 3,
            keep: false,
        });
        assert!(e.edit(&l2, tiny, FPS).unwrap_err().contains("frame"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn renames_are_saved_and_survive_undo() {
        use junctur_core::edit::{Speakers, Turn};
        let d = scratch("rename");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let mut l0 = list(vec![r(0.0, 30.0)]);
        l0.speakers = Speakers::new(
            vec![Turn {
                start: 1.0,
                end: 5.0,
                speaker: 1,
            }],
            &[None],
            None,
        );
        let l1 = step(
            &mut e,
            &l0,
            Step::Do(Op::Frames {
                start: 30,
                end: 60,
                keep: false,
            }),
            FPS,
        );
        let renamed = e.rename(&l1, 1, "  Ana María ").unwrap().unwrap().list;
        assert_eq!(renamed.speakers.name(1), Some("Ana María"));
        assert_eq!(reread(&path), renamed);
        assert!(e.is_own(&std::fs::read_to_string(&path).unwrap()));
        // The same name again writes nothing; bad names and ids are refused.
        assert!(e.rename(&renamed, 1, "Ana María").unwrap().is_none());
        assert!(e.rename(&renamed, 1, "  ").is_err());
        assert!(e.rename(&renamed, 1, "a\nb").is_err());
        assert!(e
            .rename(&renamed, 1, &"x".repeat(edit::MAX_NAME + 1))
            .is_err());
        assert!(e.rename(&renamed, 2, "Bea").is_err());
        assert!(e.rename(&renamed, 1, "track=2 Bob").is_err());
        assert!(e.rename(&renamed, 1, "color=#FF0000 Bob").is_err());
        let bob = e.rename(&renamed, 1, "Bob track=2").unwrap().unwrap().list;
        // Colours and caption settings save the same way, also not undo steps.
        let renamed = e.rename(&bob, 1, "Ana María").unwrap().unwrap().list;
        let red = e.recolor(&renamed, 1, " #ff0000").unwrap().unwrap().list;
        assert_eq!(red.speakers.color(1), Some("#FF0000"));
        assert!(e.recolor(&red, 1, "#FF0000").unwrap().is_none());
        assert!(e.recolor(&red, 1, "red").is_err());
        assert!(e.recolor(&red, 2, "#00FF00").is_err());
        let on = Captions {
            on: true,
            ..Captions::default()
        };
        let captioned = e.set_captions(&red, on.clone()).unwrap().unwrap().list;
        assert_eq!(reread(&path).captions, Some(on.clone()));
        assert!(e
            .set_captions(&captioned, Captions { lines: 9, ..on })
            .is_err());
        let renamed = captioned;
        // Undoing the cut keeps the name.
        let undone = step(&mut e, &renamed, Step::Undo, FPS);
        assert_eq!(undone.keep, l0.keep);
        assert_eq!(undone.speakers.name(1), Some("Ana María"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_fixed_word_is_saved_at_once_and_outlives_an_undo() {
        let d = scratch("fix");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let heard = Word {
            start: 12.34,
            end: 12.8,
            text: " Wezar,".into(),
            p: 0.6,
            p_min: None,
        };
        let l0 = list(vec![r(1.0, 30.0)]);
        let l1 = e
            .fix_word(&l0, 1, &heard, 0, "Uezar,")
            .unwrap()
            .expect("a change")
            .list;
        assert_eq!(l1.fixes[0].text, "Uezar,");
        assert_eq!(reread(&path), l1, "saved at once");
        assert!(!e.can_undo(), "not an undo step");
        assert!(e.fix_word(&l1, 1, &heard, 0, " Uezar, ").unwrap().is_none());
        // A cut, then its undo: the fix stays, as a rename does.
        let cut = Step::Do(Op::Frames {
            start: 90,
            end: 120,
            keep: false,
        });
        let l2 = step(&mut e, &l1, cut, FPS);
        let back = step(&mut e, &l2, Step::Undo, FPS);
        assert_eq!(back.keep, l0.keep);
        assert_eq!(reread(&path).fixes, l1.fixes);
        // The heard spelling again takes the fix away.
        let l3 = e
            .fix_word(&back, 1, &heard, 0, "Wezar,")
            .unwrap()
            .unwrap()
            .list;
        assert!(l3.fixes.is_empty());
        // 5o: confirmed, the word keeps its heard text in a fix line, saved at once.
        let l4 = e.confirm_word(&l3, 1, &heard, 0).unwrap().unwrap().list;
        assert_eq!(l4.fixes[0].text, "Wezar,");
        assert_eq!(reread(&path), l4);
        assert!(!e.can_undo(), "not an undo step");
        assert!(e.confirm_word(&l4, 1, &heard, 0).unwrap().is_none(), "once");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_fix_lands_on_every_word_heard_alike_and_those_can_be_taken_back() {
        // 5aa: Uezar fixed one "Bane" and the other stayed.
        use super::Words;
        let d = scratch("spread");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let w = |start: f64, text: &str| Word {
            start,
            end: start + 0.3,
            text: format!(" {text}"),
            p: 1.0,
            p_min: None,
        };
        e.words = Some(Arc::new(Words::from_tracked(vec![
            (0, w(2.0, "Bane")),
            (1, w(3.0, "bane,")),
            (0, w(4.0, "Bane")),
            (0, w(5.0, "the")),
        ])));
        let words = e.words.as_ref().unwrap().list.clone();
        let l0 = list(vec![r(1.0, 30.0)]);
        let edited = e
            .fix_word(&l0, 0, &words[0], 0, "Vayne")
            .unwrap()
            .expect("a change");
        let l1 = edited.list;
        assert_eq!(
            l1.fixes.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
            ["Vayne", "Vayne,", "Vayne"]
        );
        assert_eq!(edited.others.len(), 2);
        assert_eq!(reread(&path), l1, "one save");
        assert!(!e.can_undo(), "not an undo step");
        assert_eq!(edited.heard, ["bane,", "Bane"]);
        // Correct's first step finds nothing more to spread.
        let (none, s) = e.spread_all(&l1).unwrap();
        assert!(none.is_none() && s.added.is_empty());
        // The note's Undo: the two others say what was heard again, the one typed stays.
        let (l2, n) = e.keep_heard(&l1, &edited.others, &edited.heard).unwrap();
        let l2 = l2.expect("a change").list;
        assert_eq!(n, 2);
        assert_eq!(
            l2.fixes.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
            ["Vayne", "bane,", "Bane"]
        );
        assert_eq!(reread(&path), l2);
        // And it sticks: Correct's first step does not give them the fix again.
        let (none, s) = e.spread_all(&l2).unwrap();
        assert!(none.is_none() && s.added.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_retyped_sentence_is_saved_at_once_and_replaces_its_word_fixes() {
        // 5x: one fix for the run, saved as a `span` line; the word fix inside it goes.
        use super::Words;
        let d = scratch("span");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let w = |start: f64, text: &str| Word {
            start,
            end: start + 0.3,
            text: format!(" {text}"),
            p: 1.0,
            p_min: None,
        };
        let words = Words::from_tracked(vec![
            (0, w(2.0, "the")),
            (0, w(2.5, "wezar")),
            (0, w(3.0, "went")),
        ]);
        let l0 = list(vec![r(1.0, 30.0)]);
        let l1 = e
            .fix_word(&l0, 0, &words.list[1], 0, "Uezar")
            .unwrap()
            .unwrap()
            .list;
        let run = words
            .retyped(0, 2, "Uezar left.", &Default::default(), &[])
            .unwrap();
        let l2 = e
            .fix_words(&l1, &run, "Uezar left.")
            .unwrap()
            .expect("a change")
            .list;
        assert_eq!(l2.fixes.len(), 1, "{:?}", l2.fixes);
        assert!(l2.fixes[0].is_span());
        assert_eq!(reread(&path), l2, "saved at once");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("\nspan 0 2.000 3.000 Uezar left.\n"));
        assert!(!e.can_undo(), "not an undo step");
        // The heard words again take it away.
        let l3 = e
            .fix_words(&l2, &run, "the  wezar went")
            .unwrap()
            .unwrap()
            .list;
        assert!(l3.fixes.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_moved_edge_moves_the_words_beside_it_and_undoes_with_it() {
        // 2w: kept 10.0-11.0 at 30 fps (frames 300..330). Whisper put "dos" at 10.2-10.8 and "tres"
        // at 10.9-11.6, so moving the end out to 11.4 (frame 342) would make "tres" kept by its
        // middle: "dos" follows the edge, "tres" starts at it and stays cut.
        let d = scratch("edge");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let w = |start, end, text: &str| Word {
            start,
            end,
            text: text.into(),
            p: 1.0,
            p_min: None,
        };
        e.words = Some(Arc::new(Words::from_words(vec![
            w(9.5, 9.95, "pre"),
            w(10.0, 10.2, "uno"),
            w(10.2, 10.8, "dos"),
            w(10.9, 11.6, "tres"),
        ])));
        let l0 = list(vec![r(10.0, 11.0)]);
        let spans = |l: &EditList| -> Vec<(f64, f64, f64)> {
            l.times.iter().map(|t| (t.start, t.from, t.to)).collect()
        };
        let l1 = e
            .move_edge(&l0, (330, 342, true), 330, FPS)
            .unwrap()
            .expect("a change")
            .list;
        assert_eq!(frames(&l1, FPS), vec![(300, 342)]);
        assert_eq!(spans(&l1), [(10.2, 10.2, 11.4), (10.9, 11.4, 11.6)]);
        assert_eq!(reread(&path), l1, "saved with the frames");
        // What the page gets: "tres" now starts at the edge, its middle past it.
        let v = TranscriptView::new(
            e.words.as_ref().unwrap(),
            &Default::default(),
            &[],
            &l1.times,
        );
        assert_eq!((v.start[2], v.end[2], v.start[3]), (10_200, 11_400, 11_400));
        // One undo puts the frames and the words back; redo both again.
        let back = step(&mut e, &l1, Step::Undo, FPS);
        assert_eq!(back.keep, l0.keep);
        assert!(back.times.is_empty());
        let again = step(&mut e, &back, Step::Redo, FPS);
        assert_eq!(again, l1);
        // Moving it back in a little: "dos", now running into it, ends at it; "tres" is not moved.
        let l2 = e
            .move_edge(&l1, (336, 342, false), 342, FPS)
            .unwrap()
            .unwrap()
            .list;
        assert_eq!(spans(&l2), [(10.2, 10.2, 11.2), (10.9, 11.4, 11.6)]);
        // The start out to 9.8 (frame 294): "uno" starts there, "pre" ends there and stays cut.
        let l3 = e
            .move_edge(&l2, (294, 300, true), 300, FPS)
            .unwrap()
            .unwrap()
            .list;
        assert_eq!(frames(&l3, FPS), vec![(294, 336)]);
        assert_eq!(
            spans(&l3),
            [
                (9.5, 9.5, 9.8),
                (10.0, 9.8, 10.2),
                (10.2, 10.2, 11.2),
                (10.9, 11.4, 11.6)
            ]
        );
        // A frame edit that moves no edge of the list (inside a range) leaves the words.
        let l4 = e
            .move_edge(&l3, (310, 312, false), 310, FPS)
            .unwrap()
            .unwrap()
            .list;
        assert_eq!(l4.times, l3.times);
        // Without a transcript it is the frame edit alone.
        e.words = None;
        let l5 = e
            .move_edge(&l0, (330, 342, true), 330, FPS)
            .unwrap()
            .unwrap()
            .list;
        assert!(l5.times.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_word_far_from_the_edge_is_not_stretched_and_one_passed_whole_turns_kept() {
        // 2w: a range kept by hand to 20.0 whose last word ended at 15.0: moving its end out leaves
        // the word (it is not beside the edge). And moving an end over a whole short word keeps
        // that word, the word following the edge stopping at it.
        let d = scratch("edge-far");
        let path = d.join("edit.junctur");
        let mut e = Editor::default();
        e.reset(Some(path));
        let w = |start, end| Word {
            start,
            end,
            text: "w".into(),
            p: 1.0,
            p_min: None,
        };
        e.words = Some(Arc::new(Words::from_words(vec![
            w(14.0, 15.0),
            w(21.0, 21.5),
        ])));
        let l = list(vec![r(10.0, 20.0)]);
        let l1 = e
            .move_edge(&l, (600, 615, true), 600, FPS)
            .unwrap()
            .unwrap()
            .list;
        assert!(l1.times.is_empty(), "{:?}", l1.times);
        e.words = Some(Arc::new(Words::from_words(vec![
            w(10.0, 10.8),
            w(10.9, 11.0),
            w(11.5, 12.0),
        ])));
        let l = list(vec![r(10.0, 10.85)]);
        // 10.85 ends in frame 326; out to 11.1 (frame 333), past the word at 10.9-11.0 whole.
        let l2 = e
            .move_edge(&l, (326, 333, true), 326, FPS)
            .unwrap()
            .unwrap()
            .list;
        let t: Vec<_> = l2.times.iter().map(|t| (t.start, t.from, t.to)).collect();
        assert_eq!(t, [(10.0, 10.0, 10.9)]);
    }

    #[test]
    fn an_edge_moved_across_a_cut_into_the_next_range_trims_none_of_its_words() {
        // 2w repair: kept 10.0-11.0 and 11.5-20.0 at 30 fps (frames 300..330, 345..600); the
        // toolbar steps the first range's end to 11.7 (frame 351), joining the two. "y", kept in the
        // second range, keeps its head, and "dos" follows the edge only to where that range starts.
        let d = scratch("edge-join");
        let mut e = Editor::default();
        e.reset(Some(d.join("edit.junctur")));
        let w = |start, end| Word {
            start,
            end,
            text: "w".into(),
            p: 1.0,
            p_min: None,
        };
        e.words = Some(Arc::new(Words::from_words(vec![
            w(10.2, 10.8),
            w(11.6, 11.9),
        ])));
        let l = list(vec![r(10.0, 11.0), r(11.5, 20.0)]);
        let l1 = e
            .move_edge(&l, (330, 351, true), 330, FPS)
            .unwrap()
            .unwrap()
            .list;
        let t: Vec<_> = l1.times.iter().map(|t| (t.start, t.from, t.to)).collect();
        assert_eq!(t, [(10.2, 10.2, 11.5)]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_word_left_with_under_a_frame_kept_is_cut_whole() {
        // 2w repair: kept 10.0-11.0; the end moved in to 10.7 (frame 321) leaves "z" (10.69-10.9,
        // kept by its middle) under a frame on the kept side: it is cut, its head clipped to the edge.
        let d = scratch("edge-straddle");
        let mut e = Editor::default();
        e.reset(Some(d.join("edit.junctur")));
        e.words = Some(Arc::new(Words::from_words(vec![Word {
            start: 10.69,
            end: 10.9,
            text: "z".into(),
            p: 1.0,
            p_min: None,
        }])));
        let l = list(vec![r(10.0, 11.0)]);
        let l1 = e
            .move_edge(&l, (321, 330, false), 330, FPS)
            .unwrap()
            .unwrap()
            .list;
        let t: Vec<_> = l1.times.iter().map(|t| (t.start, t.from, t.to)).collect();
        assert_eq!(t, [(10.69, 10.7, 10.9)]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
