//! Carries the user's fixes and retimes onto the words of a new transcription: each moved to the
//! word heard there again, and set aside where none is.

use std::collections::{HashMap, HashSet};

use super::fixes::{self, check_fix, check_span, Fix, Fixed, Fixes};
use super::spread::{by_track, covered, likeness, likes, read, same, words_of, Heard, Text};
use super::times::{self, set_time, to_ms, Retime};
use crate::transcribe::Word;

/// A fix as the transcript about to be replaced has it: the words it is on, as heard, and when
/// they were said.
#[derive(Debug, Clone, PartialEq)]
pub struct Carried {
    pub fix: Fix,
    /// Its words as heard, in order.
    pub heard: Vec<String>,
    /// From its first word's start to its last word's end, in source seconds.
    pub from: f64,
    pub to: f64,
}

/// Every fix of `fixes` whose words `words` (the transcript about to be replaced, in [`Fixes`]' order)
/// has, with those words: what [`carry`] moves onto the new transcript's words. A fix on no word of it
/// is not taken: it shows on nothing now, or it was made on the new words while they came in, and
/// [`carry`] leaves it as it is.
pub fn snapshot(fixes: &[Fix], words: &[(usize, &Word)]) -> Vec<Carried> {
    let likes = likes(words);
    let heard = read(words, &likes);
    let tracks = by_track(&heard);
    fixes
        .iter()
        .filter_map(|f| {
            let at = words_of(f, &heard, &tracks);
            let (first, last) = (at.first()?, at.last()?);
            Some(Carried {
                fix: f.clone(),
                heard: at.iter().map(|&i| heard[i].text.to_string()).collect(),
                from: heard[*first].start,
                to: at
                    .iter()
                    .map(|&i| heard[i].end)
                    .fold(heard[*last].end, f64::max),
            })
        })
        .collect()
}

/// How far around a word's old time [`carry`] looks for it among the new words, in seconds: a
/// re-transcription moves word starts by a few tenths, and now and then most of a second.
const NEAR: f64 = 1.0;

/// The longest a word lasts, in seconds, for [`carry`]'s search by start: Whisper's words are inside a
/// chunk of at most 28 s, and far shorter in practice.
const LONGEST: f64 = 30.0;

/// What [`carry`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CarryReport {
    /// Fixes now on their word in the new transcript.
    pub kept: usize,
    /// Fixes no longer needed: Whisper now hears the words as the fix says.
    pub right: usize,
    /// Fixes whose words the new transcript does not have, as they were: set aside by the caller
    /// (`fixes-set-aside.json`), never put on whatever word is there now.
    pub gone: Vec<Carried>,
}

/// Moves the fixes `carried` names, where the list still has them as they were, onto `words` (the new
/// transcript, in [`Fixes`]' order): a word fix to the nearest new word on its track within [`NEAR`]
/// that reads as heard or as the fix, a span over the new words whose middle falls in its old time.
/// One now heard as fixed goes; one with no words, over another fix or too long is set aside, never
/// put on whatever word is there now. A transcript with no words at all moves nothing.
pub fn carry(fixes: &mut Vec<Fix>, carried: &[Carried], words: &[(usize, &Word)]) -> CarryReport {
    let mut report = CarryReport::default();
    let likes = likes(words);
    let heard = read(words, &likes);
    if heard.is_empty() {
        return report;
    }
    let tracks = by_track(&heard);
    let ours: Vec<&Carried> = carried
        .iter()
        .filter(|c| fixes.iter().any(|f| same(f, &c.fix)))
        .collect();
    if ours.is_empty() {
        return report;
    }
    fixes.retain(|f| !ours.iter().any(|c| same(f, &c.fix)));
    let mut taken = covered(fixes, words);
    for c in ours {
        let gone = |r: &mut CarryReport| r.gone.push(c.clone());
        // A track the new transcript has no word on (heard as nothing this time): set aside like
        // any fix with no word to go to, not left on an old key where it shows on nothing.
        let Some(on) = tracks.get(&c.fix.track) else {
            gone(&mut report);
            continue;
        };
        // Where the new words near its old time are: the track's words are in time order, so a
        // binary search, not a scan of a 30 h track per fix under the edit list's writer lock.
        let near = {
            let a = on.partition_point(|&i| heard[i].start < c.from - NEAR - LONGEST);
            let b = on.partition_point(|&i| heard[i].start <= c.to + NEAR);
            &on[a..b.max(a)]
        };
        let landed = if c.fix.is_span() {
            carry_span(c, near, &heard, &mut taken, fixes)
        } else {
            carry_word(c, near, &heard, &mut taken, fixes)
        };
        match landed {
            Landed::Gone => gone(&mut report),
            Landed::Right => report.right += 1,
            Landed::Kept(new) => {
                fixes.push(new);
                report.kept += 1;
            }
        }
    }
    fixes::sort(fixes);
    report
}

