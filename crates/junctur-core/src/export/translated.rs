//! Translated captions on the kept words: the words are grouped into sentences, each sentence's
//! translation looked up ([`crate::translate::Translations`]) and its words spread over the time
//! the sentence was said by character position, so each end lands in a kept word. Runs before the
//! text options and the cut, since a cue ends at the line limit rather than at a sentence's end.

/// A word as captions show it, between two points of source time, each held in the kept range
/// `(range, seconds)` it falls in.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Shown {
    pub track: usize,
    pub start: (usize, f64),
    pub end: (usize, f64),
    pub text: String,
}

/// A sentence ends after a word ending in one of these (closing quotes and brackets aside).
const ENDS: &[char] = &['.', '?', '!', '…', '。', '？', '！'];
const CLOSING: &[char] = &['"', '\'', '”', '’', '»', ')', ']'];
/// The most words of one sentence: a speaker who never stops still gets sentences the model reads
/// whole (a caption sentence is a few dozen tokens), and its timing stays local.
pub(super) const MOST_WORDS: usize = 40;

/// Whether `text` ends a sentence.
fn ends(text: &str) -> bool {
    text.trim_end_matches(CLOSING).ends_with(ENDS)
}

/// The sentences of `shown` (time order), as indices into it: one speaker's words on one track,
/// until a word ends a sentence, the speaker changes, a pause of `pause` output seconds, or
/// [`MOST_WORDS`]. `speaker` and `out` give a word's speaker and output time of a point. A word
/// with no text joins its track's open sentence (or is one of its own) and has no say in where
/// sentences end, so it cannot merge two sentences, hide a full stop or split one.
pub(super) fn sentences(
    shown: &[Shown],
    speaker: impl Fn(&Shown) -> u32,
    out: impl Fn((usize, f64)) -> f64,
    pause: f64,
) -> Vec<Vec<usize>> {
    let mut done: Vec<Vec<usize>> = Vec::new();
    // Open sentence per track: its words and its speaker. It opens on a word with text.
    let mut open: Vec<(usize, u32, Vec<usize>)> = Vec::new();
    for (i, w) in shown.iter().enumerate() {
        if w.text.is_empty() {
            match open.iter_mut().find(|(t, _, _)| *t == w.track) {
                Some((_, _, words)) => words.push(i),
                None => done.push(vec![i]),
            }
            continue;
        }
        let who = speaker(w);
        let at = open.iter().position(|(t, _, _)| *t == w.track);
        if let Some(k) = at {
            let (_, s, words) = &open[k];
            let mut said = words
                .iter()
                .map(|&j| &shown[j])
                .filter(|w| !w.text.is_empty());
            let count = said.clone().count();
            let last = said.next_back().expect("opened on a word with text");
            if *s != who
                || out(w.start) - out(last.end) >= pause
                || ends(&last.text)
                || count >= MOST_WORDS
            {
                done.push(open.remove(k).2);
            }
        }
        match open.iter_mut().find(|(t, _, _)| *t == w.track) {
            Some((_, _, words)) => words.push(i),
            None => open.push((w.track, who, vec![i])),
        }
    }
    done.extend(open.into_iter().map(|(_, _, w)| w));
    done.sort_by_key(|s| s[0]);
    done
}

/// A sentence's text as captions show it untranslated: the key its translation is kept under.
pub(super) fn text(shown: &[Shown], sentence: &[usize]) -> String {
    sentence
        .iter()
        .map(|&i| shown[i].text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `shown` with each sentence that has a translation (`lookup` of its [`text`]) replaced by the
/// translation's words, in time order. A sentence without one keeps its words as said.
pub(super) fn apply<'a>(
    shown: Vec<Shown>,
    sentences: &[Vec<usize>],
    lookup: impl Fn(&str) -> Option<&'a str>,
    clean: impl Fn(&str) -> String,
) -> Vec<Shown> {
    let mut out: Vec<Shown> = Vec::with_capacity(shown.len());
    for s in sentences {
        let words: Vec<String> = lookup(&text(&shown, s))
            .map(|t| clean(t).split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        // Words left empty by the cleaning take no place, as in the sentence's text.
        let spoken: Vec<&Shown> = s
            .iter()
            .map(|&i| &shown[i])
            .filter(|w| !w.text.is_empty())
            .collect();
        if words.is_empty() || spoken.is_empty() {
            out.extend(s.iter().map(|&i| shown[i].clone()));
        } else {
            out.extend(spread(&spoken, words));
        }
    }
    // Sentences of two tracks interleave in time; within one, words are already in order.
    out.sort_by(|a, b| a.start.1.total_cmp(&b.start.1));
    out
}

/// `words` (the translation) placed over the `spoken` words of their sentence, by character.
fn spread(spoken: &[&Shown], words: Vec<String>) -> Vec<Shown> {
    let len = |s: &str| s.chars().count().max(1) as f64;
    // Where each spoken word starts in the sentence's characters, a space between words.
    let mut at = Vec::with_capacity(spoken.len());
    let mut total = 0.0;
    for w in spoken {
        at.push(total);
        total += len(&w.text) + 1.0;
    }
    let total = total - 1.0;
    // The point `p` characters in: inside a word, a share of its time; in the space after it, a
    // share of the pause to the next word, so extra translated words do not bunch at one instant.
    // A pause across a cut has no time: the word's end. `ending` takes the word before a point
    // exactly at a word's start, so a word ending there ends with it or in the pause after it.
    let point = |p: f64, ending: bool| -> (usize, f64) {
        let k = at
            .iter()
            .rposition(|&a| if ending { a < p } else { a <= p })
            .unwrap_or(0);
        let w = spoken[k];
        let into = p - at[k];
        let l = len(&w.text);
        if into > l {
            if let Some(n) = spoken.get(k + 1).filter(|n| n.start.0 == w.end.0) {
                let share = (into - l).min(1.0);
                return (w.end.0, w.end.1 + share * (n.start.1 - w.end.1).max(0.0));
            }
        }
        let share = (into / l).clamp(0.0, 1.0);
        // A spoken word lies in one kept range (it is kept by its middle).
        (w.start.0, w.start.1 + share * (w.end.1 - w.start.1))
    };
    let shown_total: f64 = words.iter().map(|w| len(w) + 1.0).sum::<f64>() - 1.0;
    let track = spoken[0].track;
    let mut from = 0.0;
    words
        .into_iter()
        .map(|text| {
            let to = from + len(&text);
            let w = Shown {
                track,
                start: point(from / shown_total * total, false),
                end: point(to / shown_total * total, true),
                text,
            };
            from = to + 1.0;
            w
        })
        .collect()
}

#[cfg(test)]
mod tests;
