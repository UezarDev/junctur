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
    /// `previous` carried over to the same speakers: a re-cut must not undo a rename.
    pub fn new(turns: Vec<Turn>, tracks: &[Option<usize>], previous: Option<&Speakers>) -> Self {
        Self::found(turns, tracks, &[], previous, &[])
    }

    /// [`Speakers::new`], where speaker `i + 1` recognised as `people[i]` (an enrolled voice, 5b)
    /// takes that person's name and colour: they are the person's in every recording.
    ///
    /// Every other speaker takes the name and colour of the `previous` speaker it is, found by
    /// what they say rather than by id: the speaker that is the same track, else the one whose
    /// turns overlap its turns most (one to one, the largest overlaps first). Ids are renumbered
    /// whenever the voices change (enrolled people come first), so carrying by id gave a rename to
    /// someone else (the 5b review). With no turns on either side, by id. A previous
    /// name in `exclude` (case aside) is never carried: those are the people's names, current or
    /// forgotten, which belong to a voice, not to whatever cluster now sits where they were.
    /// Speakers left without a name are "Speaker N" in the first palette colour no one else in
    /// the list has.
    pub fn found(
        mut turns: Vec<Turn>,
        tracks: &[Option<usize>],
        people: &[Option<Person>],
        previous: Option<&Speakers>,
        exclude: &[String],
    ) -> Self {
        turns.sort_by(|a, b| a.start.total_cmp(&b.start));
        // To the millisecond, as the file writes them: the list held in memory (the app plays
        // and compares it) is then the list the file reads back.
        let ms = |x: f64| (x * 1000.0).round() / 1000.0;
        let turns: Vec<Turn> = join(turns)
            .into_iter()
            .map(|t| Turn {
                start: ms(t.start),
                end: ms(t.end),
                ..t
            })
            .collect();
        let n = tracks.len();
        let person = |i: usize| people.get(i).and_then(Option::as_ref);
        let carried = previous.map_or_else(
            || vec![None; n],
            |p| carry(&turns, tracks, n, p, exclude, &|i| person(i).is_some()),
        );
        let mut names: Vec<SpeakerName> = (0..n)
            .map(|i| {
                let id = i as u32 + 1;
                let (name, color) = match (person(i), carried[i]) {
                    (Some(p), _) => (p.name.clone(), p.color.clone()),
                    // A name Junctur gave ("Speaker 3") is a number, not a name: it is not
                    // carried, or two speakers could end up both "Speaker 3".
                    (None, Some(c)) if c.name == default_name(c.id) => {
                        (default_name(id), c.color.clone())
                    }
                    (None, Some(c)) => (c.name.clone(), c.color.clone()),
                    (None, None) => (default_name(id), String::new()),
                };
                SpeakerName {
                    id,
                    name,
                    track: tracks[i],
                    color,
                }
            })
            .collect();
        // Colours last, so a new speaker never takes a colour a person or a carried name has.
        for i in 0..n {
            if names[i].color.is_empty() {
                let used: Vec<String> = names.iter().map(|x| x.color.clone()).collect();
                names[i].color = free_color(names[i].id, &used);
            }
        }
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

/// The palette colour for speaker `id`, or the next one along that no one in `used` has; when
/// every palette colour is taken, `id`'s own.
fn free_color(id: u32, used: &[String]) -> String {
    (0..8u32)
        .map(|k| default_color(id + k))
        .find(|c| !used.iter().any(|u| u.eq_ignore_ascii_case(c)))
        .unwrap_or_else(|| default_color(id))
}

/// For each of the `n` new speakers, the previous speaker it is (see [`Speakers::found`]); `None`
/// for one that is a person (`is_person`) or matches nobody.
fn carry<'p>(
    turns: &[Turn],
    tracks: &[Option<usize>],
    n: usize,
    previous: &'p Speakers,
    exclude: &[String],
    is_person: &dyn Fn(usize) -> bool,
) -> Vec<Option<&'p SpeakerName>> {
    let allowed = |p: &SpeakerName| {
        !exclude
            .iter()
            .any(|x| x.to_lowercase() == p.name.to_lowercase())
    };
    let mut out: Vec<Option<&SpeakerName>> = vec![None; n];
    let mut taken = vec![false; previous.names.len()];
    // The same track first: exact.
    for (i, slot) in out.iter_mut().enumerate() {
        if is_person(i) {
            continue;
        }
        let Some(track) = tracks[i] else { continue };
        if let Some(j) = previous.names.iter().position(|p| p.track == Some(track)) {
            if !taken[j] && allowed(&previous.names[j]) {
                taken[j] = true;
                *slot = Some(&previous.names[j]);
            }
        }
    }
    if previous.turns.is_empty() || turns.is_empty() {
        // Nothing to match by: by id, as before 5b.
        for (i, slot) in out.iter_mut().enumerate() {
            if slot.is_some() || is_person(i) {
                continue;
            }
            let id = i as u32 + 1;
            if let Some(j) = previous.names.iter().position(|p| p.id == id) {
                if !taken[j] && allowed(&previous.names[j]) {
                    taken[j] = true;
                    *slot = Some(&previous.names[j]);
                }
            }
        }
        return out;
    }
    // Overlap seconds between each new and each previous speaker's turns.
    let spans = |ts: &[Turn], id: u32| -> Vec<(f64, f64)> {
        ts.iter()
            .filter(|t| t.speaker == id)
            .map(|t| (t.start, t.end))
            .collect()
    };
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for i in (0..n).filter(|&i| out[i].is_none() && !is_person(i)) {
        let a = spans(turns, i as u32 + 1);
        for (j, p) in previous.names.iter().enumerate() {
            if taken[j] || !allowed(p) {
                continue;
            }
            let o = overlap(&a, &spans(&previous.turns, p.id));
            if o > 0.0 {
                pairs.push((o, i, j));
            }
        }
    }
    // The largest overlaps first; on a tie, the same id (a re-cut of the same labels).
    pairs.sort_by(|a, b| {
        b.0.total_cmp(&a.0).then_with(|| {
            (previous.names[b.2].id == b.1 as u32 + 1)
                .cmp(&(previous.names[a.2].id == a.1 as u32 + 1))
        })
    });
    for (_, i, j) in pairs {
        if out[i].is_none() && !taken[j] {
            taken[j] = true;
            out[i] = Some(&previous.names[j]);
        }
    }
    out
}

/// Seconds two sorted lists of spans share (each list's spans may touch or overlap each other).
fn overlap(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
    let (mut i, mut j, mut total) = (0, 0, 0.0);
    while i < a.len() && j < b.len() {
        let o = a[i].1.min(b[j].1) - a[i].0.max(b[j].0);
        if o > 0.0 {
            total += o;
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    total
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
        // Renumbered (the two swap places): a rename goes with what the speaker says, and so
        // does a colour, but "Speaker N" stays a number and never doubles up.
        let swapped = Speakers::new(
            vec![t(0.0, 1.0, 2), t(2.0, 3.0, 1)],
            &[None, None],
            Some(&renamed),
        );
        assert_eq!(
            (swapped.name(1), swapped.color(1)),
            (Some("Lucía"), Some("#123456"))
        );
        assert_eq!(
            (swapped.name(2), swapped.color(2)),
            (Some("Speaker 2"), Some("#F2B33D"))
        );
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
        let s = Speakers::found(
            vec![],
            &[None, None],
            &[Some(ana), None],
            Some(&before),
            &[],
        );
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
