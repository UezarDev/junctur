//! Who speaks when, as the edit list keeps it (the names are the user's): a name per speaker and the
//! turns each one talks in. Words are labelled from the turns on demand rather than stored per word,
//! so a transcript and an edit list never have to agree on indices.

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
    /// Caption colour, `#RRGGBB`: a palette colour by id until the user picks one.
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

/// A person a speaker was recognised as: an enrolled voice's name and caption colour, which the
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

    /// [`Speakers::new`], where speaker `i + 1` recognised as `people[i]` takes that person's name
    /// and colour. Others take those of the `previous` speaker on the same track, else with most
    /// turn overlap (not by id: ids renumber when the voices change), never a name in `exclude`
    /// (a voice's). Speakers left unnamed are "Speaker N" in the first palette colour nobody else has.
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

    /// Gives `a..b` to speaker `id`: everyone else's turns there are cut out, `id`'s own are joined
    /// into it (the roll call's "who is this", 7a). False when there is no such speaker or the span is
    /// empty.
    pub fn assign(&mut self, a: f64, b: f64, id: u32) -> bool {
        // To the millisecond, as the file writes turns ([`Speakers::found`]).
        let (a, b) = ((a * 1000.0).round() / 1000.0, (b * 1000.0).round() / 1000.0);
        if b <= a || !self.names.iter().any(|n| n.id == id) {
            return false;
        }
        let mut turns = Vec::with_capacity(self.turns.len() + 2);
        let (mut lo, mut hi) = (a, b);
        for t in &self.turns {
            if t.end <= a || t.start >= b {
                turns.push(*t);
            } else if t.speaker == id {
                (lo, hi) = (lo.min(t.start), hi.max(t.end));
            } else {
                if t.start < a {
                    turns.push(Turn { end: a, ..*t });
                }
                if t.end > b {
                    turns.push(Turn { start: b, ..*t });
                }
            }
        }
        turns.push(Turn {
            start: lo,
            end: hi,
            speaker: id,
        });
        turns.sort_by(|x, y| x.start.total_cmp(&y.start));
        self.turns = turns;
        true
    }

    /// A new speaker called `name` (already checked by the caller), with the next id and a palette
    /// colour nobody has; returns the id.
    pub fn add(&mut self, name: &str) -> u32 {
        let id = self.names.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        let used: Vec<String> = self.names.iter().map(|n| n.color.clone()).collect();
        self.names.push(SpeakerName {
            id,
            name: name.trim().to_string(),
            track: None,
            color: free_color(id, &used),
        });
        id
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
    /// Everyone who says a word: its speaker by [`Labeller::of_word`] first, then anyone else
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
        // Nothing to match by: by id.
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
    let new: Vec<usize> = (0..n)
        .filter(|&i| out[i].is_none() && !is_person(i))
        .collect();
    let open = |j: usize| !taken[j] && allowed(&previous.names[j]);
    let mut pairs = overlaps(turns, &new, previous, &open);
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

/// The turns of speaker `id` as spans.
fn spans(ts: &[Turn], id: u32) -> Vec<(f64, f64)> {
    ts.iter()
        .filter(|t| t.speaker == id)
        .map(|t| (t.start, t.end))
        .collect()
}

/// `(seconds shared, new speaker, previous speaker)` for each of the `new` speakers (indices, id
/// one more) and each previous one `open` allows, where they share any time.
fn overlaps(
    turns: &[Turn],
    new: &[usize],
    previous: &Speakers,
    open: &dyn Fn(usize) -> bool,
) -> Vec<(f64, usize, usize)> {
    // Each previous speaker's spans built once, when first needed, not once per new speaker.
    let mut before: Vec<Option<Vec<(f64, f64)>>> = vec![None; previous.names.len()];
    let mut pairs = Vec::new();
    for &i in new {
        let a = spans(turns, i as u32 + 1);
        for (j, p) in previous.names.iter().enumerate() {
            if !open(j) {
                continue;
            }
            let b = before[j].get_or_insert_with(|| spans(&previous.turns, p.id));
            let o = overlap(&a, b);
            if o > 0.0 {
                pairs.push((o, i, j));
            }
        }
    }
    pairs
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
/// when consecutive: another speaker's overlap turn can sit between two of one person's.
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
mod tests;