/// Where one carried fix lands ([`carry_word`], [`carry_span`]).
enum Landed {
    /// No word to go to as it was: set aside.
    Gone,
    /// The new words are heard as the fix says: not needed.
    Right,
    /// The fix on the new words.
    Kept(Fix),
}

/// [`carry`] of the word fix `c` onto the nearest new word `near` holds that reads as heard or as
/// the fix; the word is marked `taken`.
fn carry_word(
    c: &Carried,
    near: &[usize],
    heard: &[Heard],
    taken: &mut [bool],
    fixes: &[Fix],
) -> Landed {
    let likes: HashSet<String> = [
        likeness(c.heard.first().map_or("", String::as_str)),
        likeness(&c.fix.text),
    ]
    .into_iter()
    .filter(|l| !l.is_empty())
    .collect();
    let best = near
        .iter()
        .copied()
        .filter(|&i| {
            let h = &heard[i];
            !taken[i] && h.end >= c.from - NEAR && h.start <= c.to + NEAR && likes.contains(h.like)
        })
        .min_by(|&a, &b| {
            (heard[a].start - c.from)
                .abs()
                .total_cmp(&(heard[b].start - c.from).abs())
        });
    let Some(i) = best else {
        return Landed::Gone;
    };
    let h = &heard[i];
    let kept = as_heard(c);
    // As a spread writes it: a fix that kept the punctuation it was heard with
    // takes the new word's own ("Vayne," onto a "Bane." heard now).
    let text = if kept {
        // As heard now, which a fix may not always hold (a word over `MAX_FIX`).
        match check_fix(h.text) {
            Ok(t) if !t.is_empty() => t,
            _ => return Landed::Gone,
        }
    } else {
        let was = c.heard.first().map_or("", String::as_str);
        match check_fix(&Text::of(was, &c.fix.text).on(h.text)) {
            Ok(t) if !t.is_empty() => t,
            _ => c.fix.text.clone(),
        }
    };
    let new = Fix {
        track: h.track,
        start: h.at.start,
        nth: h.at.nth,
        through: None,
        text,
    };
    if fixes.iter().any(|f| f.overlaps(&new)) {
        return Landed::Gone;
    }
    taken[i] = true;
    if h.text == new.text && !kept {
        return Landed::Right;
    }
    Landed::Kept(new)
}

