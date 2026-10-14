//! Words the user retimed (2w): a word's start and end where a moved cut edge put them, kept in the
//! edit list as `time <track> <start>[+<n>] <new start> <new end>` and used wherever a word's time
//! is (the transcript's kept and cut, the playhead's word, the caption preview, the SRT, the send
//! to Resolve, the sentences translated).
//!
//! The point of moving a cut's edge is to fix a word Whisper timed badly, which the cut then halved:
//! moving only the cut left the word's own span where Whisper put it, so the next word, now inside
//! the kept frames by its middle, showed as kept (Uezar: "it seems to be moving just the cut for the
//! video, not the domain of the word"). So the word moves with the edge, and the transcription stays
//! as it was: the override is the user's edit, in the edit list beside the text fixes (5h) for the
//! same reasons (saved with every other edit, kept through re-cuts, backed up), not written into
//! the transcript, a cache a re-run replaces whole.
//!
//! A word is named exactly as a fix names it: its track, its start as Whisper heard it to the
//! millisecond, and which of that millisecond's words on the track it is (5w). Never by its new
//! start, which the next move changes, and which the counting of `nth` (in heard order) cannot see.
//! A module of its own rather than more of `fixes.rs`: a fix is text and is read where text is, an
//! override is times and is read before a word is placed on the timeline at all.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::fixes::{millis, written};
use crate::transcribe::Word;

/// One retimed word.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Retime {
    /// The audio track (0-based) it was heard on.
    pub track: usize,
    /// Its start in source seconds as the transcript has it: what names it.
    pub start: f64,
    /// Which of the words heard on `track` starting in that millisecond it is (as a fix's `nth`).
    pub nth: usize,
    /// Where it now starts and ends, in source seconds, on the millisecond grid; `from < to`.
    pub from: f64,
    pub to: f64,
}

type Key = (usize, i64, usize);

impl Retime {
    fn key(&self) -> Key {
        (self.track, millis(self.start), self.nth)
    }
}

/// Seconds on the list's millisecond grid, as a `time` line writes them, so a list read back
/// equals the one written.
pub fn to_ms(s: f64) -> f64 {
    millis(s) as f64 / 1000.0
}

/// Sets the time of the word heard on `track` at `start` (the `nth` of that millisecond), heard
/// from `heard.0` to `heard.1`, to `from..to`, all rounded to the millisecond as the list writes
/// them, so the list in play equals the one read back (2w repair). Its time as heard
/// again takes the override away. Kept in time order. Whether anything changed. Refused for a span
/// that does not run forwards: a word's middle is what places it, and a word with none has no place.
pub fn set_time(
    times: &mut Vec<Retime>,
    track: usize,
    start: f64,
    nth: usize,
    heard: (f64, f64),
    (from, to): (f64, f64),
) -> Result<bool, String> {
    let (from, to) = (to_ms(from), to_ms(to));
    if !(from.is_finite() && to.is_finite()) || to <= from {
        return Err("a word's new end is to come after its new start".into());
    }
    let new = Retime {
        track,
        start: to_ms(start),
        nth,
        from,
        to,
    };
    let before = times.clone();
    let same = times.iter().position(|t| t.key() == new.key());
    let as_heard = from == to_ms(heard.0) && to == to_ms(heard.1);
    match (same, as_heard) {
        (Some(i), true) => {
            times.remove(i);
        }
        (Some(i), false) => {
            times[i].from = from;
            times[i].to = to;
        }
        (None, true) => {}
        (None, false) => {
            times.push(new);
            sort(times);
        }
    }
    Ok(*times != before)
}

/// The list's order: by heard start to the millisecond (as written), then track, then which word
/// of the millisecond: the same order before writing and after reading (2w repair: by the start's
/// last bits, two tracks' words of one millisecond swapped places on every round trip).
pub(super) fn sort(times: &mut [Retime]) {
    times.sort_by_key(|t| (millis(t.start), t.track, t.nth));
}

/// A `time` line's text after `time `.
pub(super) fn line(t: &Retime) -> String {
    let nth = if t.nth > 0 {
        format!("+{}", t.nth)
    } else {
        String::new()
    };
    format!(
        "{} {}{nth} {} {}",
        t.track,
        written(t.start),
        written(t.from),
        written(t.to)
    )
}

