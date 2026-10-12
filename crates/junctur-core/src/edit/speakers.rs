//! Who speaks when, as the edit list keeps it: a name per speaker and the turns each one talks in.
//!
//! Kept in the edit file, not beside it, because the names are the user's (renamed from the
//! "Speaker 1..N" Junctur starts with) and the edit file is the one piece of project state that is
//! the user's: readable, diffable, carried through a re-cut. The turns come from the `speakers`
//! stage ([`crate::pipeline`]); the words are labelled from them on demand ([`Speakers::of_word`])
//! rather than stored per word, so a transcript and an edit list never have to agree on indices.

use serde::{Deserialize, Serialize};

use super::captions::default_color;

/// One speaker. `id` counts from 1, as the file and the user see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerName {
    pub id: u32,
    pub name: String,
    /// The audio track (0-based ordinal) this speaker is, when speakers were told apart by track
    /// (a mic and a Discord recorded on their own tracks). Every word heard on that track is
    /// theirs, which beats any overlap test when two tracks talk at once.
    pub track: Option<usize>,
    /// Caption colour, `#RRGGBB` (5a): a palette colour by id until the user picks one.
    pub color: String,
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

/// A person a speaker was recognised as (5b): an enrolled voice's name and caption colour, which the
/// speaker takes in every recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub color: String,
}

/// The longest speaker name: a name labels a transcript paragraph, a caption colour and a caption
/// file; past this it is a sentence, not a name.
pub const MAX_NAME: usize = 60;

/// A speaker name as the edit file can keep it: trimmed, one line, not starting like the speaker
/// line's own `track=` / `color=` fields, at most [`MAX_NAME`] characters. Shared by renames and
/// enrolled voices, whose names become speaker names.
pub fn check_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a speaker needs a name".into());
    }
    if name.contains(['\n', '\r']) {
        return Err("a speaker's name is one line".into());
    }
    if name.starts_with("track=") || name.starts_with("color=") {
        return Err("a speaker's name cannot start with \"track=\" or \"color=\"".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("a speaker's name is at most {MAX_NAME} characters"));
    }
    Ok(name)
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

    /// "Speaker 1..N" for `turns` (speakers numbered from 1), with the names and colours of
    /// `previous` kept wherever an id is still there: a re-cut must not undo a rename.
    pub fn new(turns: Vec<Turn>, tracks: &[Option<usize>], previous: Option<&Speakers>) -> Self {
        Self::found(turns, tracks, &[], previous)
    }

    /// [`Speakers::new`], where speaker `i + 1` recognised as `people[i]` (an enrolled voice, 5b)
    /// takes that person's name and colour instead: they are the person's in every recording, and
    /// an id can mean someone else after the voices change.
    pub fn found(
        mut turns: Vec<Turn>,
        tracks: &[Option<usize>],
        people: &[Option<Person>],
        previous: Option<&Speakers>,
    ) -> Self {
        turns.sort_by(|a, b| a.start.total_cmp(&b.start));
        let names = tracks
            .iter()
            .enumerate()
            .map(|(i, &track)| {
                let id = i as u32 + 1;
                let before = previous.and_then(|p| p.names.iter().find(|n| n.id == id));
                let (name, color) = match people.get(i).and_then(Option::as_ref) {
                    Some(p) => (p.name.clone(), p.color.clone()),
                    None => (
                        before.map_or_else(|| default_name(id), |n| n.name.clone()),
                        before.map_or_else(|| default_color(id), |n| n.color.clone()),
                    ),
                };
                SpeakerName {
                    id,
                    name,
                    track,
                    color,
                }
            })
            .collect();
        // To the millisecond, as the file writes them: the list held in memory (the app plays
        // and compares it) is then the list the file reads back.
        let ms = |x: f64| (x * 1000.0).round() / 1000.0;
        let turns = join(turns)
            .into_iter()
            .map(|t| Turn {
                start: ms(t.start),
                end: ms(t.end),
                ..t
            })
            .collect();
        Self { names, turns }
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

    /// Sets speaker `id`'s caption colour (already checked by the caller); false when there is no
    /// such speaker.
    pub fn recolor(&mut self, id: u32, color: &str) -> bool {
        match self.names.iter_mut().find(|n| n.id == id) {
            Some(n) => {
                n.color = color.to_string();
                true
            }
            None => false,
        }
    }

    /// Speaker `id`'s caption colour.
    pub fn color(&self, id: u32) -> Option<&str> {
        self.names
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.color.as_str())
    }

    /// The words' labeller: build once, then ask per word ([`Labeller::of_word`]).
    pub fn labeller(&self) -> Labeller<'_> {
        let mut reach: Vec<(f64, usize)> = Vec::with_capacity(self.turns.len());
        for (i, t) in self.turns.iter().enumerate() {
            match reach.last() {
                Some(&(end, j)) if end >= t.end => reach.push((end, j)),
                _ => reach.push((t.end, i)),
            }
        }
        Labeller { s: self, reach }
    }
}

