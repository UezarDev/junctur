//! Spreads a fix to every word heard alike, and carries fixes onto the words of a new transcription.
//! Apart from `fixes.rs` because it works over a whole transcript, not one word.

use std::collections::{HashMap, HashSet};

use super::fixes::{self, check_fix, check_span, Fix, Fixed, Fixes, WordAt};
use super::times::{self, set_time, to_ms, Retime};
use crate::text::{bare, split_bare};
use crate::transcribe::Word;

/// A word's text as two hearings of the same word compare: trimmed of the punctuation around it,
/// lower-cased. "Bane," and "bane" are alike; "Bane" and "Vayne" are not.
pub fn likeness(text: &str) -> String {
    bare(text).to_lowercase()
}

/// `s`'s punctuation before and after its word.
fn around(s: &str) -> (&str, &str) {
    let (lead, _, trail) = split_bare(s.trim());
    (lead, trail)
}

/// One word of the stream, as the scans need it.
struct Heard<'a> {
    track: usize,
    at: WordAt,
    key: (i64, usize),
    text: &'a str,
    like: &'a str,
    start: f64,
    end: f64,
}

/// Each word's [`likeness`], in the stream's order.
fn likes(words: &[(usize, &Word)]) -> Vec<String> {
    words.iter().map(|(_, w)| likeness(&w.text)).collect()
}

/// The words in [`Fixes`]' order with each one's place in its millisecond (`nth`), counted per track
/// as [`Fixes::of`] counts it; `likes` is [`likes`] of them.
fn read<'a>(words: &[(usize, &'a Word)], likes: &'a [String]) -> Vec<Heard<'a>> {
    let mut seen: HashMap<usize, (i64, usize)> = HashMap::new();
    words
        .iter()
        .zip(likes)
        .map(|(&(track, w), like)| {
            let ms = (w.start * 1000.0).round() as i64;
            let s = seen.entry(track).or_insert((ms, 0));
            if s.0 == ms {
                s.1 += 1;
            } else {
                *s = (ms, 1);
            }
            let nth = s.1 - 1;
            Heard {
                track,
                at: WordAt {
                    start: w.start,
                    nth,
                },
                key: (ms, nth),
                text: w.text.trim(),
                like,
                start: w.start,
                end: w.end,
            }
        })
        .collect()
}

/// Which words a fix of `fixes` already covers, word by word.
fn covered(fixes: &[Fix], words: &[(usize, &Word)]) -> Vec<bool> {
    let mut by = Fixes::new(fixes);
    words.iter().map(|&(t, w)| by.of(t, w).is_some()).collect()
}

/// Per track, the stream's places of its words, in time order.
fn by_track(heard: &[Heard]) -> HashMap<usize, Vec<usize>> {
    tracks_of(heard.iter().map(|h| h.track))
}

/// [`by_track`] of the words' tracks, in the stream's order.
fn tracks_of(tracks: impl Iterator<Item = usize>) -> HashMap<usize, Vec<usize>> {
    let mut out: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, t) in tracks.enumerate() {
        out.entry(t).or_default().push(i);
    }
    out
}

/// What [`spread`] reads of a transcript that the fixes do not change: each word's likeness, its
/// track's words, its place among them and where each likeness is heard. Built once per transcript
/// ([`SpreadIndex::new`]) by a caller that spreads fix after fix over the same words.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpreadIndex {
    likes: Vec<String>,
    tracks: HashMap<usize, Vec<usize>>,
    place: Vec<usize>,
    starts: HashMap<String, Vec<usize>>,
}

