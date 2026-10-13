//! The caption text options (5j) applied to the kept words: find and replace, then case, then
//! punctuation, then censoring, giving the units [`super::srt::cues`] cuts into cues. Before the cut
//! and not on finished cues, because a replacement or a mask changes a line's width (wrapping on the
//! old words could pass the line limit) and a phrase to find can straddle two cues.
//!
//! Matching is on whole words, case aside: a Whisper word carries its punctuation (`"¿Qué?"`), so
//! each is split into that and its core, from its first to its last letter or digit, and only cores
//! are compared. It runs over one track's words at a time, since two people talking at once have
//! their words interleaved in time.
//!
//! A replacement's text is split back into words that keep the span of what it replaced (a
//! `group`), so case, punctuation and the censor list see each of its words; the group is joined
//! again into one unit at the end, so the cut still treats it as one word.

use crate::edit::{Captions, Case, CensorStyle};

/// A run of kept words shown as one: `first..=last` index the words given to [`units`].
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub first: usize,
    pub last: usize,
    pub text: String,
}

/// A word split for matching: what comes before its first letter or digit, from there to its
/// last, and the rest. Pieces of one `group` (a replacement's words) share its span and show as
/// one unit.
#[derive(Debug, Clone)]
struct Piece {
    first: usize,
    last: usize,
    group: usize,
    lead: String,
    core: String,
    trail: String,
}

impl Piece {
    fn new(i: usize, text: &str) -> Self {
        let start = text.find(char::is_alphanumeric);
        let (lead, core, trail) = match start {
            None => (text, "", ""),
            Some(a) => {
                let b = text
                    .char_indices()
                    .rev()
                    .find(|(_, c)| c.is_alphanumeric())
                    .map_or(text.len(), |(k, c)| k + c.len_utf8());
                (&text[..a], &text[a..b], &text[b..])
            }
        };
        Self {
            first: i,
            last: i,
            group: i,
            lead: lead.into(),
            core: core.into(),
            trail: trail.into(),
        }
    }

    /// The core lower-cased. `İ` lower-cases to `i` and a combining dot; the dot is dropped so
    /// `İstanbul` matches `istanbul`.
    fn key(&self) -> String {
        self.core.to_lowercase().replace('\u{307}', "")
    }
}

/// A list entry as the cores it matches, lower-cased: `"What the heck?"` is `what`, `the`, `heck`.
fn keys(entry: &str) -> Vec<String> {
    entry
        .split_whitespace()
        .map(|w| Piece::new(0, w).key())
        .filter(|k| !k.is_empty())
        .collect()
}

/// Whether `want` matches the pieces starting at `at`.
fn matches(pieces: &[Piece], keys: &[String], at: usize, want: &[String]) -> bool {
    !want.is_empty()
        && keys.get(at..at + want.len()).is_some_and(|k| k == want)
        && pieces[at..at + want.len()]
            .iter()
            .all(|p| !p.core.is_empty())
}

/// The units the kept `words` show as under `c`, in order. `words` is each kept word's track and
/// cleaned text, in time order; a word left empty is dropped. With every text option at its default
/// each word is its own unit, unchanged.
pub fn units(words: &[(usize, String)], c: &Captions) -> Vec<Unit> {
    let mut tracks: Vec<usize> = words.iter().map(|w| w.0).collect();
    tracks.sort_unstable();
    tracks.dedup();
    let mut out: Vec<Unit> = Vec::with_capacity(words.len());
    for t in tracks {
        let pieces: Vec<Piece> = words
            .iter()
            .enumerate()
            .filter(|(_, w)| w.0 == t)
            .map(|(i, w)| Piece::new(i, &w.1))
            .collect();
        // A group's pieces come one after another; joined with spaces into one unit.
        let mut group: Option<(usize, Unit)> = None;
        for p in censor(text_of(replace(pieces, c), c), c) {
            let text = format!("{}{}{}", p.lead, p.core, p.trail);
            let text = text.trim();
            match &mut group {
                Some((g, u)) if *g == p.group => {
                    if !text.is_empty() && !u.text.is_empty() {
                        u.text.push(' ');
                    }
                    u.text.push_str(text);
                }
                _ => {
                    out.extend(group.take().map(|(_, u)| u));
                    let unit = Unit {
                        first: p.first,
                        last: p.last,
                        text: text.to_string(),
                    };
                    group = Some((p.group, unit));
                }
            }
        }
        out.extend(group.map(|(_, u)| u));
    }
    out.retain(|u| !u.text.is_empty());
    out.sort_by_key(|u| u.first);
    out
}