/// Labels words with speakers. Holds, per turn, the latest end among the turns up to it and which
/// turn that is, so finding the turns that overlap a word stops as soon as no earlier turn can
/// reach it, and the nearest turn before a word is one lookup (a 30 h transcript is a few hundred
/// thousand words against tens of thousands of turns).
pub struct Labeller<'a> {
    s: &'a Speakers,
    reach: Vec<(f64, usize)>,
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
        // Turns starting at or before the word's end; a turn touching a zero-length word (Whisper
        // gives some) counts as overlapping it, with overlap 0.
        let upto = turns.partition_point(|t| t.start <= end);
        let mut best: Option<(f64, u32)> = None;
        for i in (0..upto).rev() {
            if self.reach[i].0 < start {
                break;
            }
            let t = &turns[i];
            if t.end < start {
                continue;
            }
            let overlap = t.end.min(end) - t.start.max(start);
            if best.is_none_or(|(o, _)| overlap > o) {
                best = Some((overlap, t.speaker));
            }
        }
        if let Some((_, s)) = best {
            return Some(s);
        }
        // Nothing overlaps: the turn ending last before the word, or the first one after it.
        // The turn reaching furthest before the word is the nearest one there.
        let before = upto.checked_sub(1).map(|i| {
            let (end, j) = self.reach[i];
            (start - end, turns[j].speaker)
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

impl Labeller<'_> {
    /// Everyone who says a word (5b): its speaker by [`Labeller::of_word`] first, then anyone else
    /// whose turn holds the word's middle. Two people's turns overlap only where both talk at once,
    /// and the transcript is one mixed stream there, so the word is both of theirs. Empty when nobody
    /// is near.
    pub fn speakers_of_word(&self, track: usize, start: f64, end: f64) -> Vec<u32> {
        let Some(first) = self.of_word(track, start, end) else {
            return Vec::new();
        };
        let mut out = vec![first];
        if self.s.names.iter().any(|n| n.track == Some(track)) {
            return out;
        }
        let mid = (start + end) / 2.0;
        let turns = &self.s.turns;
        let upto = turns.partition_point(|t| t.start <= mid);
        for i in (0..upto).rev() {
            if self.reach[i].0 < mid {
                break;
            }
            let t = &turns[i];
            if t.end >= mid && !out.contains(&t.speaker) {
                out.push(t.speaker);
            }
        }
        out
    }
}

pub fn default_name(id: u32) -> String {
    format!("Speaker {id}")
}

/// Joins turns (sorted by start) of one speaker less than [`JOIN_S`] apart. Per speaker, not only
/// when consecutive: since 5b another speaker's overlap turn can sit between two of one person's.
fn join(turns: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    // Index in `out` of each speaker's latest turn.
    let mut last: std::collections::HashMap<u32, usize> = Default::default();
    for t in turns {
        match last.get(&t.speaker) {
            Some(&i) if t.start - out[i].end < JOIN_S => out[i].end = out[i].end.max(t.end),
            _ => {
                last.insert(t.speaker, out.len());
                out.push(t);
            }
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
        assert!(renamed.recolor(2, "#123456"));
        assert!(!renamed.recolor(9, "#123456"));
        assert_eq!(s.color(1), Some("#F2B33D"));
        assert!(!renamed.rename(9, "x"));
        // A re-cut finds three speakers now: the rename stays, the new one is numbered.
        let again = Speakers::new(vec![], &[None, None, None], Some(&renamed));
        assert_eq!(again.name(2), Some("Lucía"));
        assert_eq!(again.color(2), Some("#123456"));
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
        // A zero-length word inside a turn, and one exactly on a turn's edge.
        assert_eq!(s.labeller().of_word(0, 11.5, 11.5), Some(2));
        assert_eq!(s.labeller().of_word(0, 40.0, 40.0), Some(1));
        // Nearest before: the turn reaching furthest, not the last to start.
        let long = Speakers::new(vec![t(0.0, 30.0, 1), t(5.0, 6.0, 2)], &[None, None], None);
        assert_eq!(long.labeller().of_word(0, 31.0, 31.5), Some(1));
        assert_eq!(Speakers::default().labeller().of_word(0, 1.0, 2.0), None);
    }

    #[test]
    fn a_word_where_two_people_talk_at_once_is_both_of_theirs() {
        // Speaker 1 talks 0-10, speaker 2 joins in 4-6 (an overlap turn) and talks 10-12.
        let s = Speakers::new(
            vec![t(0.0, 10.0, 1), t(4.0, 6.0, 2), t(10.0, 12.0, 2)],
            &[None, None],
            None,
        );
        let l = s.labeller();
        assert_eq!(l.speakers_of_word(0, 1.0, 1.4), vec![1]);
        assert_eq!(l.speakers_of_word(0, 4.5, 5.0), vec![2, 1]);
        assert_eq!(l.speakers_of_word(0, 11.0, 11.5), vec![2]);
        // A word straddling a change of speaker: its primary only (the middle is in one turn).
        assert_eq!(l.speakers_of_word(0, 9.7, 10.1), vec![1]);
        assert!(l.speakers_of_word(0, 40.0, 41.0).is_empty());
        // Two of one person's turns around another's overlap turn are still joined.
        let j = Speakers::new(
            vec![t(0.0, 3.0, 1), t(2.5, 3.2, 2), t(3.5, 6.0, 1)],
            &[None, None],
            None,
        );
        assert_eq!(j.turns, vec![t(0.0, 6.0, 1), t(2.5, 3.2, 2)]);
    }

    #[test]
    fn an_enrolled_person_names_their_speaker() {
        let ana = Person {
            name: "Ana".into(),
            color: "#123456".into(),
        };
        let mut before = Speakers::new(vec![], &[None, None], None);
        before.rename(1, "Old");
        before.rename(2, "Kept");
        let s = Speakers::found(vec![], &[None, None], &[Some(ana), None], Some(&before));
        assert_eq!((s.name(1), s.color(1)), (Some("Ana"), Some("#123456")));
        assert_eq!(s.name(2), Some("Kept"));
        // Turns are held to the millisecond, as the file writes them.
        let r = Speakers::new(vec![t(1.23456, 2.0004, 1)], &[None], None);
        assert_eq!(r.turns, vec![t(1.235, 2.0, 1)]);
        assert!(check_name(" Ana ").is_ok_and(|n| n == "Ana"));
        assert!(check_name("").is_err() && check_name("a\nb").is_err());
        assert!(check_name("track=1").is_err() && check_name(&"x".repeat(61)).is_err());
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