impl SpreadIndex {
    /// The index of `words`, the transcript in [`Fixes`]' order.
    pub fn new(words: &[(usize, &Word)]) -> Self {
        let likes = likes(words);
        let tracks = tracks_of(words.iter().map(|&(t, _)| t));
        // Each word's place in its track's list, to read the words after it.
        let mut place = vec![0usize; words.len()];
        for on in tracks.values() {
            for (k, &i) in on.iter().enumerate() {
                place[i] = k;
            }
        }
        // Where each likeness is heard, for the first word of a run.
        let mut starts: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, l) in likes.iter().enumerate() {
            if !l.is_empty() {
                starts.entry(l.clone()).or_default().push(i);
            }
        }
        Self {
            likes,
            tracks,
            place,
            starts,
        }
    }

    /// Whether this can be the index of `words`: as many words. Which words is the caller's to
    /// keep (the app keeps the index beside the words it was built from).
    fn fits(&self, words: &[(usize, &Word)]) -> bool {
        let fits = self.likes.len() == words.len() && self.place.len() == words.len();
        // The words' tracks, at least, must be the indexed ones.
        debug_assert!(
            !fits
                || words.iter().enumerate().all(|(i, &(t, _))| self
                    .tracks
                    .get(&t)
                    .and_then(|on| on.get(self.place[i]))
                    == Some(&i)),
            "a spread index of other words"
        );
        fits
    }
}

/// The stream's places of the words `f` names, on its track: one for a word fix, its words for a span.
fn words_of(f: &Fix, heard: &[Heard], tracks: &HashMap<usize, Vec<usize>>) -> Vec<usize> {
    let first = (fixes::millis(f.start), f.nth);
    let last = f.through.map_or(first, |t| (fixes::millis(t.start), t.nth));
    let Some(on) = tracks.get(&f.track) else {
        return Vec::new();
    };
    let from = on.partition_point(|&i| heard[i].key < first);
    on[from..]
        .iter()
        .copied()
        .take_while(|&i| heard[i].key <= last)
        .collect()
}

/// What a source fix writes on another word.
#[derive(Debug, Clone, PartialEq)]
enum Text {
    /// The fixed word in the other word's own punctuation: the fix kept the punctuation it was heard
    /// with ("Bane," → "Vayne,"), so "Bane." elsewhere becomes "Vayne.".
    Core(String),
    /// The text as typed.
    Typed(String),
}

impl Text {
    /// The rule a one-word fix from `heard` to `fixed` follows on another word.
    fn of(heard: &str, fixed: &str) -> Text {
        if around(fixed) == around(heard) && !bare(fixed).is_empty() {
            Text::Core(bare(fixed).to_string())
        } else {
            Text::Typed(fixed.to_string())
        }
    }

    fn on(&self, heard: &str) -> String {
        match self {
            Text::Core(core) => {
                let (lead, trail) = around(heard);
                format!("{lead}{core}{trail}")
            }
            Text::Typed(t) => t.clone(),
        }
    }
}

/// Which fixes [`spread`] spreads.
#[derive(Debug, Clone, Copy)]
pub enum Which {
    /// Every fix in the list (the Correct button, a re-transcription's end).
    All,
    /// The fix on `first..=last` of `track`, just made by hand.
    Words {
        track: usize,
        first: WordAt,
        last: WordAt,
    },
}

/// What [`spread`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spread {
    /// The fixes it added, in the list's order: what the app's "Undo those" takes back
    /// ([`keep_heard`]).
    pub added: Vec<Fix>,
    /// What each of `added`'s words were heard as, joined by spaces: what "Undo those" leaves them
    /// saying. Kept beside the fixes because by then the editor may hold other words (a
    /// re-transcription running).
    pub heard: Vec<String>,
    /// Heard texts two fixes disagree on (`Which::All`): left as heard, for the caller to say.
    pub unclear: Vec<String>,
}

/// Gives every other run of words heard like a fix's (each word's [`likeness`], on any track) the same
/// fix, when none of those words is fixed already: the user's own fixes win. `words` is the transcript
/// in [`Fixes`]' order. Not spread: a fix that only changes the punctuation around what was heard (every
/// "so" made "so," is not a mishearing), a fix whose words are gone, and (with [`Which::All`]) a heard
/// text two fixes disagree on, which comes back as [`Spread::unclear`]. Kept in the list's order.
pub fn spread(fixes: &mut Vec<Fix>, words: &[(usize, &Word)], which: Which) -> Spread {
    if fixes.is_empty() || words.is_empty() {
        return Spread::default();
    }
    spread_with(fixes, words, &SpreadIndex::new(words), which)
}

