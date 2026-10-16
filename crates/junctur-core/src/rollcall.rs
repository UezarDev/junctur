//! The speaker roll call (phase 7a, a prototype): after the speakers are found, the app shows one card
//! per voice with a few seconds of it to play, and asks about the stretches the speakers are least
//! sure of. This is what those cards are made from, pure over the edit list's speakers and the word
//! times, so the app only gathers and shows; a module because neither the speakers stage nor the edit
//! list owns "which seconds best show a voice".

use serde::Serialize;

use crate::edit::{Speakers, Turn};

/// A sample is at most this long, cut from the middle of the turn it is from.
pub const SAMPLE_S: f64 = 3.0;
/// A turn shorter than this, with the same other speaker on both sides, may have been split off them.
const ISLAND_S: f64 = 2.0;
/// How close those neighbours are.
const NEIGHBOUR_S: f64 = 1.5;
/// Words nobody's turn is near, closer than this, are one stretch.
const JOIN_S: f64 = 2.0;
/// At most this many stretches are asked about: a roll call, not a review of the recording.
pub const MAX_UNSURE: usize = 40;

/// Whether `t` overlaps a turn of anyone else.
fn shared(s: &Speakers, t: &Turn) -> bool {
    s.turns
        .iter()
        .any(|o| o.speaker != t.speaker && o.start < t.end && t.start < o.end)
}

/// Speaker `id`'s turns nobody else talks in, longest first.
fn clean(s: &Speakers, id: u32) -> Vec<Turn> {
    let mut v: Vec<Turn> = s
        .turns
        .iter()
        .filter(|t| t.speaker == id && !shared(s, t))
        .copied()
        .collect();
    v.sort_by(|a, b| (b.end - b.start).total_cmp(&(a.end - a.start)));
    v
}

/// The seconds to play for speaker `id`: the middle [`SAMPLE_S`] of their longest turn nobody else
/// talks in (else of their longest turn), all of it when shorter; `None` when they have no turn.
pub fn sample(s: &Speakers, id: u32) -> Option<(f64, f64)> {
    let longest = |v: &mut dyn Iterator<Item = Turn>| {
        v.max_by(|a, b| (a.end - a.start).total_cmp(&(b.end - b.start)))
    };
    let t = clean(s, id)
        .first()
        .copied()
        .or_else(|| longest(&mut s.turns.iter().copied().filter(|t| t.speaker == id)))?;
    let len = (t.end - t.start).min(SAMPLE_S);
    let mid = (t.start + t.end) / 2.0;
    Some((mid - len / 2.0, mid + len / 2.0))
}

/// What "Remember this voice" enrols for speaker `id`: their turns nobody else talks in, longest
/// first, up to `max_s` seconds (the last one cut short), in time order. A few seconds alone would be
/// "a very short clip" to recognise them by in other recordings.
pub fn voice_stretches(s: &Speakers, id: u32, max_s: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut left = max_s;
    for t in clean(s, id) {
        if left <= 0.0 {
            break;
        }
        let len = (t.end - t.start).min(left);
        out.push((t.start, t.start + len));
        left -= len;
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Why a stretch is asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// A short turn with one other speaker on both sides: maybe theirs, split off by the clustering.
    Island,
    /// Words nobody's turn is near.
    Nobody,
}

/// A stretch to ask "who is this" about.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unsure {
    pub start: f64,
    pub end: f64,
    /// Who the list says it is now.
    pub speaker: Option<u32>,
    /// The audio track (0-based) its words were heard on; `None` for an island (the voices' track).
    pub track: Option<usize>,
    pub why: Why,
}

