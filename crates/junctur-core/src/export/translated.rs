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
mod tests {
    use super::*;

    fn w(track: usize, start: f64, end: f64, text: &str) -> Shown {
        Shown {
            track,
            start: (0, start),
            end: (0, end),
            text: text.into(),
        }
    }

    fn texts(shown: &[Shown], s: &[Vec<usize>]) -> Vec<String> {
        s.iter().map(|s| text(shown, s)).collect()
    }

    #[test]
    fn sentences_end_at_marks_pauses_speakers_and_the_word_limit() {
        let shown = vec![
            w(0, 0.0, 0.3, "Hola,"),
            w(0, 0.3, 0.6, "¿qué"),
            w(0, 0.6, 0.9, "tal?"),
            w(0, 1.0, 1.3, "Bien"),
            // Another track talks at once: its own sentence, the first one goes on.
            w(1, 1.1, 1.4, "Yo"),
            w(0, 1.3, 1.6, "\"gracias.\""),
            w(1, 1.4, 1.7, "también"),
            // A pause ends a sentence without a mark.
            w(0, 3.0, 3.3, "Luego"),
        ];
        let s = sentences(&shown, |_| 1, |p| p.1, 1.0);
        assert_eq!(
            texts(&shown, &s),
            [
                "Hola, ¿qué tal?",
                "Bien \"gracias.\"",
                "Yo también",
                "Luego"
            ]
        );
        // A change of speaker on one track.
        let s = sentences(&shown, |w| u32::from(w.start.1 >= 0.5), |p| p.1, 1.0);
        assert_eq!(texts(&shown, &s)[..2], ["Hola, ¿qué", "tal?"]);
        // A run with no end at all is cut every MOST_WORDS words.
        let long: Vec<Shown> = (0..100)
            .map(|i| w(0, i as f64 * 0.2, i as f64 * 0.2 + 0.1, "la"))
            .collect();
        let s = sentences(&long, |_| 1, |p| p.1, 1.0);
        assert_eq!(
            s.iter().map(Vec::len).collect::<Vec<_>>(),
            [MOST_WORDS, MOST_WORDS, 100 - 2 * MOST_WORDS]
        );
        assert!(sentences(&[], |_| 1, |p| p.1, 1.0).is_empty());
    }

    #[test]
    fn an_empty_word_neither_joins_nor_splits_sentences() {
        // A word cleaning left empty (a fix or a Whisper word of tags only) stays with the words
        // around it but plays no part in where sentences end.
        let n = |shown: &[Shown]| sentences(shown, |_| 1, |p| p.1, 1.0);
        // 0.6 s on either side of it, 1.4 s between the words with text: a pause.
        let pause = vec![
            w(0, 0.0, 0.4, "uno"),
            w(0, 1.0, 1.2, ""),
            w(0, 1.8, 2.0, "dos"),
        ];
        assert_eq!(n(&pause), [vec![0, 1], vec![2]]);
        // A full stop before it still ends the sentence.
        let stop = vec![
            w(0, 0.0, 0.4, "Fin."),
            w(0, 0.5, 0.6, ""),
            w(0, 0.7, 1.0, "Otra"),
        ];
        assert_eq!(texts(&stop, &n(&stop)), ["Fin.", "Otra"]);
        // The speaker its time falls to does not close the sentence; the next word's still does.
        let three = vec![w(0, 0.0, 0.4, "a"), w(0, 0.4, 0.5, ""), w(0, 0.5, 0.9, "b")];
        let s = sentences(&three, |w| u32::from(w.text.is_empty()) + 1, |p| p.1, 1.0);
        assert_eq!(s, [vec![0, 1, 2]]);
        let s = sentences(&three, |w| u32::from(w.start.1 >= 0.45) + 1, |p| p.1, 1.0);
        assert_eq!(s, [vec![0, 1], vec![2]]);
        // Empty words do not count toward MOST_WORDS.
        let long: Vec<Shown> = (0..MOST_WORDS * 2)
            .map(|i| {
                let t = i as f64 * 0.1;
                w(0, t, t + 0.05, if i % 2 == 0 { "la" } else { "" })
            })
            .collect();
        assert_eq!(n(&long).len(), 1);
        // One with no sentence open on its track is a sentence of its own, with no text.
        let alone = vec![w(0, 0.0, 0.1, ""), w(0, 0.2, 0.5, "hola")];
        assert_eq!(n(&alone), [vec![0], vec![1]]);
        assert_eq!(texts(&alone, &n(&alone)), ["", "hola"]);
    }

