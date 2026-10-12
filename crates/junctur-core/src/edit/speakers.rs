//! Who speaks when, as the edit list keeps it: a name per speaker and the turns each one talks in.
//!
//! Kept in the edit file, not beside it, because the names are the user's (renamed from the
//! "Speaker 1..N" Junctur starts with) and the edit file is the one piece of project state that is
//! the user's: readable, diffable, carried through a re-cut. The turns come from the `speakers`
//! stage ([`crate::pipeline`]); the words are labelled from them on demand ([`Speakers::of_word`])
//! rather than stored per word, so a transcript and an edit list never have to agree on indices.

use serde::{Deserialize, Serialize};

/// One speaker. `id` counts from 1, as the file and the user see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerName {
    pub id: u32,
    pub name: String,
    /// The audio track (0-based ordinal) this speaker is, when speakers were told apart by track
    /// (a mic and a Discord recorded on their own tracks). Every word heard on that track is
    /// theirs, which beats any overlap test when two tracks talk at once.
    pub track: Option<usize>,
}

/// A stretch of the source one speaker talks in, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Speakers {
    pub names: Vec<SpeakerName>,
    /// Sorted by start. Turns of different speakers may overlap (two tracks talking at once).
    pub turns: Vec<Turn>,
}

/// A word further than this from every turn is left unlabelled: it is not near anyone's speech.
const NEAREST_S: f64 = 10.0;

/// Same-speaker turns closer than this are written as one: a turn per breath would make the file
/// (and a later caption cue per turn) choppy for no gain.
pub const JOIN_S: f64 = 1.0;

impl Speakers {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.turns.is_empty()
    }

    /// "Speaker 1..N" for `turns` (speakers numbered from 1), with the names (and tracks) of
    /// `previous` kept wherever an id is still there: a re-cut must not undo a rename.
    pub fn new(
        mut turns: Vec<Turn>,
        tracks: &[Option<usize>],
        previous: Option<&Speakers>,
    ) -> Self {
        turns.sort_by(|a, b| a.start.total_cmp(&b.start));
        let names = tracks
            .iter()
            .enumerate()
            .map(|(i, &track)| {
                let id = i as u32 + 1;
                let name = previous
                    .and_then(|p| p.name(id))
                    .map_or_else(|| default_name(id), str::to_string);
                SpeakerName { id, name, track }
            })
            .collect();
        Self {
            names,
            turns: join(turns),
        }
    }

    pub fn name(&self, id: u32) -> Option<&str> {
        self.names
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.name.as_str())
    }

    /// Renames speaker `id`; false when there is no such speaker. Blank names are refused by the
    /// caller; the file could not write one (the name is the rest of its line).
    pub fn rename(&mut self, id: u32, name: &str) -> bool {
        match self.names.iter_mut().find(|n| n.id == id) {
            Some(n) => {
                n.name = name.trim().to_string();
                true
            }
            None => false,
        }
    }

    /// The words' labeller: build once, then ask per word ([`Labeller::of_word`]).
    pub fn labeller(&self) -> Labeller<'_> {
        let reach = self
            .turns
            .iter()
            .scan(f64::NEG_INFINITY, |m, t| {
                *m = m.max(t.end);
                Some(*m)
            })
            .collect();
        Labeller { s: self, reach }
    }
}

/// Labels words with speakers. Holds the latest turn end up to each turn, so finding the turns
/// that overlap a word stops as soon as no earlier turn can reach it (a 30 h transcript is a few
/// hundred thousand words against tens of thousands of turns).
pub struct Labeller<'a> {
    s: &'a Speakers,
    reach: Vec<f64>,
}

