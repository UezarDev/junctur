//! A fix reaching every word heard alike, and following its word into a new transcription (5aa).
//!
//! Uezar fixed one "Bane" to "Vayne" and the other "Bane" stayed: a fix is one word ([`super::fixes`]),
//! and nothing looked for the words Whisper heard the same way. [`spread`] does: every other run of
//! words whose heard text is the fixed one's, case and the punctuation around each word aside, gets the
//! same fix, as an ordinary `fix` or `span` line the user can take away on its own. [`carry`] moves the
//! fixes onto the words of a new transcription of the recording (the Correct button hears it again with
//! the word list), where the old keys name other words or none.
//!
//! A module of its own rather than more of `fixes.rs`: that is what a fix is and how one word reads it;
//! this is work over a whole transcript (an index of what every word sounds like, a scan per track),
//! used by the app on every fix and by the pipeline after a re-transcription.

use std::collections::{HashMap, HashSet};

use super::fixes::{self, check_fix, check_span, Fix, Fixed, Fixes, WordAt};
use super::times::{self, set_time, to_ms, Retime};
use crate::transcribe::Word;

/// A word's text as two hearings of the same word compare: trimmed of the punctuation around it,
/// lower-cased. "Bane," and "bane" are alike; "Bane" and "Vayne" are not.
pub fn likeness(text: &str) -> String {
    bare(text).to_lowercase()
}

/// `s` without the punctuation around it.
fn bare(s: &str) -> &str {
    s.trim_matches(|c: char| !c.is_alphanumeric())
}

