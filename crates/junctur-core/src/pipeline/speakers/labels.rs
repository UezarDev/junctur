//! Who each speaker is: the clusters heard matched to the remembered voices, the user's pins
//! applied, and stretches of two talking at once given to the pair heard around them.

use std::path::Path;

use super::embeddings::{heard_embeddings, HEARD_BY_AN_OLDER_JUNCTUR};
use super::{outside, Found, Method};
use crate::diarize::{self, Overlap, CENTROID_MATCH, MINOR_SHARE, VOICE_MATCH};
use crate::edit::{Person, Turn};
use crate::voices::Voice;
use crate::Result;

/// An overlap's local speaker is named by the speaker whose turns cover most of where it talks
/// alone in its window, when they cover at least this much.
const MIN_EVIDENCE_S: f64 = 0.5;

/// When only one of an overlap's two people is known, the other is the nearest other speaker
/// within this many seconds of it.
const NEIGHBOUR_S: f64 = 3.0;

/// `found` labelled again with `voices` from what was heard (no audio, no model): what enrolling,
/// forgetting or renaming a voice calls. Speakers that are tracks, or none, are returned as they
/// are.
pub fn relabel(found: &Found, dir: &Path, voices: &[Voice]) -> Result<Found> {
    if found.method != Method::Voices {
        return Ok(found.clone());
    }
    let e = found
        .heard
        .as_ref()
        .and_then(|h| heard_embeddings(h, dir))
        .ok_or_else(|| crate::Error::Edit(HEARD_BY_AN_OLDER_JUNCTUR.into()))?;
    Ok(label(found, &e, voices))
}

/// Labels the heard windows ([`diarize::label`]); turns from the windows, plus a turn for each of
/// the two people in every overlap stretch. Pins of a voice in `voices` win: inside a pin there is
/// exactly one speaker, the pinned one, who gets a speaker even if no window came out as them.
pub fn label(found: &Found, embeddings: &[Vec<f32>], voices: &[Voice]) -> Found {
    let Some(heard) = &found.heard else {
        return found.clone();
    };
    let prints: Vec<Vec<f32>> = voices.iter().map(|v| v.print.clone()).collect();
    // Pins of the voices here, by voice index; pins are sorted and never overlap.
    let pins: Vec<(f64, f64, usize)> = found
        .pins
        .iter()
        .filter_map(|p| {
            let name = p.name.to_lowercase();
            voices
                .iter()
                .position(|v| v.name.to_lowercase() == name)
                .map(|v| (p.start, p.end, v))
        })
        .collect();
    let spans: Vec<(f64, f64)> = pins.iter().map(|&(a, b, _)| (a, b)).collect();
    let fixed: Vec<Option<usize>> = heard
        .windows
        .iter()
        .map(|w| {
            let mid = (w.start + w.end) / 2.0;
            let i = pins.partition_point(|p| p.1 < mid);
            pins.get(i).filter(|p| p.0 <= mid).map(|p| p.2)
        })
        .collect();
    let mut labels = diarize::label_fixed(
        embeddings,
        &prints,
        &fixed,
        VOICE_MATCH,
        CENTROID_MATCH,
        MINOR_SHARE,
    );
    for &(_, _, v) in &pins {
        if !labels.voices.contains(&Some(v)) {
            labels.voices.push(Some(v));
        }
    }
    let id_of = |v: usize| {
        labels
            .voices
            .iter()
            .position(|&x| x == Some(v))
            .expect("every pinned voice has a speaker") as u32
            + 1
    };
    let windows = diarize::turns(&heard.windows, &labels.windows);
    // Window turns outside the pins, and the pins: still sorted and never overlapping, which
    // `who`, `covering` and `neighbour` rely on.
    let mut turns: Vec<Turn> = windows
        .iter()
        .flat_map(|t| {
            outside(t.start, t.end, &spans)
                .into_iter()
                .map(move |(start, end)| Turn {
                    start,
                    end,
                    speaker: t.speaker as u32 + 1,
                })
        })
        .chain(pins.iter().map(|&(start, end, v)| Turn {
            start,
            end,
            speaker: id_of(v),
        }))
        .collect();
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    let base = turns.clone();
    for o in &heard.overlaps {
        let pair = match (who(&o.a, &base), who(&o.b, &base)) {
            (Some(a), Some(b)) if a != b => Some((a, b)),
            // One of the two is known (or both came out as one person): the other is whoever
            // else talks nearest to the stretch, as the one cutting in or being cut in on.
            (Some(x), _) | (_, Some(x)) => neighbour(o, x, &base).map(|y| (x, y)),
            // Neither talks alone in the window: whoever the windows say talks there, and the
            // nearest other speaker.
            (None, None) => covering(o, &base).and_then(|x| neighbour(o, x, &base).map(|y| (x, y))),
        };
        for speaker in pair.into_iter().flat_map(|(a, b)| [a, b]) {
            for (start, end) in outside(o.start, o.end, &spans) {
                turns.push(Turn {
                    start,
                    end,
                    speaker,
                });
            }
        }
    }
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut known = found.known.clone();
    add_known(&mut known, found.people.iter().flatten().map(|p| &p.name));
    add_known(&mut known, voices.iter().map(|v| &v.name));
    Found {
        known,
        method: Method::Voices,
        tracks: vec![None; labels.voices.len()],
        people: labels
            .voices
            .iter()
            .map(|v| {
                v.map(|i| Person {
                    name: voices[i].name.clone(),
                    color: voices[i].color.clone(),
                })
            })
            .collect(),
        turns,
        speech: found.speech.clone(),
        heard: found.heard.clone(),
        pins: found.pins.clone(),
    }
}

