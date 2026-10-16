//! Moments (phase 7a, a prototype): points of a recording the streamer marked while playing, each
//! proposed as a clip of the half minute before it. A marker is a phrase said aloud ("clip that",
//! "marca eso"), found in the words Whisper already timed, or a line of a timestamp file a hotkey
//! wrote. A module of its own, not part of smart keep, because later sources (a match against an
//! example, a question) add moments too; it is pure over words and text, and its one file holds only
//! the user's answers, so the moments are rebuilt on every read and a new transcript or an edited
//! markers file shows at once.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::edit::Range;
use crate::text::folded;
use crate::transcribe::Word;
use crate::Result;

/// What a streamer says to mark a moment, English then Spanish, in their natural spelling: matched
/// folded ([`folded`]), so accents, case and punctuation do not matter. One place, so more can be added.
pub const PHRASES: &[&str] = &[
    "clip that",
    "clip this",
    "clip it",
    "cut this",
    "cut that",
    "clipea eso",
    "clipéalo",
    "marca eso",
    "márcalo",
    "corta eso",
];

/// A moment's clip is this much before its marker.
pub const BEFORE_S: f64 = 30.0;
/// Markers closer than this are one moment: "clip that, clip that", or a hotkey and a phrase at once.
const MERGE_S: f64 = 5.0;
/// The words of one phrase are at most this far apart; further, they are two remarks.
const GAP_S: f64 = 1.5;
/// The user's answers, in the project folder: theirs, never rebuilt.
pub const FILE: &str = "moments.json";
/// The timestamp file in the project folder, when there is none beside the recording.
pub const MARKERS: &str = "markers.txt";

/// What marked a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Voice,
    Hotkey,
}

/// One marker: when it was set and, for a phrase, the seconds the phrase is said in (cut once the
/// moment is accepted).
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub at: f64,
    pub source: Source,
    pub cut: Option<(f64, f64)>,
    /// What the phrase was heard as, or the text after a timestamp.
    pub text: String,
}

/// A proposed clip.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Moment {
    /// The first marker's second: what names the moment, and what an answer is matched by.
    pub at: f64,
    pub start: f64,
    pub end: f64,
    /// The phrases said in it, each cut once it is accepted.
    pub cuts: Vec<(f64, f64)>,
    pub source: Source,
    pub label: String,
}

impl Moment {
    /// The seconds an accept edits: the clip and every phrase.
    fn hull(&self) -> (f64, f64) {
        self.cuts
            .iter()
            .fold((self.start, self.end), |(a, b), c| (a.min(c.0), b.max(c.1)))
    }
}