/// `s`'s punctuation before and after its word.
fn around(s: &str) -> (&str, &str) {
    let s = s.trim();
    let b = bare(s);
    if b.is_empty() {
        return (s, "");
    }
    let at = s.find(b).unwrap_or(0);
    (&s[..at], &s[at + b.len()..])
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
        self.likes.len() == words.len() && self.place.len() == words.len()
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
            // A fix whose words the transcript no longer has (5h: left in the list, shown on no word)
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

/// "Undo those" (5aa): each fix of `added` (a [`spread`]'s, `heard` beside it) still as it was in
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

/// A fix as the transcript about to be replaced has it (5aa): the words it is on, as heard, and when
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
/// is not taken: it shows on nothing now, or it was made on the new words while they came in (2j: the
/// editor shows a re-transcription's words as they come), and [`carry`] leaves it as it is.
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
/// re-transcription moves word starts by a few tenths, and now and then most of a second (on
/// Uezar's recording, heard again with the word list: a "Bane" from 104.76 s to 103.94 s, "para el
/// reto." 0.4 s earlier).
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
/// transcript, in [`Fixes`]' order). A word fix goes to the nearest new word on its track within
/// [`NEAR`] of its old time that reads as the heard word or as the fix; a span over the new words on its
/// track whose middle falls in its old time. When those words now say exactly what the fix says, the fix
/// is not needed and goes; when there are none, it is dropped (the report names it) rather than put on
/// whatever word is there now. A fix of the heard text ([`keep_heard`]) stays on its word as it now
/// reads. A fix not in `carried` stays as it is, and a carried fix is never put over a word another
/// fix covers, nor written with a text a fix may not have (5ad). A transcript with no words at all
/// moves nothing: that run heard nothing, and the caller says so.
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
        // any fix with no word to go to (5ad). 5aa's repair left these on their old keys, where
        // they showed on nothing and were never listed.
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
            // As a spread writes it (5aa repair): a fix that kept the punctuation it was heard with
            // takes the new word's own ("Vayne," onto a "Bane." heard now).
            let text = if kept {
                // As heard now, which a fix may not always hold (5ad: a word over `MAX_FIX`).
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
            // (5ad: past `MAX_SPAN` the list would not have read back). Set aside then.
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
            // spread would put that sentence on every word heard like it (5aa repair). Set aside.
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

/// A retimed word (2w) as the transcript about to be replaced has it (5ad): what [`carry_times`]
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
/// new transcript, in [`Fixes`]' order), as [`carry`] moves a word fix (5ad: a re-transcription left
/// every `time` line on its old key, so a word the user had pulled out of a cut was halved by it
/// again, or another word starting in that millisecond took its time). The word: the nearest new word
/// on its track within [`NEAR`] of its old start that reads as it was heard or as its fix, none
/// another retime is on. The side the user moved (a start or end that was not Whisper's) stays where
/// they put it, at the cut's edge; the other side is the new word's own. No such word, or the two
/// sides crossing: set aside. The new word heard just so: not needed. A retime not in `carried` stays
/// as it is, and a transcript with no words at all moves nothing.
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

/// A span's words heard again (5aa): the run of new words on its track, none of them fixed, whose
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
mod tests {
    use super::*;

    fn word(start: f64, text: &str) -> Word {
        Word {
            start,
            end: start + 0.3,
            text: text.into(),
            p: 1.0,
            p_min: None,
        }
    }

    fn fix(track: usize, start: f64, text: &str) -> Fix {
        Fix {
            track,
            start,
            nth: 0,
            through: None,
            text: text.into(),
        }
    }

    fn at(start: f64, nth: usize) -> WordAt {
        WordAt { start, nth }
    }

    /// What `ws` show, each heard on its track: a span's text once.
    fn shown(fixes: &[Fix], ws: &[(usize, Word)]) -> Vec<String> {
        let mut by = Fixes::new(fixes);
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for (t, w) in ws {
            match by.of(*t, w) {
                Some(super::super::Fixed::Span { index, text }) => {
                    if !seen.contains(&index) {
                        seen.push(index);
                        out.push(text.to_string());
                    }
                }
                Some(super::super::Fixed::Word(t)) => out.push(t.to_string()),
                None => out.push(w.text.trim().to_string()),
            }
        }
        out
    }

    fn refs(ws: &[(usize, Word)]) -> Vec<(usize, &Word)> {
        ws.iter().map(|(t, w)| (*t, w)).collect()
    }

    /// Uezar's recording: "Bane" heard twice (once with a comma), and once on the other track.
    fn banes() -> Vec<(usize, Word)> {
        vec![
            (0, word(1.0, " Bane,")),
            (0, word(1.5, " is")),
            (1, word(2.0, " bane")),
            (0, word(3.0, " Bane.")),
            (0, word(4.0, " Baneful")),
        ]
    }

    #[test]
    fn a_fix_spreads_to_every_word_heard_alike_in_its_own_punctuation() {
        let ws = banes();
        let mut f = vec![fix(0, 1.0, "Vayne,")];
        let s = spread(
            &mut f,
            &refs(&ws),
            Which::Words {
                track: 0,
                first: at(1.0, 0),
                last: at(1.0, 0),
            },
        );
        assert_eq!(
            shown(&f, &ws),
            ["Vayne,", "is", "Vayne", "Vayne.", "Baneful"]
        );
        assert_eq!(s.added.len(), 2, "{s:?}");
        assert_eq!((s.added[0].track, s.added[1].track), (1, 0));
        assert!(s.unclear.is_empty());
        // Spread again: nothing more, every "Bane" is fixed.
        assert_eq!(spread(&mut f, &refs(&ws), Which::All), Spread::default());
        assert_eq!(s.heard, ["bane", "Bane."]);
        // "Undo those": the others say what was heard again, the fix made by hand stays, and one
        // changed since stays as changed.
        f[1].text = "Vain".into();
        assert_eq!(keep_heard(&mut f, &s.added, &s.heard), 1);
        assert_eq!(shown(&f, &ws), ["Vayne,", "is", "Vain", "Bane.", "Baneful"]);
        // Kept as heard is kept: spreading every fix again does not reach it.
        f[1].text = "bane".into();
        assert_eq!(spread(&mut f, &refs(&ws), Which::All), Spread::default());
        assert_eq!(shown(&f, &ws), ["Vayne,", "is", "bane", "Bane.", "Baneful"]);
    }

    #[test]
    fn one_index_kept_for_fix_after_fix_spreads_as_a_fresh_one_does() {
        let mut ws = banes();
        ws.extend([
            (1, word(6.0, " so")),
            (1, word(6.0, " Bane")),
            (0, word(7.0, " big")),
            (0, word(7.5, " bane")),
            (1, word(8.0, " big")),
            (1, word(8.5, " Bane!")),
            (0, word(9.0, "")),
        ]);
        let words = refs(&ws);
        let index = SpreadIndex::new(&words);
        assert_eq!(index, SpreadIndex::new(&words), "built the same each time");
        let one = |track, start, nth| Which::Words {
            track,
            first: at(start, nth),
            last: at(start, nth),
        };
        let steps = [
            (fix(0, 1.0, "Vayne,"), one(0, 1.0, 0)),
            (fix(0, 7.0, "pig"), one(0, 7.0, 0)),
            (fix(0, 4.0, "Painful"), one(0, 4.0, 0)),
            (fix(0, 1.5, "is"), Which::All),
        ];
        let (mut fresh, mut kept) = (Vec::new(), Vec::new());
        for (f, which) in steps {
            fresh.push(f.clone());
            kept.push(f);
            fixes::sort(&mut fresh);
            fixes::sort(&mut kept);
            let a = spread(&mut fresh, &words, which);
            let b = spread_with(&mut kept, &words, &index, which);
            assert_eq!(a, b);
            assert_eq!(fresh, kept);
        }
        assert!(fresh.len() > 4, "the fixes spread: {fresh:?}");
        // An index of other words (fewer of them) is not used.
        let fewer = refs(&ws[..3]);
        let mut f = vec![fix(0, 1.0, "Vayne,")];
        let mut g = f.clone();
        assert_eq!(
            spread_with(&mut f, &fewer, &index, Which::All),
            spread(&mut g, &fewer, Which::All)
        );
        assert_eq!(f, g);
    }

    #[test]
    fn a_word_already_fixed_keeps_its_own_fix_and_a_typed_text_goes_as_typed() {
        let ws = banes();
        // The second "Bane" was fixed by hand to something else: it stays.
        let mut f = vec![fix(0, 1.0, "Vayne!"), fix(0, 3.0, "Bain.")];
        let s = spread(
            &mut f,
            &refs(&ws),
            Which::Words {
                track: 0,
                first: at(1.0, 0),
                last: at(1.0, 0),
            },
        );
        // "Vayne!" changed the punctuation it was heard with: written as typed.
        assert_eq!(
            shown(&f, &ws),
            ["Vayne!", "is", "Vayne!", "Bain.", "Baneful"]
        );
        assert_eq!(s.added.len(), 1);
        // From every fix, the two disagree: no more spreading, and the heard word is unclear.
        let mut more = ws.clone();
        more.push((0, word(5.0, " BANE")));
        let mut g = vec![fix(0, 1.0, "Vayne,"), fix(0, 3.0, "Bain.")];
        let s = spread(&mut g, &refs(&more), Which::All);
        assert!(s.added.is_empty(), "{s:?}");
        assert_eq!(s.unclear, ["bane"]);
    }

    #[test]
    fn a_punctuation_only_fix_or_a_case_fix_spreads_as_it_should() {
        let ws = vec![
            (0, word(1.0, " so")),
            (0, word(2.0, " so")),
            (0, word(3.0, " tsunheto")),
            (0, word(4.0, " Tsunheto?")),
            (0, word(5.0, " tsunheto")),
        ];
        // "so" made "so," is not a mishearing: not spread.
        let mut f = vec![fix(0, 1.0, "so,")];
        assert!(spread(&mut f, &refs(&ws), Which::All).added.is_empty());
        // A name heard in lower case: its case spreads, the other word's punctuation kept.
        let mut f = vec![fix(0, 3.0, "Tsunheto")];
        let s = spread(&mut f, &refs(&ws), Which::All);
        assert_eq!(
            shown(&f, &ws),
            ["so", "so", "Tsunheto", "Tsunheto?", "Tsunheto"]
        );
        // "Tsunheto?" already reads as the fix: no fix needed there.
        assert_eq!(s.added.len(), 1, "{s:?}");
    }

    #[test]
    fn a_span_spreads_over_the_same_words_on_any_track_never_over_a_fixed_one() {
        let ws = vec![
            (0, word(1.0, " the")),
            (0, word(1.3, " wezar")),
            (0, word(1.6, " went")),
            (0, word(2.0, " The")),
            (0, word(2.3, " Wezar")),
            (0, word(2.6, " went.")),
            (1, word(3.0, " the")),
            (1, word(3.3, " wezar")),
            (1, word(3.6, " went")),
            (0, word(4.0, " the")),
            (0, word(4.3, " wezar")),
            (0, word(4.6, " sat")),
            (0, word(5.0, " the")),
            (0, word(5.3, " wezar")),
            (0, word(5.6, " went")),
        ];
        let mut f = vec![
            Fix {
                track: 0,
                start: 1.0,
                nth: 0,
                through: Some(at(1.6, 0)),
                text: "Uezar went".into(),
            },
            fix(0, 5.6, "left"),
        ];
        let s = spread(&mut f, &refs(&ws), Which::All);
        assert_eq!(
            shown(&f, &ws),
            [
                "Uezar went",
                "Uezar went",
                "Uezar went",
                "the",
                "wezar",
                "sat",
                "the",
                "wezar",
                "left"
            ]
        );
        assert_eq!(s.added.len(), 2, "{s:?}");
        assert!(s.added.iter().all(Fix::is_span));
        // A retyped sentence that only moved punctuation is not spread.
        let mut g = vec![Fix {
            track: 0,
            start: 1.0,
            nth: 0,
            through: Some(at(1.6, 0)),
            text: "the wezar, went.".into(),
        }];
        assert!(spread(&mut g, &refs(&ws), Which::All).added.is_empty());
    }

    #[test]
    fn a_fix_follows_its_word_into_a_new_transcription() {
        let old = vec![
            (0, word(1.0, " Bane,")),
            (0, word(2.0, " tsunheto")),
            (0, word(3.0, " ok")),
            (0, word(4.0, " the")),
            (0, word(4.3, " wezar")),
            (0, word(6.0, " Mel")),
            (0, word(8.0, " hom")),
        ];
        let mut f = vec![
            fix(0, 1.0, "Vayne,"),
            fix(0, 2.0, "Tsunheto"),
            fix(0, 3.0, "okay"),
            Fix {
                track: 0,
                start: 4.0,
                nth: 0,
                through: Some(at(4.3, 0)),
                text: "Uezar".into(),
            },
            fix(0, 6.0, "Mell"),
            fix(0, 8.0, "home"),
        ];
        let carried = snapshot(&f, &refs(&old));
        assert_eq!(carried.len(), 6);
        assert_eq!(carried[0].heard, ["Bane,"]);
        // While it ran, the user fixed a new word on their own (at 8.1, where "hom" was): the old
        // fix of that word does not go over it.
        f.push(fix(0, 8.1, "house"));
        let new = vec![
            // Moved a little, still misheard: the fix moves with it.
            (0, word(1.1, " Bane,")),
            // Heard right now, but in lower case: the case fix stays.
            (0, word(2.0, " tsunheto")),
            // Heard as the fix: not needed.
            (0, word(3.05, "okay")),
            // The span's words came out as one word: a word fix.
            (0, word(4.1, " thewezar")),
            // Something else entirely where "Mel" was: dropped.
            (0, word(6.0, " and")),
            (0, word(8.1, " hom")),
        ];
        let r = carry(&mut f, &carried, &refs(&new));
        assert_eq!(r.kept, 3, "{r:?}");
        assert_eq!(r.right, 1);
        let gone: Vec<(String, f64)> = r.gone.iter().map(|g| (g.heard.join(" "), g.from)).collect();
        assert_eq!(gone, [("Mel".to_string(), 6.0), ("hom".to_string(), 8.0)]);
        assert_eq!(r.gone[0].fix.text, "Mell", "set aside as it was");
        assert_eq!(
            shown(&f, &new),
            ["Vayne,", "Tsunheto", "okay", "Uezar", "and", "house"]
        );
        assert_eq!(f.iter().filter(|x| x.start == 1.1).count(), 1);
        // Carried again onto the same words: nothing moves (a resumed run's end).
        let again = snapshot(&f, &refs(&new));
        let before = f.clone();
        carry(&mut f, &again, &refs(&new));
        assert_eq!(f, before);
    }

    #[test]
    fn a_spread_never_lays_a_fix_over_one_whose_word_is_gone() {
        // 5aa repair: a fix left on no word (5h: a word re-transcribed away) inside the keys a
        // spread span would cover; with both the list would not read back.
        let ws = vec![
            (0, word(1.0, " the")),
            (0, word(1.3, " wezar")),
            (0, word(2.0, " the")),
            (0, word(2.3, " wezar")),
        ];
        let mut f = vec![
            Fix {
                track: 0,
                start: 1.0,
                nth: 0,
                through: Some(at(1.3, 0)),
                text: "Uezar".into(),
            },
            fix(0, 2.15, "stale"),
        ];
        let s = spread(&mut f, &refs(&ws), Which::All);
        assert!(s.added.is_empty(), "{s:?}");
        assert!(!super::super::fixes::clash(&f));
        // A word fix next to it still spreads.
        let mut g = vec![fix(0, 1.3, "Uezar"), fix(0, 2.15, "stale")];
        assert_eq!(spread(&mut g, &refs(&ws), Which::All).added.len(), 1);
        assert!(!super::super::fixes::clash(&g));
    }

    #[test]
    fn a_carried_fix_takes_its_new_words_punctuation_and_keeps_its_shape() {
        // 5aa repair: "Vayne," onto a "Bane." heard now is "Vayne.", as a spread would write it.
        let old = vec![
            (0, word(1.0, " Bane,")),
            (0, word(3.0, " the")),
            (0, word(3.3, " wezar")),
            (1, word(5.0, " hola")),
        ];
        let mut f = vec![
            fix(0, 1.0, "Vayne,"),
            Fix {
                track: 0,
                start: 3.0,
                nth: 0,
                through: Some(at(3.3, 0)),
                text: "Uezar went".into(),
            },
            fix(1, 5.0, "Hola,"),
        ];
        let carried = snapshot(&f, &refs(&old));
        // Track 1 heard nothing this time; the sentence came out as one word.
        let new = vec![(0, word(1.0, " Bane.")), (0, word(3.1, " thewezar"))];
        let r = carry(&mut f, &carried, &refs(&new));
        assert_eq!(f[0].text, "Vayne.");
        // A sentence is not made a one-word fix (a spread would copy it onto every such word):
        // set aside. And the track with no words sets its fix aside too (5ad: it was left on its
        // old key, shown on nothing and never listed).
        let gone: Vec<&str> = r.gone.iter().map(|g| g.fix.text.as_str()).collect();
        assert_eq!(gone, ["Uezar went", "Hola,"]);
        assert_eq!(f.len(), 1);
        assert!(!super::super::fixes::clash(&f));
        // A transcript with no words at all (a run that heard nothing) moves nothing.
        let mut g = vec![fix(1, 5.0, "Hola,")];
        let carried = snapshot(&g, &refs(&old));
        assert_eq!(carry(&mut g, &carried, &[]), CarryReport::default());
        assert_eq!(g, [fix(1, 5.0, "Hola,")]);
    }

    #[test]
    fn a_fix_kept_as_heard_is_never_carried_longer_than_a_fix_may_be() {
        // 5ad: a span kept as heard took the new words as said, unchecked; past `MAX_SPAN` the list
        // would not read back. Likewise a word heard now longer than `MAX_FIX`. Set aside.
        let run = |w: &str| -> Vec<(usize, Word)> {
            (0..100)
                .map(|i| (0, word(1.0 + i as f64 * 0.1, w)))
                .collect()
        };
        let old = run(" palabra");
        let said: Vec<&str> = old.iter().map(|(_, w)| w.text.trim()).collect();
        let mut f = vec![Fix {
            track: 0,
            start: 1.0,
            nth: 0,
            through: Some(at(1.0 + 99.0 * 0.1, 0)),
            text: said.join(" "),
        }];
        assert!(f[0].text.chars().count() <= super::super::MAX_SPAN);
        let carried = snapshot(&f, &refs(&old));
        // Heard again as longer words in the same time: as said now it is over a span's limit.
        let r = carry(&mut f, &carried, &refs(&run(" palabrotas")));
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(r.gone.len(), 1);
        // One word kept as heard, now heard with a run of punctuation past a word fix's limit.
        let mut g = vec![fix(0, 1.0, "Bane")];
        let carried = snapshot(&g, &refs(&[(0, word(1.0, " Bane"))]));
        let loud = format!(" Bane{}", "!".repeat(super::super::MAX_FIX));
        let r = carry(&mut g, &carried, &refs(&[(0, word(1.0, &loud))]));
        assert_eq!(r.gone.len(), 1, "{r:?}");
        assert!(g.is_empty());
    }

    #[test]
    fn a_retimed_word_follows_its_word_into_a_new_transcription() {
        // 5ad: a word pulled out of a cut by moving the cut's edge (2w) keeps that edge after
        // Correct: its `time` line moves onto the new word, the side the user moved kept.
        let old = vec![
            (0, word(1.0, " Bane,")),
            (0, word(2.0, " said")),
            (0, word(4.0, " Mel")),
            (1, word(6.0, " hola")),
        ];
        let rt = |track: usize, start: f64, from: f64, to: f64| Retime {
            track,
            start,
            nth: 0,
            from,
            to,
        };
        let timed = |start: f64, end: f64, text: &str| Word {
            end,
            ..word(start, text)
        };
        // "Bane," ended at 1.3 as heard, the edge put it at 1.5; "said" started at 2.0, the edge
        // put it at 1.9, its end Whisper's; "Mel" and "hola" are retimed too.
        let mut t = vec![
            rt(0, 1.0, 1.0, 1.5),
            rt(0, 2.0, 1.9, 2.3),
            rt(0, 4.0, 4.0, 4.6),
            rt(1, 6.0, 6.0, 6.5),
        ];
        // A fix on "Bane,": the new word may be heard as the fix.
        let f = vec![fix(0, 1.0, "Vayne,")];
        let carried = snapshot_times(&t, &f, &refs(&old));
        assert_eq!(carried.len(), 4);
        assert_eq!(carried[0].fixed.as_deref(), Some("Vayne,"));
        // While it ran, the user retimed a new word on their own: it stays.
        t.push(rt(0, 8.0, 8.0, 8.9));
        let new = vec![
            // Heard as the fix now, a little later: the user's end kept, the start the new one.
            (0, timed(1.1, 1.35, " Vayne,")),
            // Heard from the edge already, its end as before: not needed.
            (0, timed(1.9, 2.3, " said")),
            // Something else where "Mel" was: set aside. Track 1 heard nothing: set aside.
            (0, word(4.0, " and")),
            (0, word(8.0, " bien")),
        ];
        let r = carry_times(&mut t, &carried, &refs(&new));
        assert_eq!((r.kept, r.right), (1, 1), "{r:?}");
        let gone: Vec<&str> = r.gone.iter().map(|g| g.heard.as_str()).collect();
        assert_eq!(gone, ["Mel", "hola"]);
        assert_eq!(t, [rt(0, 1.1, 1.1, 1.5), rt(0, 8.0, 8.0, 8.9)]);
        // Read where the words are: "Vayne," ends where the edge is.
        let mut times = super::super::Times::new(&t);
        assert_eq!(times.of(0, &new[0].1), (1.1, 1.5));
        // Carried again onto the same words: nothing moves.
        let again = snapshot_times(&t, &f, &refs(&new));
        let before = t.clone();
        carry_times(&mut t, &again, &refs(&new));
        assert_eq!(t, before);
        // The user's end at 1.5, the new word now starting at 1.6: nothing of it left before the
        // edge, set aside rather than written backwards.
        let mut v = vec![rt(0, 1.0, 1.0, 1.5)];
        let c = snapshot_times(&v, &[], &refs(&old));
        let r = carry_times(&mut v, &c, &refs(&[(0, timed(1.6, 1.9, " Bane,"))]));
        assert_eq!(r.gone.len(), 1);
        assert!(v.is_empty());
    }

    #[test]
    fn a_word_kept_as_heard_stays_so_after_a_new_transcription() {
        // 5aa: "Undo those" left the second "nuevo" as heard (Uezar's recording: one "nuevo" was a
        // mishearing of "gordo", the other was right). A new transcription keeps that on its word,
        // as it now reads, so the spread after it does not reach it.
        let old = vec![(0, word(1.0, " nuevo")), (0, word(4.0, " nuevo"))];
        let mut f = vec![fix(0, 1.0, "gordo"), fix(0, 4.0, "nuevo")];
        let carried = snapshot(&f, &refs(&old));
        let new = vec![(0, word(1.0, " nuevo")), (0, word(4.1, " nuevo,"))];
        let r = carry(&mut f, &carried, &refs(&new));
        assert_eq!((r.kept, r.right), (2, 0));
        assert_eq!(f[1].text, "nuevo,");
        assert!(spread(&mut f, &refs(&new), Which::All).added.is_empty());
        assert_eq!(shown(&f, &new), ["gordo", "nuevo,"]);
    }

    #[test]
    fn a_span_is_laid_over_the_new_words_said_in_its_time() {
        let old = vec![
            (0, word(1.0, " so")),
            (0, word(2.0, " the")),
            (0, word(2.4, " wezar")),
            (0, word(2.8, " went")),
            (0, word(4.0, " ok")),
        ];
        let span = Fix {
            track: 0,
            start: 2.0,
            nth: 0,
            through: Some(at(2.8, 0)),
            text: "Uezar went home.".into(),
        };
        let mut f = vec![span.clone()];
        let carried = snapshot(&f, &refs(&old));
        assert_eq!(carried[0].from, 2.0);
        assert!((carried[0].to - 3.1).abs() < 1e-9, "{}", carried[0].to);
        let new = vec![
            (0, word(1.0, " so")),
            (0, word(2.05, " the")),
            (0, word(2.3, " wezar")),
            (0, word(2.5, " uh")),
            (0, word(2.75, " went")),
            (0, word(4.0, " ok")),
        ];
        let r = carry(&mut f, &carried, &refs(&new));
        assert_eq!(r.kept, 1);
        assert_eq!(shown(&f, &new), ["so", "Uezar went home.", "ok"]);
        assert_eq!(f[0].through, Some(at(2.75, 0)));
        // The same words heard again, earlier than its old time: carried onto them.
        let mut f = vec![span.clone()];
        let earlier = vec![
            (0, word(1.0, " so")),
            (0, word(1.5, " the")),
            (0, word(1.7, " wezar")),
            (0, word(1.9, " went")),
            (0, word(4.0, " ok")),
        ];
        let r = carry(&mut f, &carried, &refs(&earlier));
        assert_eq!(r.kept, 1);
        assert_eq!((f[0].start, f[0].through), (1.5, Some(at(1.9, 0))));
        // Said as the span now: not needed.
        let mut f = vec![span];
        let right = vec![
            (0, word(2.0, " Uezar")),
            (0, word(2.4, " went")),
            (0, word(2.8, " home.")),
        ];
        let r = carry(&mut f, &carried, &refs(&right));
        assert_eq!((r.right, f.len()), (1, 0));
    }
}