/// [`carry`] of the span `c` onto the same words heard again near where they were, else whatever
/// `near` has said in its time; those words are marked `taken`.
fn carry_span(
    c: &Carried,
    near: &[usize],
    heard: &[Heard],
    taken: &mut [bool],
    fixes: &[Fix],
) -> Landed {
    // The same words heard again near where they were, else whatever is said in its time.
    let inside = again(c, near, heard, taken).unwrap_or_else(|| {
        near.iter()
            .copied()
            .filter(|&i| {
                let mid = (heard[i].start + heard[i].end) / 2.0;
                mid >= c.from && mid <= c.to
            })
            .collect()
    });
    if inside.is_empty() || inside.iter().any(|&i| taken[i]) {
        return Landed::Gone;
    }
    let said: Vec<&str> = inside.iter().map(|&i| heard[i].text).collect();
    let kept = as_heard(c);
    if check_span(&said.join(" ")).ok() == check_span(&c.fix.text).ok() && !kept {
        for &i in &inside {
            taken[i] = true;
        }
        return Landed::Right;
    }
    // Kept as heard: the new words as said, which can be longer than a span's text may be
    // (past `MAX_SPAN` the list would not read back). Set aside then.
    let text = if kept {
        match check_span(&said.join(" ")) {
            Ok(t) if !t.is_empty() => t,
            _ => return Landed::Gone,
        }
    } else {
        c.fix.text.clone()
    };
    let (a, b) = (
        &heard[inside[0]],
        &heard[*inside.last().expect("not empty")],
    );
    let through = (inside.len() > 1).then_some(b.at);
    // A sentence now heard as one word is not made a one-word fix of several words: a
    // spread would put that sentence on every word heard like it. Set aside.
    if through.is_none() && (text.split_whitespace().count() > 1 || check_fix(&text).is_err()) {
        return Landed::Gone;
    }
    let new = Fix {
        track: a.track,
        start: a.at.start,
        nth: a.at.nth,
        through,
        text,
    };
    if fixes.iter().any(|f| f.overlaps(&new)) {
        return Landed::Gone;
    }
    for &i in &inside {
        taken[i] = true;
    }
    Landed::Kept(new)
}

/// A retimed word as the transcript about to be replaced has it: what [`carry_times`]
/// moves onto the new transcript's words, as [`carry`] moves the fixes.
#[derive(Debug, Clone, PartialEq)]
pub struct CarriedTime {
    pub time: Retime,
    /// Its word as heard.
    pub heard: String,
    /// The word fix on it, if any: the new word may be heard as that.
    pub fixed: Option<String>,
    /// Its start and end as heard, in source seconds.
    pub start: f64,
    pub end: f64,
}

/// Every retime of `times` whose word `words` (the transcript about to be replaced, in [`Fixes`]'
/// order) has, with that word and the word fix on it (`fixes` as they are on that transcript). A
/// retime on no word of it stays where it is ([`carry_times`]), as such a fix does.
pub fn snapshot_times(
    times: &[Retime],
    fixes: &[Fix],
    words: &[(usize, &Word)],
) -> Vec<CarriedTime> {
    if times.is_empty() {
        return Vec::new();
    }
    let by_key: HashMap<(usize, i64, usize), &Retime> = times
        .iter()
        .map(|t| ((t.track, fixes::millis(t.start), t.nth), t))
        .collect();
    let mut by = Fixes::new(fixes);
    let likes = likes(words);
    read(words, &likes)
        .iter()
        .zip(words)
        .filter_map(|(h, &(track, w))| {
            // Every word read, in order: `Fixes::of` counts each one's place in its millisecond.
            let fixed = match by.of(track, w) {
                Some(Fixed::Word(t)) => Some(t.to_string()),
                _ => None,
            };
            let t = by_key.get(&(h.track, h.key.0, h.key.1))?;
            Some(CarriedTime {
                time: **t,
                heard: h.text.to_string(),
                fixed,
                start: h.start,
                end: h.end,
            })
        })
        .collect()
}

/// What [`carry_times`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimesReport {
    /// Retimes now on their word in the new transcript.
    pub kept: usize,
    /// Retimes no longer needed: the new word is heard with the times the user set.
    pub right: usize,
    /// Retimes whose word the new transcript does not have: set aside by the caller, as fixes are.
    pub gone: Vec<CarriedTime>,
}