/// The phrase markers in `words` (each with its 0-based track, in time order). A phrase is matched
/// on one track's words, consecutive and at most [`GAP_S`] apart; a word may hold several folded
/// tokens ("clip-that").
pub fn voice_markers(words: &[(usize, &Word)]) -> Vec<Marker> {
    let phrases: Vec<Vec<String>> = PHRASES.iter().map(|p| tokens(p)).collect();
    let mut tracks: Vec<usize> = words.iter().map(|w| w.0).collect();
    tracks.sort_unstable();
    tracks.dedup();
    let mut out = Vec::new();
    for track in tracks {
        // Every token of the track's words, with the word it is in.
        let mine: Vec<&Word> = words.iter().filter(|w| w.0 == track).map(|w| w.1).collect();
        let toks: Vec<(String, usize)> = mine
            .iter()
            .enumerate()
            .flat_map(|(i, w)| tokens(&w.text).into_iter().map(move |t| (t, i)))
            .collect();
        let mut k = 0;
        while k < toks.len() {
            let hit = phrases.iter().find(|p| said_at(&toks, k, p, &mine));
            let Some(p) = hit else {
                k += 1;
                continue;
            };
            let (first, last) = (mine[toks[k].1], mine[toks[k + p.len() - 1].1]);
            let text = mine[toks[k].1..=toks[k + p.len() - 1].1]
                .iter()
                .map(|w| w.text.trim())
                .collect::<Vec<_>>()
                .join(" ");
            out.push(Marker {
                at: first.start,
                source: Source::Voice,
                cut: Some((ms(first.start), ms(last.end.max(first.start)))),
                text,
            });
            k += p.len();
        }
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

fn tokens(s: &str) -> Vec<String> {
    folded(s).split_whitespace().map(str::to_string).collect()
}

/// Whether `phrase` is said from token `k` on: its tokens there, the words they are in near enough.
fn said_at(toks: &[(String, usize)], k: usize, phrase: &[String], words: &[&Word]) -> bool {
    let Some(run) = toks.get(k..k + phrase.len()) else {
        return false;
    };
    let same = run.iter().zip(phrase).all(|((t, _), p)| t == p);
    same && run
        .windows(2)
        .all(|w| w[1].1 == w[0].1 || words[w[1].1].start - words[w[0].1].end <= GAP_S)
}

/// The timestamp file for a recording: `<stem>.markers.txt` beside it, else [`MARKERS`] in its project
/// folder; `None` when neither is there.
pub fn markers_file(source: &Path, project_dir: &Path) -> Option<PathBuf> {
    let beside = source.file_stem().map(|stem| {
        let mut name = stem.to_os_string();
        name.push(".markers.txt");
        source.with_file_name(name)
    });
    beside
        .into_iter()
        .chain(std::iter::once(project_dir.join(MARKERS)))
        .find(|p| p.is_file())
}

/// The markers of a timestamp file's `text`, and how many lines did not read. One per line: seconds
/// (`754.2`) or `m:ss` / `h:mm:ss`, each with optional `.ms`; text after a space, tab or comma is the
/// moment's label. Blank lines and `#` comments are skipped.
pub fn hotkey_markers(text: &str) -> (Vec<Marker>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    // A file written by Windows tools may start with a byte order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (stamp, rest) = line
            .split_once(|c: char| c.is_whitespace() || c == ',')
            .unwrap_or((line, ""));
        match seconds(stamp) {
            Some(at) => out.push(Marker {
                at,
                source: Source::Hotkey,
                cut: None,
                text: rest
                    .trim_matches(|c: char| c.is_whitespace() || c == ',')
                    .into(),
            }),
            None => bad += 1,
        }
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    (out, bad)
}

/// `754.2`, `12:34.2` or `1:02:03.250` as seconds; `None` for anything else, or a time past a part's
/// range (`1:75`).
fn seconds(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    let (last, whole) = parts.split_last()?;
    let digits = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
    let decimal = |p: &str| {
        let (i, f) = p.split_once('.').unwrap_or((p, "0"));
        digits(i) && (f.is_empty() || digits(f))
    };
    if parts.len() > 3 || !whole.iter().all(|p| digits(p)) || !decimal(last) {
        return None;
    }
    let sec: f64 = last.parse().ok()?;
    let mut total = 0.0;
    for (i, p) in whole.iter().enumerate() {
        let v: f64 = p.parse().ok()?;
        // Minutes after hours are under 60, as seconds after minutes are.
        if i > 0 && v >= 60.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    if !whole.is_empty() && sec >= 60.0 {
        return None;
    }
    Some(total * 60.0 + sec)
}

/// The moments of `markers` in a recording `duration` long: each a clip of [`BEFORE_S`] up to its
/// marker, markers closer than [`MERGE_S`] one moment (its clip up to the last of them, every phrase
/// cut), in time order. A marker at 0 s, with nothing before it, makes none.
pub fn moments(mut markers: Vec<Marker>, duration: f64) -> Vec<Moment> {
    markers.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut out: Vec<Moment> = Vec::new();
    for m in markers {
        let at = m.at.clamp(0.0, duration.max(0.0));
        let cuts: Vec<(f64, f64)> = m.cut.into_iter().collect();
        // A moment's end is its latest marker.
        if let Some(o) = out.last_mut().filter(|o| at - o.end < MERGE_S) {
            o.end = o.end.max(ms(at));
            o.cuts.extend(cuts);
            continue;
        }
        let start = (at - BEFORE_S).max(0.0);
        if at - start < 0.5 {
            continue;
        }
        let label = match (m.source, m.text.is_empty()) {
            (Source::Voice, _) => format!("\u{201c}{}\u{201d}", m.text),
            (Source::Hotkey, true) => "Hotkey marker".into(),
            (Source::Hotkey, false) => m.text.clone(),
        };
        out.push(Moment {
            at: ms(at),
            start: ms(start),
            end: ms(at),
            cuts,
            source: m.source,
            label,
        });
    }
    out
}

fn ms(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// What the user did with a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    Accepted,
    Rejected,
}

/// One answer, by the moment's [`Moment::at`]. `before`: what the list kept of the span an accept
/// edited, so undoing it puts back exactly that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub at: f64,
    pub how: How,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<Range>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answers {
    #[serde(default)]
    pub answers: Vec<Answer>,
}

impl Answers {
    pub fn of(&self, m: &Moment) -> Option<&Answer> {
        self.answers.iter().find(|a| (a.at - m.at).abs() < 1e-3)
    }

    fn set(&mut self, a: Answer) {
        self.answers.retain(|x| (x.at - a.at).abs() >= 1e-3);
        self.answers.push(a);
        self.answers.sort_by(|a, b| a.at.total_cmp(&b.at));
    }

    /// Forgets `m`'s answer: it is waiting again.
    pub fn forget(&mut self, m: &Moment) {
        self.answers.retain(|x| (x.at - m.at).abs() >= 1e-3);
    }
}

pub fn load_answers(dir: &Path) -> Result<Answers> {
    match std::fs::read(dir.join(FILE)) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Answers::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_answers(dir: &Path, a: &Answers) -> Result<()> {
    crate::output::write_replacing(&dir.join(FILE), serde_json::to_vec_pretty(a)?)?;
    Ok(())
}

/// The edits (start, end, keep) that accept `take` (moments of `all`), in order: every clip kept,
/// then every phrase of an accepted moment that a kept clip reaches cut, so a clip overlapping one
/// accepted before does not bring its phrase back. Each answer goes in `answers` with what `now` (the
/// list's kept ranges, sorted) keeps of the span it edits.
pub fn accept(
    take: &[&Moment],
    all: &[Moment],
    now: &[Range],
    answers: &mut Answers,
) -> Vec<(f64, f64, bool)> {
    let mut edits = Vec::new();
    for m in take {
        let (a, b) = m.hull();
        let from = now.partition_point(|r| r.end <= a);
        let before = now[from..]
            .iter()
            .take_while(|r| r.start < b)
            .map(|r| Range {
                start: r.start.max(a),
                end: r.end.min(b),
            })
            .collect();
        answers.set(Answer {
            at: m.at,
            how: How::Accepted,
            before,
        });
        edits.push((m.start, m.end, true));
    }
    let reached = |&(a, b): &(f64, f64)| {
        take.iter().any(|m| {
            let (x, y) = m.hull();
            a < y && x < b
        })
    };
    let accepted = all
        .iter()
        .filter(|m| answers.of(m).is_some_and(|x| x.how == How::Accepted));
    edits.extend(
        accepted
            .flat_map(|m| m.cuts.iter().copied())
            .filter(reached)
            .map(|(a, b)| (a, b, false)),
    );
    edits
}

/// Turns `m` down: no edit.
pub fn reject(m: &Moment, answers: &mut Answers) {
    answers.set(Answer {
        at: m.at,
        how: How::Rejected,
        before: Vec::new(),
    });
}

/// The edits that undo an accepted `m` (the span cut, then what was kept there kept again), its
/// answer forgotten so it is waiting again; nothing for a moment not accepted.
pub fn undo(m: &Moment, answers: &mut Answers) -> Vec<(f64, f64, bool)> {
    let Some(a) = answers.of(m).filter(|a| a.how == How::Accepted).cloned() else {
        return Vec::new();
    };
    answers.forget(m);
    let (from, to) = m.hull();
    let mut edits = vec![(from, to, false)];
    edits.extend(a.before.iter().map(|r| (r.start, r.end, true)));
    edits
}

#[cfg(test)]
mod tests;