/// The stretches the speakers are least sure of, at most [`MAX_UNSURE`] (past that the shortest
/// islands and the longest runs nobody says, the likeliest mistakes), in time order. The
/// clustering keeps no sureness per turn, so this reads it from the turns: islands ([`Why::Island`]),
/// and runs of `words` (track, start, end) nobody's turn is near ([`Why::Nobody`]). Speakers that are
/// tracks are sure by construction: none of theirs is asked about.
pub fn unsure(s: &Speakers, words: &[(usize, f64, f64)]) -> Vec<Unsure> {
    let by_track = |id: u32| s.names.iter().any(|n| n.id == id && n.track.is_some());
    let mut out: Vec<Unsure> = s
        .turns
        .iter()
        .enumerate()
        .filter(|&(i, t)| {
            let (Some(before), Some(after)) =
                (i.checked_sub(1).map(|j| s.turns[j]), s.turns.get(i + 1))
            else {
                return false;
            };
            t.end - t.start < ISLAND_S
                && !by_track(t.speaker)
                && before.speaker == after.speaker
                && before.speaker != t.speaker
                && t.start - before.end <= NEIGHBOUR_S
                && after.start - t.end <= NEIGHBOUR_S
        })
        .map(|(_, t)| Unsure {
            start: t.start,
            end: t.end,
            speaker: Some(t.speaker),
            track: None,
            why: Why::Island,
        })
        .collect();
    let labeller = s.labeller();
    let mut run: Option<(f64, f64, usize)> = None;
    let mut runs = Vec::new();
    for &(track, a, b) in words {
        if labeller.of_word(track, a, b).is_some() {
            continue;
        }
        run = match run {
            Some((x, y, k)) if k == track && a - y < JOIN_S => Some((x, y.max(b), k)),
            Some(r) => {
                runs.push(r);
                Some((a, b, track))
            }
            None => Some((a, b, track)),
        };
    }
    runs.extend(run);
    out.extend(runs.into_iter().map(|(start, end, track)| Unsure {
        start,
        end,
        speaker: None,
        track: Some(track),
        why: Why::Nobody,
    }));
    if out.len() > MAX_UNSURE {
        // An island is likelier wrong the shorter it is; words nobody says, the more of them.
        let doubt = |u: &Unsure| match u.why {
            Why::Island => ISLAND_S - (u.end - u.start),
            Why::Nobody => u.end - u.start,
        };
        out.sort_by(|a, b| doubt(b).total_cmp(&doubt(a)));
        out.truncate(MAX_UNSURE);
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
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

    fn two(turns: Vec<Turn>) -> Speakers {
        Speakers::new(turns, &[None, None], None)
    }

    #[test]
    fn a_sample_is_the_middle_of_the_longest_turn_nobody_shares() {
        // Speaker 1's longest turn (20 s) is shared with speaker 2; their longest clean one is 8 s.
        let s = two(vec![
            t(0.0, 20.0, 1),
            t(5.0, 6.0, 2),
            t(30.0, 38.0, 1),
            t(40.0, 41.0, 1),
        ]);
        assert_eq!(sample(&s, 1), Some((32.5, 35.5)));
        // Speaker 2 only talks over 1: their one turn, whole, being short.
        assert_eq!(sample(&s, 2), Some((5.0, 6.0)));
        assert_eq!(sample(&s, 7), None);
        // Enrolled from the clean turns, longest first, up to the seconds asked, in time order.
        assert_eq!(voice_stretches(&s, 1, 8.5), [(30.0, 38.0), (40.0, 40.5)]);
        assert!(voice_stretches(&s, 2, 30.0).is_empty());
    }

    #[test]
    fn islands_and_words_nobody_says_are_asked_about() {
        let s = two(vec![
            t(0.0, 10.0, 1),
            t(10.5, 11.5, 2),
            t(12.0, 20.0, 1),
            // A long turn between the same speaker is no island, nor is one far from them.
            t(21.0, 25.0, 2),
            t(26.0, 30.0, 1),
        ]);
        let words = [
            (0, 1.0, 1.3),
            (0, 60.0, 60.4),
            (0, 61.0, 61.5),
            (0, 90.0, 90.2),
        ];
        let u = unsure(&s, &words);
        assert_eq!(
            u,
            [
                Unsure {
                    start: 10.5,
                    end: 11.5,
                    speaker: Some(2),
                    track: None,
                    why: Why::Island
                },
                Unsure {
                    start: 60.0,
                    end: 61.5,
                    speaker: None,
                    track: Some(0),
                    why: Why::Nobody
                },
                Unsure {
                    start: 90.0,
                    end: 90.2,
                    speaker: None,
                    track: Some(0),
                    why: Why::Nobody
                },
            ]
        );
        // Speakers that are tracks are never asked about: every word on a track is theirs.
        let tracks = Speakers::new(
            vec![t(0.0, 10.0, 1), t(10.5, 11.5, 2), t(12.0, 20.0, 1)],
            &[Some(0), Some(1)],
            None,
        );
        assert!(unsure(&tracks, &[(0, 60.0, 60.4)]).is_empty());
    }
}