impl Labeller<'_> {
    /// Who says a word heard on `track` from `start` to `end`: the speaker that is that track if
    /// there is one, else the turn overlapping the word most, else the nearest turn within
    /// [`NEAREST_S`] (diarization leaves short or stray stretches unlabelled). `None` when nobody
    /// is near.
    pub fn of_word(&self, track: usize, start: f64, end: f64) -> Option<u32> {
        let turns = &self.s.turns;
        if let Some(n) = self.s.names.iter().find(|n| n.track == Some(track)) {
            return Some(n.id);
        }
        let upto = turns.partition_point(|t| t.start < end);
        let mut best: Option<(f64, u32)> = None;
        for i in (0..upto).rev() {
            if self.reach[i] <= start {
                break;
            }
            let t = &turns[i];
            let overlap = t.end.min(end) - t.start.max(start);
            if overlap > 0.0 && best.is_none_or(|(o, _)| overlap > o) {
                best = Some((overlap, t.speaker));
            }
        }
        if let Some((_, s)) = best {
            return Some(s);
        }
        // Nothing overlaps: the turn ending last before the word, or the first one after it.
        let before = upto
            .checked_sub(1)
            .map(|i| (start - self.reach[i], i))
            .map(|(d, i)| {
                // The turn that reaches furthest is the nearest one before the word.
                let j = (0..=i)
                    .rev()
                    .find(|&j| turns[j].end == self.reach[i])
                    .unwrap_or(i);
                (d, turns[j].speaker)
            });
        let after = turns.get(upto).map(|t| (t.start - end, t.speaker));
        [before, after]
            .into_iter()
            .flatten()
            .filter(|&(d, _)| d <= NEAREST_S)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, s)| s)
    }
}

pub fn default_name(id: u32) -> String {
    format!("Speaker {id}")
}

/// Joins consecutive (by start) turns of one speaker less than [`JOIN_S`] apart.
fn join(turns: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    for t in turns {
        match out.last_mut() {
            Some(last) if last.speaker == t.speaker && t.start - last.end < JOIN_S => {
                last.end = last.end.max(t.end);
            }
            _ => out.push(t),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(start: f64, end: f64, speaker: u32) -> Turn {
        Turn {
            start,
            end,
            speaker,
        }
    }

    #[test]
    fn names_start_numbered_and_survive_a_recut() {
        let s = Speakers::new(vec![t(0.0, 1.0, 1), t(2.0, 3.0, 2)], &[None, None], None);
        assert_eq!(s.name(1), Some("Speaker 1"));
        assert_eq!(s.name(2), Some("Speaker 2"));
        let mut renamed = s.clone();
        assert!(renamed.rename(2, "  Lucía "));
        assert!(!renamed.rename(9, "x"));
        // A re-cut finds three speakers now: the rename stays, the new one is numbered.
        let again = Speakers::new(vec![], &[None, None, None], Some(&renamed));
        assert_eq!(again.name(2), Some("Lucía"));
        assert_eq!(again.name(3), Some("Speaker 3"));
    }

    #[test]
    fn close_turns_of_one_speaker_join_and_others_do_not() {
        let s = Speakers::new(
            vec![
                t(5.0, 6.0, 1),
                t(0.0, 1.0, 1),
                t(1.5, 2.0, 1),
                t(2.2, 3.0, 2),
            ],
            &[None, None],
            None,
        );
        assert_eq!(
            s.turns,
            vec![t(0.0, 2.0, 1), t(2.2, 3.0, 2), t(5.0, 6.0, 1)]
        );
    }

    #[test]
    fn words_go_to_the_most_overlapping_then_the_nearest_turn() {
        let s = Speakers::new(
            vec![t(0.0, 10.0, 1), t(9.0, 12.0, 2), t(40.0, 41.0, 1)],
            &[None, None],
            None,
        );
        assert_eq!(s.labeller().of_word(0, 1.0, 1.5), Some(1));
        // 9.2-10.8 overlaps speaker 1 by 0.8 s and speaker 2 by 1.6 s.
        assert_eq!(s.labeller().of_word(0, 9.2, 10.8), Some(2));
        // Between turns: the nearer one, within reach.
        assert_eq!(s.labeller().of_word(0, 13.0, 13.5), Some(2));
        assert_eq!(s.labeller().of_word(0, 35.0, 35.5), Some(1));
        assert_eq!(s.labeller().of_word(0, 25.0, 25.5), None);
        assert_eq!(Speakers::default().labeller().of_word(0, 1.0, 2.0), None);
    }

    #[test]
    fn a_speaker_that_is_a_track_takes_every_word_on_it() {
        // Two tracks talking at once: the overlap test alone would give the word to speaker 2.
        let s = Speakers::new(
            vec![t(0.0, 1.0, 1), t(0.0, 10.0, 2)],
            &[Some(0), Some(3)],
            None,
        );
        assert_eq!(s.labeller().of_word(0, 2.0, 3.0), Some(1));
        assert_eq!(s.labeller().of_word(3, 0.2, 0.6), Some(2));
    }
}