/// Find and replace, one pass left to right; at each word the longest `from` that matches wins and
/// its words become one group of the `to`'s words, the first's leading and the last's trailing
/// punctuation round them; an empty `to` drops them, punctuation and all.
fn replace(pieces: Vec<Piece>, c: &Captions) -> Vec<Piece> {
    if c.replace.is_empty() {
        return pieces;
    }
    let mut pairs: Vec<(Vec<String>, &str)> = c
        .replace
        .iter()
        .map(|r| (keys(&r.from), r.to.as_str()))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    pairs.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    let keys: Vec<String> = pieces.iter().map(Piece::key).collect();
    let mut out = Vec::with_capacity(pieces.len());
    let mut i = 0;
    while i < pieces.len() {
        match pairs
            .iter()
            .find(|(want, _)| matches(&pieces, &keys, i, want))
        {
            Some((want, to)) => {
                let end = &pieces[i + want.len() - 1];
                let (first, last) = (pieces[i].first, end.last);
                let mut words: Vec<Piece> = to
                    .split_whitespace()
                    .map(|w| Piece {
                        first,
                        last,
                        group: first,
                        ..Piece::new(first, w)
                    })
                    .collect();
                if let Some(w) = words.first_mut() {
                    w.lead.insert_str(0, &pieces[i].lead);
                }
                if let Some(w) = words.last_mut() {
                    w.trail.push_str(&end.trail);
                }
                out.extend(words);
                i += want.len();
            }
            None => {
                out.push(pieces[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// Case, then punctuation removed if asked, on each piece.
fn text_of(mut pieces: Vec<Piece>, c: &Captions) -> Vec<Piece> {
    for p in &mut pieces {
        for s in [&mut p.lead, &mut p.core, &mut p.trail] {
            *s = cased(s, c.case);
        }
        if !c.punctuation {
            for s in [&mut p.lead, &mut p.core, &mut p.trail] {
                *s = unpunctuated(s);
            }
        }
    }
    pieces
}

fn cased(s: &str, case: Case) -> String {
    match case {
        Case::AsSpoken => s.to_string(),
        Case::Upper => s.to_uppercase(),
        Case::Lower => s.to_lowercase(),
        // The rest as spoken, so `NASA` survives (`iPhone` reads `IPhone`); lower-casing the rest
        // would break every acronym.
        Case::Title => {
            let mut out = String::with_capacity(s.len());
            let mut start = true;
            for ch in s.chars() {
                if start && ch.is_alphabetic() {
                    out.extend(ch.to_uppercase());
                    start = false;
                } else {
                    out.push(ch);
                    if ch.is_whitespace() || ch == '-' {
                        start = true;
                    } else if ch.is_alphanumeric() {
                        start = false;
                    }
                }
            }
            out
        }
    }
}

/// Punctuation, as opposed to symbols that carry meaning (`$ % & # @ + = * / € £ °` stay).
fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '…'
            | '¿'
            | '¡'
            | '"'
            | '\''
            | '`'
            | '“'
            | '”'
            | '„'
            | '‘'
            | '’'
            | '‚'
            | '«'
            | '»'
            | '‹'
            | '›'
            | '('
            | ')'
            | '['
            | ']'
            | '-'
            | '–'
            | '—'
            | '_'
            | '~'
            | '·'
            | '•'
    )
}

/// `s` without punctuation, but for an apostrophe or hyphen between letters (`don't`, `e-mail`)
/// and a point, comma or colon between digits (`5.50`, `1,000`, `10:30`), part of the number.
fn unpunctuated(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let between = |k: usize, f: fn(&char) -> bool| {
        k > 0 && f(&chars[k - 1]) && chars.get(k + 1).is_some_and(f)
    };
    for (k, &ch) in chars.iter().enumerate() {
        let inside = (matches!(ch, '\'' | '’' | '-') && between(k, |c| c.is_alphanumeric()))
            || (matches!(ch, '.' | ',' | ':') && between(k, char::is_ascii_digit));
        if inside || !is_punctuation(ch) {
            out.push(ch);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every piece of a censored word or phrase has its core masked; its punctuation stays.
fn censor(mut pieces: Vec<Piece>, c: &Captions) -> Vec<Piece> {
    let lists: Vec<Vec<String>> = c
        .censor
        .iter()
        .map(|e| keys(e))
        .filter(|k| !k.is_empty())
        .collect();
    if lists.is_empty() {
        return pieces;
    }
    let keys: Vec<String> = pieces.iter().map(Piece::key).collect();
    let mut hidden = vec![false; pieces.len()];
    for i in 0..pieces.len() {
        for want in &lists {
            if matches(&pieces, &keys, i, want) {
                hidden[i..i + want.len()].iter_mut().for_each(|h| *h = true);
            }
        }
    }
    for (p, _) in pieces.iter_mut().zip(&hidden).filter(|(_, h)| **h) {
        p.core = masked(&p.core, c.censor_style);
    }
    pieces
}

fn masked(core: &str, style: CensorStyle) -> String {
    let chars: Vec<char> = core.chars().collect();
    let n = chars.len();
    match style {
        CensorStyle::Whole => "*".repeat(n),
        CensorStyle::Middle => match n {
            0 | 1 => "*".repeat(n),
            2 => format!("{}*", chars[0]),
            _ => format!("{}{}{}", chars[0], "*".repeat(n - 2), chars[n - 1]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::Replace;

    fn shown(words: &[&str], c: &Captions) -> Vec<String> {
        let w: Vec<(usize, String)> = words.iter().map(|w| (0, w.to_string())).collect();
        units(&w, c).into_iter().map(|u| u.text).collect()
    }

    fn pair(from: &str, to: &str) -> Replace {
        Replace {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn the_defaults_show_the_words_as_spoken() {
        let w = ["¿Qué", "tal?", "...", "NASA"];
        assert_eq!(shown(&w, &Captions::default()), w);
        let u = units(&[(0, "a".into()), (1, "b".into())], &Captions::default());
        assert_eq!((u[1].first, u[1].last), (1, 1));
    }

    #[test]
    fn case_changes_letters_only() {
        let w = ["¿qué", "tal?", "NASA", "iPhone", "e-mail"];
        let with = |case| {
            shown(
                &w,
                &Captions {
                    case,
                    ..Default::default()
                },
            )
        };
        assert_eq!(
            with(Case::Upper),
            ["¿QUÉ", "TAL?", "NASA", "IPHONE", "E-MAIL"]
        );
        assert_eq!(
            with(Case::Lower),
            ["¿qué", "tal?", "nasa", "iphone", "e-mail"]
        );
        assert_eq!(
            with(Case::Title),
            ["¿Qué", "Tal?", "NASA", "IPhone", "E-Mail"]
        );
    }

    #[test]
    fn punctuation_goes_but_words_and_symbols_stay() {
        let c = Captions {
            punctuation: false,
            ..Default::default()
        };
        assert_eq!(
            shown(
                &["¿Qué", "tal?", "don't", "e-mail,", "50%", "$5.", "—", "\"hi\"", "(sí)"],
                &c
            ),
            ["Qué", "tal", "don't", "e-mail", "50%", "$5", "hi", "sí"]
        );
        // A point, comma or colon inside a number is part of it; one after it is not.
        assert_eq!(
            shown(&["$5.50.", "3.14,", "1,000", "10:30:", "a.b"], &c),
            ["$5.50", "3.14", "1,000", "10:30", "ab"]
        );
    }

    #[test]
    fn censored_words_are_masked_whole_words_case_aside() {
        let c = Captions {
            censor: vec!["damn".into(), "what the heck".into(), "ok".into()],
            ..Default::default()
        };
        assert_eq!(
            shown(
                &[
                    "Damn!",
                    "damnation",
                    "what",
                    "the",
                    "HECK?",
                    "the",
                    "heck",
                    "OK",
                    "a"
                ],
                &c
            ),
            [
                "D**n!",
                "damnation",
                "w**t",
                "t*e",
                "H**K?",
                "the",
                "heck",
                "O*",
                "a"
            ]
        );
        let whole = Captions {
            censor_style: CensorStyle::Whole,
            ..c
        };
        assert_eq!(shown(&["Damn!", "x"], &whole), ["****!", "x"]);
        // A one-letter word.
        let one = Captions {
            censor: vec!["a".into()],
            ..Default::default()
        };
        assert_eq!(shown(&["a", "b"], &one), ["*", "b"]);
        // Case aside covers `İ`, which lower-cases with a combining dot.
        let city = Captions {
            censor: vec!["istanbul".into()],
            ..Default::default()
        };
        assert_eq!(shown(&["İstanbul"], &city), ["İ******l"]);
    }

    #[test]
    fn the_censor_list_sees_each_word_of_a_replacement() {
        let c = Captions {
            replace: vec![pair("darn", "damn it"), pair("gosh", "oh my god")],
            censor: vec!["damn".into(), "my god".into()],
            case: Case::Title,
            ..Default::default()
        };
        let w: Vec<(usize, String)> = ["Darn!", "gosh,", "so"]
            .iter()
            .map(|w| (0, w.to_string()))
            .collect();
        // Still one unit each, masked word by word, the punctuation round the whole.
        let text: Vec<(usize, usize, String)> = units(&w, &c)
            .into_iter()
            .map(|u| (u.first, u.last, u.text))
            .collect();
        assert_eq!(
            text,
            [
                (0, 0, "D**n It!".to_string()),
                (1, 1, "Oh M* G*d,".to_string()),
                (2, 2, "So".to_string())
            ]
        );
        // A phrase replaced across words keeps their span.
        let p = Captions {
            replace: vec![pair("new york", "New York City")],
            ..Default::default()
        };
        let w: Vec<(usize, String)> = ["new", "york."]
            .iter()
            .map(|w| (0, w.to_string()))
            .collect();
        assert_eq!(
            units(&w, &p),
            [Unit {
                first: 0,
                last: 1,
                text: "New York City.".into()
            }]
        );
    }

    #[test]
    fn replacements_match_whole_words_and_phrases_before_case_and_censoring() {
        let c = Captions {
            replace: vec![
                pair("gonna", "going to"),
                pair("you know", ""),
                pair("you", "ya"),
                pair("frick", "damn"),
            ],
            censor: vec!["damn".into()],
            case: Case::Upper,
            ..Default::default()
        };
        let w: Vec<(usize, String)> = [
            "¿Gonna",
            "go?",
            "You",
            "know,",
            "you",
            "gonnadance",
            "Frick!",
        ]
        .iter()
        .map(|w| (0, w.to_string()))
        .collect();
        let u = units(&w, &c);
        let text: Vec<&str> = u.iter().map(|u| u.text.as_str()).collect();
        // The longer phrase wins over `you`; its empty `to` drops it, comma and all; the censor
        // list sees the replacement.
        assert_eq!(text, ["¿GOING TO", "GO?", "YA", "GONNADANCE", "D**N!"]);
        assert_eq!((u[0].first, u[0].last), (0, 0));
        assert_eq!((u[2].first, u[2].last), (4, 4));
        // A phrase becomes one unit spanning its words.
        let p = Captions {
            replace: vec![pair("new york", "NYC")],
            ..Default::default()
        };
        let w: Vec<(usize, String)> = ["in", "New", "York."]
            .iter()
            .map(|w| (0, w.to_string()))
            .collect();
        let u = units(&w, &p);
        assert_eq!(
            u[1],
            Unit {
                first: 1,
                last: 2,
                text: "NYC.".into()
            }
        );
    }

    #[test]
    fn a_phrase_does_not_match_across_two_tracks() {
        let c = Captions {
            replace: vec![pair("new york", "NYC")],
            ..Default::default()
        };
        // Track 0 says "new", track 1 says "york" in between; track 0 then says "york".
        let w = vec![
            (0, "new".to_string()),
            (1, "york".to_string()),
            (0, "york".to_string()),
        ];
        let u = units(&w, &c);
        assert_eq!(
            u,
            vec![
                Unit {
                    first: 0,
                    last: 2,
                    text: "NYC".into()
                },
                Unit {
                    first: 1,
                    last: 1,
                    text: "york".into()
                }
            ]
        );
    }
}