/// The speaker whose `turns` (from windows: sorted, never overlapping) cover most of `spans`, when
/// they cover at least [`MIN_EVIDENCE_S`].
fn who(spans: &[[f64; 2]], turns: &[Turn]) -> Option<u32> {
    let mut time: Vec<(u32, f64)> = Vec::new();
    for &[s, e] in spans {
        let from = turns.partition_point(|t| t.end <= s);
        for t in turns[from..].iter().take_while(|t| t.start < e) {
            let o = t.end.min(e) - t.start.max(s);
            if o <= 0.0 {
                continue;
            }
            match time.iter_mut().find(|(id, _)| *id == t.speaker) {
                Some((_, x)) => *x += o,
                None => time.push((t.speaker, o)),
            }
        }
    }
    time.into_iter()
        .filter(|&(_, x)| x >= MIN_EVIDENCE_S)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

fn add_known<'a>(known: &mut Vec<String>, names: impl Iterator<Item = &'a String>) {
    for n in names {
        if !known.iter().any(|k| k.to_lowercase() == n.to_lowercase()) {
            known.push(n.clone());
        }
    }
}

/// Adds to `found`'s [`Found::known`] the voice names `old` (the result it replaces) knew.
pub fn remember_known(found: &mut Found, old: &Found) {
    let mut known = std::mem::take(&mut found.known);
    add_known(&mut known, old.known.iter());
    add_known(&mut known, old.people.iter().flatten().map(|p| &p.name));
    found.known = known;
}

/// The speaker whose turn (from windows) holds the middle of overlap `o`.
fn covering(o: &Overlap, turns: &[Turn]) -> Option<u32> {
    let mid = (o.start + o.end) / 2.0;
    let i = turns.partition_point(|t| t.end < mid);
    turns.get(i).filter(|t| t.start <= mid).map(|t| t.speaker)
}

/// The speaker other than `x` whose turn (from windows) is nearest to overlap `o`, within
/// [`NEIGHBOUR_S`].
fn neighbour(o: &Overlap, x: u32, turns: &[Turn]) -> Option<u32> {
    let from = turns.partition_point(|t| t.end < o.start - NEIGHBOUR_S);
    turns[from..]
        .iter()
        .take_while(|t| t.start <= o.end + NEIGHBOUR_S)
        .filter(|t| t.speaker != x)
        .map(|t| {
            let gap = (t.start - o.end).max(o.start - t.end).max(0.0);
            (gap, t.speaker)
        })
        .filter(|&(gap, _)| gap <= NEIGHBOUR_S)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}