/// Moves the retimes `carried` names, where the list still has them as they were, onto `words` (the
/// new transcript, in [`Fixes`]' order), as [`carry`] moves a word fix: an old key would name another
/// word. The side the user moved stays at the cut's edge, the other is the new word's own. No such
/// word, or the sides crossing: set aside. The new word heard just so: not needed.
pub fn carry_times(
    times: &mut Vec<Retime>,
    carried: &[CarriedTime],
    words: &[(usize, &Word)],
) -> TimesReport {
    let mut report = TimesReport::default();
    let likes = likes(words);
    let heard = read(words, &likes);
    if heard.is_empty() {
        return report;
    }
    let tracks = by_track(&heard);
    let ours: Vec<&CarriedTime> = carried.iter().filter(|c| times.contains(&c.time)).collect();
    if ours.is_empty() {
        return report;
    }
    times.retain(|t| !ours.iter().any(|c| c.time == *t));
    let key = |h: &Heard| (h.track, h.key.0, h.key.1);
    let mut taken: HashSet<(usize, i64, usize)> = times
        .iter()
        .map(|t| (t.track, fixes::millis(t.start), t.nth))
        .collect();
    for c in ours {
        let gone = |r: &mut TimesReport| r.gone.push(c.clone());
        let Some(on) = tracks.get(&c.time.track) else {
            gone(&mut report);
            continue;
        };
        let likes: HashSet<String> = [Some(c.heard.as_str()), c.fixed.as_deref()]
            .into_iter()
            .flatten()
            .map(likeness)
            .filter(|l| !l.is_empty())
            .collect();
        let a = on.partition_point(|&i| heard[i].start < c.start - NEAR);
        let b = on.partition_point(|&i| heard[i].start <= c.start + NEAR);
        let best = on[a..b.max(a)]
            .iter()
            .copied()
            .filter(|&i| !taken.contains(&key(&heard[i])) && likes.contains(heard[i].like))
            .min_by(|&x, &y| {
                (heard[x].start - c.start)
                    .abs()
                    .total_cmp(&(heard[y].start - c.start).abs())
            });
        let Some(i) = best else {
            gone(&mut report);
            continue;
        };
        let h = &heard[i];
        let from = if c.time.from == to_ms(c.start) {
            h.start
        } else {
            c.time.from
        };
        let to = if c.time.to == to_ms(c.end) {
            h.end
        } else {
            c.time.to
        };
        let mut one = Vec::new();
        if set_time(
            &mut one,
            h.track,
            h.start,
            h.at.nth,
            (h.start, h.end),
            (from, to),
        )
        .is_err()
        {
            gone(&mut report);
            continue;
        }
        taken.insert(key(h));
        if one.is_empty() {
            report.right += 1;
        } else {
            times.extend(one);
            report.kept += 1;
        }
    }
    times::sort(times);
    report
}

/// A span's words heard again: the run of new words on its track, none of them fixed, whose
/// likenesses are its old words', starting within [`NEAR`] of where they did, the nearest such run.
fn again(c: &Carried, on: &[usize], heard: &[Heard], taken: &[bool]) -> Option<Vec<usize>> {
    let like: Vec<String> = c.heard.iter().map(|h| likeness(h)).collect();
    (0..on.len())
        .filter(|&k| (heard[on[k]].start - c.from).abs() <= NEAR)
        .filter_map(|k| {
            let run = on.get(k..k + like.len())?;
            let alike = run
                .iter()
                .zip(&like)
                .all(|(&i, l)| !taken[i] && heard[i].like == *l);
            alike.then(|| run.to_vec())
        })
        .min_by(|a, b| {
            (heard[a[0]].start - c.from)
                .abs()
                .total_cmp(&(heard[b[0]].start - c.from).abs())
        })
}

/// A fix of the heard text itself ([`keep_heard`](super::keep_heard)): the words are right as heard.
fn as_heard(c: &Carried) -> bool {
    check_span(&c.heard.join(" ")).ok() == check_span(&c.fix.text).ok()
}
