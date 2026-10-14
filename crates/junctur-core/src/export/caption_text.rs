//! The caption text options applied to the kept words: find and replace, then case, punctuation
//! and censoring, giving the units [`super::srt::cues`] cuts into cues. Before the cut, since a
//! replacement changes a line's width and a phrase to find can straddle two cues. Matching is on
//! whole words, case aside, comparing each word's core (first to last letter or digit), one track
//! at a time since people talking at once interleave in time.

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
        let (lead, core, trail) = crate::text::split_bare(text);
        Self {
            first: i,
            last: i,
            group: i,
            lead: lead.into(),
            core: core.into(),
            trail: trail.into(),
        }
    }

    /// The core lower-cased. `İ` lower-cases to `i` and a combining dot, so it is made a plain `i`
    /// first (written whole or as `I` and the dot) and `İstanbul` matches `istanbul`. Only that
    /// dot goes, so a letter written with a combining dot of its own (`ż` as `z` and the dot) does
    /// not match its bare letter.
    fn key(&self) -> String {
        self.core
            .replace('\u{130}', "i")
            .replace("I\u{307}", "i")
            .to_lowercase()
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
    crate::text::one_spaced(&out)
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
mod tests;