/// [`spread`] with `index`, which must be [`SpreadIndex::new`] of these `words` (one of another
/// length is not used: the index is built again).
pub fn spread_with(
    fixes: &mut Vec<Fix>,
    words: &[(usize, &Word)],
    index: &SpreadIndex,
    which: Which,
) -> Spread {
    let mut out = Spread::default();
    if fixes.is_empty() || words.is_empty() {
        return out;
    }
    if !index.fits(words) {
        return spread(fixes, words, which);
    }
    let mut added: Vec<(Fix, String)> = Vec::new();
    let heard = read(words, &index.likes);
    let tracks = &index.tracks;
    let mut taken = covered(fixes, words);
    let sources: Vec<&Fix> = match which {
        Which::All => fixes.iter().collect(),
        Which::Words { track, first, last } => {
            let ends = (
                (fixes::millis(first.start), first.nth),
                (fixes::millis(last.start), last.nth),
            );
            fixes
                .iter()
                .filter(|f| f.track == track && f.ends() == ends)
                .collect()
        }
    };
    // Each heard run (its words' likenesses) and what it becomes; `None` once two fixes disagree.
    let mut rules: Vec<(Vec<String>, Option<Text>)> = Vec::new();
    for f in sources {
        let at = words_of(f, &heard, tracks);
        if at.is_empty() {
            continue;
        }
        let like: Vec<String> = at.iter().map(|&i| heard[i].like.to_string()).collect();
        if like.iter().all(String::is_empty) {
            continue;
        }
        let text = if at.len() == 1 && !f.is_span() {
            let was = heard[at[0]].text;
            if bare(&f.text) == bare(was) {
                // Only the punctuation around it changed.
                continue;
            }
            Text::of(was, &f.text)
        } else {
            let said: Vec<&str> = at.iter().map(|&i| bare(heard[i].text)).collect();
            let typed: Vec<&str> = f.text.split_whitespace().map(bare).collect();
            if said == typed {
                continue;
            }
            Text::Typed(f.text.clone())
        };
        match rules.iter_mut().find(|(l, _)| *l == like) {
            Some((_, t)) => {
                if t.as_ref().is_some_and(|t| *t != text) {
                    *t = None;
                }
            }
            None => rules.push((like, Some(text))),
        }
    }
    let (starts, place) = (&index.starts, &index.place);
    for (like, text) in &rules {
        let Some(text) = text else {
            out.unclear.push(like.join(" "));
            continue;
        };
        let Some(firsts) = starts.get(like[0].as_str()) else {
            continue;
        };
        for &i in firsts {
            let on = &tracks[&heard[i].track];
            let run = &on[place[i]..];
            if run.len() < like.len() {
                continue;
            }
            let run = &run[..like.len()];
            if run.iter().any(|&j| taken[j])
                || run.iter().zip(like).any(|(&j, l)| heard[j].like != *l)
            {
                continue;
            }
            let (a, b) = (&heard[run[0]], &heard[*run.last().expect("not empty")]);
            let fix = if run.len() == 1 {
                let Ok(t) = check_fix(&text.on(a.text)) else {
                    continue;
                };
                if t.is_empty() || t == check_fix(a.text).unwrap_or_default() {
                    continue;
                }
                Fix {
                    track: a.track,
                    start: a.at.start,
                    nth: a.at.nth,
                    through: None,
                    text: t,
                }
            } else {
                let Ok(t) = check_span(&text.on(a.text)) else {
                    continue;
                };
                Fix {
                    track: a.track,
                    start: a.at.start,
                    nth: a.at.nth,
                    through: Some(b.at),
                    text: t,
                }
            };
            // A fix whose words the transcript no longer has (left in the list, shown on no word)
            // can still lie inside this run's keys; the list would not read back with both.
            if fixes
                .iter()
                .chain(added.iter().map(|(f, _)| f))
                .any(|f| f.overlaps(&fix))
            {
                continue;
            }
            for &j in run {
                taken[j] = true;
            }
            let said: Vec<&str> = run.iter().map(|&j| heard[j].text).collect();
            added.push((fix, said.join(" ")));
        }
    }
    added.sort_by(|a, b| {
        a.0.start
            .total_cmp(&b.0.start)
            .then(a.0.track.cmp(&b.0.track))
            .then(a.0.nth.cmp(&b.0.nth))
    });
    (out.added, out.heard) = added.into_iter().unzip();
    fixes.extend(out.added.iter().cloned());
    fixes::sort(fixes);
    out
}