/// A list's overrides by word: every word's time as the user left it, read as the words come
/// ([`Self::of`] counts each word's `nth` itself, so it is given every word of the transcript in
/// heard order, as [`super::Fixes`] is: a stable sort by start of the segments' words, cut words
/// included).
pub struct Times {
    by_word: HashMap<Key, (f64, f64)>,
    /// Per track, the millisecond of the latest word read and how many of its words were read.
    seen: HashMap<usize, (i64, usize)>,
}

impl Times {
    pub fn new(times: &[Retime]) -> Self {
        Self {
            by_word: times.iter().map(|t| (t.key(), (t.from, t.to))).collect(),
            seen: HashMap::new(),
        }
    }

    /// No word is retimed: callers can skip the work.
    pub fn is_empty(&self) -> bool {
        self.by_word.is_empty()
    }

    /// Where the next word, `w` heard on `track`, starts and ends: its override, else as heard.
    pub fn of(&mut self, track: usize, w: &Word) -> (f64, f64) {
        if self.by_word.is_empty() {
            return (w.start, w.end);
        }
        let ms = millis(w.start);
        let seen = self.seen.entry(track).or_insert((ms, 0));
        if seen.0 == ms {
            seen.1 += 1;
        } else {
            *seen = (ms, 1);
        }
        let nth = seen.1 - 1;
        self.by_word
            .get(&(track, ms, nth))
            .copied()
            .unwrap_or((w.start, w.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, end: f64) -> Word {
        Word {
            start,
            end,
            text: "w".into(),
            p: 1.0,
            p_min: None,
        }
    }

    #[test]
    fn a_time_is_set_changed_and_taken_back_by_the_heard_time() {
        let mut t = Vec::new();
        assert!(set_time(&mut t, 0, 2.0, 0, (2.0, 2.4), (2.0, 2.61234)).unwrap());
        assert_eq!((t[0].from, t[0].to), (2.0, 2.612), "on the millisecond");
        assert!(!set_time(&mut t, 0, 2.0, 0, (2.0, 2.4), (2.0, 2.612)).unwrap());
        assert!(set_time(&mut t, 1, 1.0, 0, (1.0, 1.3), (0.9, 1.3)).unwrap());
        assert_eq!(t.iter().map(|x| x.track).collect::<Vec<_>>(), [1, 0]);
        assert!(set_time(&mut t, 0, 2.0, 0, (2.0, 2.4), (2.0, 2.7)).unwrap());
        assert_eq!(t.len(), 2, "changed in place");
        assert!(set_time(&mut t, 0, 2.0, 0, (2.0, 2.4), (2.0, 2.4)).unwrap());
        assert!(set_time(&mut t, 1, 1.0, 0, (1.0, 1.3), (1.0, 1.3)).unwrap());
        assert!(t.is_empty());
        assert!(set_time(&mut t, 0, 2.0, 0, (2.0, 2.4), (2.5, 2.5)).is_err());
        assert!(set_time(&mut t, 0, 2.0, 0, (2.0, 2.4), (2.0, 2.4)).is_ok_and(|c| !c));
        // A start off in its last bits is kept as the list writes it, so what is read back equals it.
        set_time(&mut t, 0, 300.0 + 12.34, 0, (312.34, 312.6), (312.3, 312.6)).unwrap();
        assert_eq!(t[0].start, 312.34);
    }

    #[test]
    fn two_tracks_of_one_millisecond_keep_their_order_through_the_list() {
        // 2w repair: sorted by the start's last bits, these swapped places on each round trip.
        let mut t = vec![
            Retime {
                track: 1,
                start: 5.0001,
                nth: 0,
                from: 5.0,
                to: 5.2,
            },
            Retime {
                track: 0,
                start: 5.0004,
                nth: 0,
                from: 5.0,
                to: 5.3,
            },
        ];
        sort(&mut t);
        assert_eq!(t.iter().map(|t| t.track).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn words_of_one_millisecond_are_told_apart_and_tracks_counted_apart() {
        // As fixes are (5w): "en" and "él" at one start, another track's word between.
        let t = vec![Retime {
            track: 0,
            start: 54.944,
            nth: 1,
            from: 54.95,
            to: 55.3,
        }];
        let mut times = Times::new(&t);
        let en = word(54.944, 54.944);
        let yes = word(54.944, 55.2);
        let el = word(54.9441, 55.0);
        assert_eq!(times.of(0, &en), (54.944, 54.944));
        assert_eq!(times.of(1, &yes), (54.944, 55.2));
        assert_eq!(times.of(0, &el), (54.95, 55.3));
        assert!(Times::new(&[]).is_empty());
    }
}
