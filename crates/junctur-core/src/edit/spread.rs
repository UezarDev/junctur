//! Spreads a fix to every word heard alike, and reads a transcript's words as fixes find them
//! (shared with `carry.rs`). Apart from `fixes.rs` because it works over a whole transcript, not
//! one word.

use std::collections::HashMap;

use super::fixes::{self, check_fix, check_span, Fix, Fixes, WordAt};
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
pub(super) struct Heard<'a> {
    pub(super) track: usize,
    pub(super) at: WordAt,
    pub(super) key: (i64, usize),
    pub(super) text: &'a str,
    pub(super) like: &'a str,
    pub(super) start: f64,
    pub(super) end: f64,
}

/// Each word's [`likeness`], in the stream's order.
pub(super) fn likes(words: &[(usize, &Word)]) -> Vec<String> {
    words.iter().map(|(_, w)| likeness(&w.text)).collect()
}

/// The words in [`Fixes`]' order with each one's place in its millisecond (`nth`), counted per track
/// as [`Fixes::of`] counts it; `likes` is [`likes`] of them.
pub(super) fn read<'a>(words: &[(usize, &'a Word)], likes: &'a [String]) -> Vec<Heard<'a>> {
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
pub(super) fn covered(fixes: &[Fix], words: &[(usize, &Word)]) -> Vec<bool> {
    let mut by = Fixes::new(fixes);
    words.iter().map(|&(t, w)| by.of(t, w).is_some()).collect()
}

/// Per track, the stream's places of its words, in time order.
pub(super) fn by_track(heard: &[Heard]) -> HashMap<usize, Vec<usize>> {
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
pub(super) fn words_of(
    f: &Fix,
    heard: &[Heard],
    tracks: &HashMap<usize, Vec<usize>>,
) -> Vec<usize> {
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
pub(super) enum Text {
    /// The fixed word in the other word's own punctuation: the fix kept the punctuation it was heard
    /// with ("Bane," → "Vayne,"), so "Bane." elsewhere becomes "Vayne.".
    Core(String),
    /// The text as typed.
    Typed(String),
}

impl Text {
    /// The rule a one-word fix from `heard` to `fixed` follows on another word.
    pub(super) fn of(heard: &str, fixed: &str) -> Text {
        if around(fixed) == around(heard) && !bare(fixed).is_empty() {
            Text::Core(bare(fixed).to_string())
        } else {
            Text::Typed(fixed.to_string())
        }
    }

    pub(super) fn on(&self, heard: &str) -> String {
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
    let rules = rules(&sources(fixes, which), &heard, tracks);
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
            let Some(fix) = fix_on(run, text, &heard) else {
                continue;
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

/// The fixes [`spread_with`] spreads from: every one, or the one `which` names.
fn sources(fixes: &[Fix], which: Which) -> Vec<&Fix> {
    match which {
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
    }
}

/// Each heard run (its words' likenesses) and what it becomes; `None` once two fixes disagree.
type Rules = Vec<(Vec<String>, Option<Text>)>;

/// The rules `sources` give on the words `heard`: a fix whose words are gone, or that only changes
/// the punctuation around what was heard, gives none.
fn rules(sources: &[&Fix], heard: &[Heard], tracks: &HashMap<usize, Vec<usize>>) -> Rules {
    let mut rules: Rules = Vec::new();
    for &f in sources {
        let at = words_of(f, heard, tracks);
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
    rules
}

/// `text` as a fix on the words `run` (one word, or a span of them); `None` when it would not be
/// a fix there (one a word cannot hold, or a word already saying it).
fn fix_on(run: &[usize], text: &Text, heard: &[Heard]) -> Option<Fix> {
    let (a, b) = (&heard[run[0]], &heard[*run.last().expect("not empty")]);
    if run.len() == 1 {
        let t = check_fix(&text.on(a.text)).ok()?;
        if t.is_empty() || t == check_fix(a.text).unwrap_or_default() {
            return None;
        }
        Some(Fix {
            track: a.track,
            start: a.at.start,
            nth: a.at.nth,
            through: None,
            text: t,
        })
    } else {
        let t = check_span(&text.on(a.text)).ok()?;
        Some(Fix {
            track: a.track,
            start: a.at.start,
            nth: a.at.nth,
            through: Some(b.at),
            text: t,
        })
    }
}

/// "Undo those": each fix of `added` (a [`spread`]'s, `heard` beside it) still as it was in
/// `fixes` is made to say what its words were heard as, a fix of the heard text. Not taken away: a
/// word the user kept as heard is fixed as far as a later spread goes (the Correct button's, or the
/// one after a re-transcription, which would otherwise give it the fix again), and [`carry`](super::carry) keeps it
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

/// A fix of the list where the transcript has it (5ae: the list the user takes wrong fixes out of).
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub fix: Fix,
    /// Its first and last words' places in the transcript; `None` when it is on no word of it now.
    pub words: Option<(usize, usize)>,
    /// Its words as heard, joined by spaces; empty when it is on no word.
    pub heard: String,
}

impl Placed {
    /// A fix of the heard text itself (Undo those, a removal, Looks right): the words read as heard.
    pub fn as_heard(&self) -> bool {
        !self.heard.is_empty() && check_span(&self.heard).ok() == check_span(&self.fix.text).ok()
    }
}

/// Every fix of `fixes` with the words of `words` (the transcript, in [`Fixes`]' order) it is on, in
/// the list's order.
pub fn placed(fixes: &[Fix], words: &[(usize, &Word)]) -> Vec<Placed> {
    let likes = likes(words);
    let heard = read(words, &likes);
    let tracks = by_track(&heard);
    fixes
        .iter()
        .map(|f| {
            let at = words_of(f, &heard, &tracks);
            Placed {
                fix: f.clone(),
                words: at.first().zip(at.last()).map(|(&a, &b)| (a, b)),
                heard: at
                    .iter()
                    .map(|&i| heard[i].text)
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        })
        .collect()
}

/// Takes the fixes of `gone` still in `fixes` as listed back to what their words of `words` were
/// heard as (5ae), as [`keep_heard`] does, so a later spread does not give them the wrong text again.
/// One on no word now, or whose heard text a fix may not hold (the parser would refuse the list), is
/// dropped: the words read as heard all the same. One changed since stays. How many went back.
pub fn remove(fixes: &mut Vec<Fix>, gone: &[Fix], words: &[(usize, &Word)]) -> usize {
    let ours: Vec<Fix> = fixes
        .iter()
        .filter(|f| gone.iter().any(|g| same(f, g)))
        .cloned()
        .collect();
    let mut n = 0;
    for p in placed(&ours, words) {
        let Some(i) = fixes.iter().position(|f| same(f, &p.fix)) else {
            continue;
        };
        let text = if p.fix.is_span() {
            check_span(&p.heard)
        } else {
            check_fix(&p.heard)
        };
        match text {
            Ok(t) if !t.is_empty() => fixes[i].text = t,
            _ => {
                fixes.remove(i);
            }
        }
        n += 1;
    }
    n
}

/// Both name the same words with the same text (a fix read back from the list may differ from the one
/// it was written from in the last bits of its start).
pub(super) fn same(a: &Fix, b: &Fix) -> bool {
    a.track == b.track && a.ends() == b.ends() && a.text == b.text
}

#[cfg(test)]
mod tests;