/// "Undo those": each fix of `added` (a [`spread`]'s, `heard` beside it) still as it was in
/// `fixes` is made to say what its words were heard as, a fix of the heard text. Not taken away: a
/// word the user kept as heard is fixed as far as a later spread goes (the Correct button's, or the
/// one after a re-transcription, which would otherwise give it the fix again), and [`carry`] keeps it
/// on its word. One the user changed or took away since is theirs and stays as it is. How many.
pub fn keep_heard(fixes: &mut [Fix], added: &[Fix], heard: &[String]) -> usize {
    let mut n = 0;
    for (a, h) in added.iter().zip(heard) {
        let text = if a.is_span() {
            check_span(h)
        } else {
            check_fix(h)
        };
        let Ok(text) = text else {
            continue;
        };
        if let Some(f) = fixes.iter_mut().find(|f| same(f, a)) {
            f.text = text;
            n += 1;
        }
    }
    n
}

/// A fix of the heard text itself ([`keep_heard`]): the words are right as heard.
fn as_heard(c: &Carried) -> bool {
    check_span(&c.heard.join(" ")).ok() == check_span(&c.fix.text).ok()
}

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
        if !c.fix.is_span() {
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
                    !taken[i]
                        && h.end >= c.from - NEAR
                        && h.start <= c.to + NEAR
                        && likes.contains(h.like)
                })
                .min_by(|&a, &b| {
                    (heard[a].start - c.from)
                        .abs()
                        .total_cmp(&(heard[b].start - c.from).abs())
                });
            let Some(i) = best else {
                gone(&mut report);
                continue;
            };
            let h = &heard[i];
            let kept = as_heard(c);
            // As a spread writes it: a fix that kept the punctuation it was heard with
            // takes the new word's own ("Vayne," onto a "Bane." heard now).
            let text = if kept {
                // As heard now, which a fix may not always hold (a word over `MAX_FIX`).
                match check_fix(h.text) {
                    Ok(t) if !t.is_empty() => t,
                    _ => {
                        gone(&mut report);
                        continue;
                    }
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
                gone(&mut report);
                continue;
            }
            taken[i] = true;
            if h.text == new.text && !kept {
                report.right += 1;
                continue;
            }
            fixes.push(new);
            report.kept += 1;
        } else {
            // The same words heard again near where they were, else whatever is said in its time.
            let inside = again(c, near, &heard, &taken).unwrap_or_else(|| {
                near.iter()
                    .copied()
                    .filter(|&i| {
                        let mid = (heard[i].start + heard[i].end) / 2.0;
                        mid >= c.from && mid <= c.to
                    })
                    .collect()
            });
            if inside.is_empty() || inside.iter().any(|&i| taken[i]) {
                gone(&mut report);
                continue;
            }
            let said: Vec<&str> = inside.iter().map(|&i| heard[i].text).collect();
            let kept = as_heard(c);
            if check_span(&said.join(" ")).ok() == check_span(&c.fix.text).ok() && !kept {
                for &i in &inside {
                    taken[i] = true;
                }
                report.right += 1;
                continue;
            }
            // Kept as heard: the new words as said, which can be longer than a span's text may be
            // (past `MAX_SPAN` the list would not read back). Set aside then.
            let text = if kept {
                match check_span(&said.join(" ")) {
                    Ok(t) if !t.is_empty() => t,
                    _ => {
                        gone(&mut report);
                        continue;
                    }
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
            if through.is_none()
                && (text.split_whitespace().count() > 1 || check_fix(&text).is_err())
            {
                gone(&mut report);
                continue;
            }
            let new = Fix {
                track: a.track,
                start: a.at.start,
                nth: a.at.nth,
                through,
                text,
            };
            if fixes.iter().any(|f| f.overlaps(&new)) {
                gone(&mut report);
                continue;
            }
            for &i in &inside {
                taken[i] = true;
            }
            fixes.push(new);
            report.kept += 1;
        }
    }
    fixes::sort(fixes);
    report
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

/// Both name the same words with the same text (a fix read back from the list may differ from the one
/// it was written from in the last bits of its start).
fn same(a: &Fix, b: &Fix) -> bool {
    a.track == b.track && a.ends() == b.ends() && a.text == b.text
}

#[cfg(test)]
mod tests;
