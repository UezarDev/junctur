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

use super::fixes::{self, check_fix, check_span, Fix, Fixes, WordAt};
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
    like: String,
    start: f64,
    end: f64,
}

/// The words in [`Fixes`]' order with each one's place in its millisecond (`nth`), counted per track
/// as [`Fixes::of`] counts it.
fn read<'a>(words: &[(usize, &'a Word)]) -> Vec<Heard<'a>> {
    let mut seen: HashMap<usize, (i64, usize)> = HashMap::new();
    words
        .iter()
        .map(|&(track, w)| {
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
                like: likeness(&w.text),
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
    let mut out: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, h) in heard.iter().enumerate() {
        out.entry(h.track).or_default().push(i);
    }
    out
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
    let mut out = Spread::default();
    if fixes.is_empty() || words.is_empty() {
        return out;
    }
    let mut added: Vec<(Fix, String)> = Vec::new();
    let heard = read(words);
    let tracks = by_track(&heard);
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
        let at = words_of(f, &heard, &tracks);
        if at.is_empty() {
            continue;
        }
        let like: Vec<String> = at.iter().map(|&i| heard[i].like.clone()).collect();
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
    // Where each likeness is heard, for the first word of a run.
    let mut starts: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, h) in heard.iter().enumerate() {
        if !h.like.is_empty() {
            starts.entry(h.like.as_str()).or_default().push(i);
        }
    }
    // Each word's place in its track's list, to read the words after it.
    let mut place = vec![0usize; heard.len()];
    for on in tracks.values() {
        for (k, &i) in on.iter().enumerate() {
            place[i] = k;
        }
    }
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
    let heard = read(words);
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
/// fix covers.
pub fn carry(fixes: &mut Vec<Fix>, carried: &[Carried], words: &[(usize, &Word)]) -> CarryReport {
    let mut report = CarryReport::default();
    let heard = read(words);
    let tracks = by_track(&heard);
    // A track the new transcript has no word on (heard as nothing, or not transcribed) keeps its
    // fixes as they are: there is nothing to move them onto, and nothing they could land on wrongly.
    let ours: Vec<&Carried> = carried
        .iter()
        .filter(|c| tracks.contains_key(&c.fix.track))
        .filter(|c| fixes.iter().any(|f| same(f, &c.fix)))
        .collect();
    if ours.is_empty() {
        return report;
    }
    fixes.retain(|f| !ours.iter().any(|c| same(f, &c.fix)));
    let mut taken = covered(fixes, words);
    for c in ours {
        let on = &tracks[&c.fix.track];
        let gone = |r: &mut CarryReport| r.gone.push(c.clone());
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
                        && likes.contains(&h.like)
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
                h.text.to_string()
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
            let text = if kept {
                said.join(" ")
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
        // set aside.
        assert_eq!(r.gone.len(), 1);
        assert_eq!(r.gone[0].fix.text, "Uezar went");
        // The track with no words keeps its fix as it was.
        assert_eq!(f.len(), 2);
        assert_eq!(
            (f[1].track, f[1].start, f[1].text.as_str()),
            (1, 5.0, "Hola,")
        );
        assert!(!super::super::fixes::clash(&f));
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