    #[test]
    fn a_translation_is_spread_over_its_sentence_by_character() {
        // "aaaa bbbb" said 0..1 and 2..3: 9 characters.
        let spoken = vec![w(0, 0.0, 1.0, "aaaa"), w(0, 2.0, 3.0, "bbbb")];
        let s = vec![vec![0, 1]];
        let tr = |t: &str| (t == "aaaa bbbb").then_some("xx yy zz");
        let got = apply(spoken.clone(), &s, tr, |t| t.to_string());
        let texts: Vec<&str> = got.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["xx", "yy", "zz"]);
        // The first starts with the sentence, the last ends with it, in order between.
        assert_eq!(got[0].start, (0, 0.0));
        assert_eq!(got[2].end, (0, 3.0));
        for p in got.windows(2) {
            assert!(p[0].end.1 <= p[1].start.1 && p[0].start.1 < p[0].end.1);
        }
        // "yy" is the middle of the translation, across the space between the spoken words: it
        // starts in the first and ends in the second.
        assert!(got[1].start.1 < 1.0 && got[1].end.1 > 2.0, "{got:?}");
        // No translation, or an empty one, keeps the words as said.
        assert_eq!(
            apply(spoken.clone(), &s, |_| None, |t| t.to_string()),
            spoken
        );
        assert_eq!(
            apply(spoken.clone(), &s, |_| Some("  "), |t| t.to_string()),
            spoken
        );
    }

    #[test]
    fn translated_words_keep_their_spoken_words_range_across_a_cut() {
        // The sentence's words sit in two kept ranges: every translated end lies in one of them.
        let spoken = vec![
            Shown {
                track: 0,
                start: (0, 0.0),
                end: (0, 1.0),
                text: "uno".into(),
            },
            Shown {
                track: 0,
                start: (1, 5.0),
                end: (1, 6.0),
                text: "dos".into(),
            },
        ];
        let got = apply(
            spoken,
            &[vec![0, 1]],
            |_| Some("one two three four"),
            |t| t.to_string(),
        );
        for w in &got {
            for (r, t) in [w.start, w.end] {
                assert!(
                    (r == 0 && (0.0..=1.0).contains(&t)) || (r == 1 && (5.0..=6.0).contains(&t)),
                    "{w:?}"
                );
            }
        }
        assert_eq!((got[0].start.0, got[3].end.0), (0, 1));
    }

    #[test]
    fn extra_words_spread_over_a_pause_and_empty_words_take_no_place() {
        // "aa" 0..1, a pause, "bb" 3..4; an empty word between takes no place. Twelve
        // translated words: some fall in the space between, and spread over the pause rather
        // than all starting at 1.0.
        let spoken = vec![
            w(0, 0.0, 1.0, "aa"),
            w(0, 1.0, 1.0, ""),
            w(0, 3.0, 4.0, "bb"),
        ];
        let s = vec![vec![0, 1, 2]];
        let many = "a b c d e f g h i j k l";
        let got = apply(spoken, &s, |_| Some(many), |t| t.to_string());
        assert_eq!(got.len(), 12);
        let in_pause: Vec<f64> = got
            .iter()
            .map(|w| w.start.1)
            .filter(|&t| t > 1.0 && t < 3.0)
            .collect();
        assert!(!in_pause.is_empty(), "{got:?}");
        for p in got.windows(2) {
            assert!(
                p[0].start.1 <= p[1].start.1 && p[0].start.1 <= p[0].end.1,
                "{got:?}"
            );
        }
        // Touching words leave no pause: the word's end.
        let touching = vec![w(0, 0.0, 1.0, "aa"), w(0, 1.0, 2.0, "bb")];
        let got = apply(touching, &[vec![0, 1]], |_| Some(many), |t| t.to_string());
        assert!(got.iter().all(|w| (0.0..=2.0).contains(&w.start.1)));
    }

    #[test]
    fn two_tracks_sentences_interleave_in_time() {
        let spoken = vec![
            w(0, 0.0, 1.0, "a."),
            w(1, 0.5, 1.5, "b."),
            w(0, 2.0, 3.0, "c."),
        ];
        let s = sentences(&spoken, |_| 1, |p| p.1, 1.0);
        let tr = |t: &str| Some(if t == "b." { "B." } else { "X." });
        let got = apply(spoken, &s, tr, |t| t.to_string());
        let texts: Vec<(usize, &str)> = got.iter().map(|w| (w.track, w.text.as_str())).collect();
        assert_eq!(texts, [(0, "X."), (1, "B."), (0, "X.")]);
    }
}
