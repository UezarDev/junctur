//! Where each speaker's caption sits in the picture: the preview and the Text+ clips sent to
//! Resolve both place captions from here, so the editor shows what Resolve gets. Positions are
//! block centres as fractions of the picture, x from the left and y up from the bottom (Fusion's
//! `Center`); heights are in picture heights, a block `lines × size × LINE` tall. The SRT does not
//! use it: a subtitle track has one position for every caption.

use serde::Serialize;

use super::srt::{wrap_onto, Cue, Spoken};
use crate::edit::{Captions, EditList, Layout};

/// Line height over the em, the usual default of a text renderer (Fusion's and a browser's).
pub const LINE: f64 = 1.2;
/// Space between two stacked blocks, in ems.
const GAP: f64 = 0.3;
/// Side by side round a protagonist: the others' centres, right then left.
const BESIDE: [f64; 2] = [0.8, 0.2];

/// One speaker's lines in a placed caption.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlacedPart {
    /// Speaker id, 0 for none.
    pub speaker: u32,
    pub lines: Vec<String>,
    /// Centre of the block, fractions of the picture: x from the left, y from the bottom.
    pub x: f64,
    pub y: f64,
    /// Where each word sits on `lines` and when it is lit, only with the highlight on.
    pub words: Vec<WordSpan>,
}

/// A word's characters on its placed line: `from..to` in Unicode scalar values, which is how the
/// animated caption counts them too (its `utf8len` counts every byte that does not continue a
/// UTF-8 sequence).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WordSpan {
    /// Which word of the part, in order.
    pub word: usize,
    pub line: usize,
    pub from: usize,
    pub to: usize,
    /// Output seconds.
    pub start: f64,
    pub end: f64,
}

/// A caption with each speaker's block placed; parts in the cue's order (who started first).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placed {
    /// Output seconds.
    pub start: f64,
    pub end: f64,
    pub parts: Vec<PlacedPart>,
}

/// An average glyph's advance over the em, for guessing a line's width: the layout cannot measure
/// the font (the preview and Text+ draw it), so side-by-side columns are kept with room to spare.
const GLYPH: f64 = 0.5;
/// Side by side round a protagonist: each column's width (the gap between [`BESIDE`]'s centres).
const BESIDE_COLUMN: f64 = 0.3;
/// Fewest characters a side-by-side line is narrowed to, so a tiny column still holds a word.
const MIN_CHARS: usize = 6;

/// Places every cue's speakers by the caption settings' size, position, layout and protagonist,
/// on a picture `aspect` (height over width) in shape: side by side, each speaker's lines are
/// wrapped again to fit their column, and every block is kept inside the picture.
pub fn place(cues: &[Cue], edit: &EditList, c: &Captions, aspect: f64) -> Vec<Placed> {
    let protagonist = c.protagonist_in(&edit.speakers.names);
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        9.0 / 16.0
    };
    cues.iter()
        .map(|cue| place_one(cue, protagonist, c, aspect))
        .collect()
}

fn place_one(cue: &Cue, protagonist: Option<u32>, c: &Captions, aspect: f64) -> Placed {
    let em = c.size / 100.0;
    let bottom = c.position / 100.0;
    // One character's width in picture widths.
    let glyph = em * aspect * GLYPH;
    // Who keeps the normal spot: the protagonist when they talk in this caption, else whoever
    // started first; the others follow in the order they started.
    let mut order: Vec<usize> = (0..cue.parts.len()).collect();
    let keeps = protagonist.and_then(|id| cue.parts.iter().position(|q| q.speaker == id));
    if let Some(p) = keeps {
        order.remove(p);
        order.insert(0, p);
    }
    // Side by side, a block wider than its column would run into the next: its words are wrapped
    // again to the column.
    let column = match (c.layout, cue.parts.len()) {
        (Layout::Side, n) if n > 1 && keeps.is_some() => Some(BESIDE_COLUMN),
        (Layout::Side, n) if n > 1 => Some(1.0 / n as f64),
        _ => None,
    };
    let lines: Vec<Vec<String>> = cue
        .parts
        .iter()
        .map(|p| match column {
            Some(w) => {
                let chars = ((w * 0.9 / glyph) as usize).max(MIN_CHARS);
                rewrap(&p.lines, chars)
            }
            None => p.lines.clone(),
        })
        .collect();
    let height = |i: usize| lines[i].len().max(1) as f64 * em * LINE;
    let width = |i: usize| {
        lines[i]
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0) as f64
            * glyph
    };
    let mut at = vec![(0.5, 0.0); cue.parts.len()];
    // A stack from `bottom` up through `ids`, each centred at `x`.
    let stack = |ids: &[usize], x: f64, at: &mut Vec<(f64, f64)>| {
        let mut edge = bottom;
        for &i in ids {
            at[i] = (x, edge + height(i) / 2.0);
            edge += height(i) + GAP * em;
        }
    };
    match c.layout {
        Layout::Stacked => stack(&order, 0.5, &mut at),
        Layout::TopBottom => {
            let mut low = vec![order[0]];
            low.extend(order.iter().skip(2));
            stack(&low, 0.5, &mut at);
            if let Some(&top) = order.get(1) {
                at[top] = (0.5, 1.0 - bottom - height(top) / 2.0);
            }
        }
        Layout::Side if protagonist.is_some_and(|id| cue.parts[order[0]].speaker == id) => {
            // The protagonist stays centred; two others beside, any more above the protagonist.
            let mut centre = vec![order[0]];
            centre.extend(order.iter().skip(1 + BESIDE.len()));
            stack(&centre, 0.5, &mut at);
            for (&i, x) in order.iter().skip(1).zip(BESIDE) {
                stack(&[i], x, &mut at);
            }
        }
        Layout::Side => {
            let n = order.len() as f64;
            for (k, &i) in order.iter().enumerate() {
                stack(&[i], (2.0 * k as f64 + 1.0) / (2.0 * n), &mut at);
            }
        }
    }
    let sizes: Vec<(f64, f64)> = (0..cue.parts.len())
        .map(|i| (width(i), height(i)))
        .collect();
    // Inside the picture: a tall stack or a wide block near an edge is pushed back in.
    let inside = |v: f64, size: f64| {
        if size >= 1.0 {
            0.5
        } else {
            v.clamp(size / 2.0, 1.0 - size / 2.0)
        }
    };
    Placed {
        start: cue.start,
        end: cue.end,
        parts: cue
            .parts
            .iter()
            .zip(lines)
            .zip(at)
            .enumerate()
            .map(|(i, ((p, lines), (x, y)))| PlacedPart {
                speaker: p.speaker,
                x: inside(x, sizes[i].0),
                y: inside(y, sizes[i].1),
                words: if c.highlight {
                    spans(&lines, &p.words)
                } else {
                    Vec::new()
                },
                lines,
            })
            .collect(),
    }
}

/// Whitespace as Lua's `%s` has it (C's `isspace`): the animated caption splits its text into
/// words with `%S+`, so the words here are cut the same way, and a no-break space stays inside one.
pub fn is_lua_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r')
}

/// Where `words` sit on `lines`, a span per word as the animated caption counts words (split at
/// whitespace): a unit of several words gets a span each, sharing its time evenly in turn, since
/// the macro lights one word at a time. Nothing when they do not line up: no highlight rather
/// than the wrong word lit.
fn spans(lines: &[String], words: &[Spoken]) -> Vec<WordSpan> {
    // Every piece of the lines, as (line, from, to, text).
    let mut pieces = Vec::new();
    for (k, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut from = None;
        for i in 0..=chars.len() {
            let blank = chars.get(i).is_none_or(|&c| is_lua_space(c));
            match (blank, from) {
                (false, None) => from = Some(i),
                (true, Some(f)) => {
                    pieces.push((k, f, i, chars[f..i].iter().collect::<String>()));
                    from = None;
                }
                _ => {}
            }
        }
    }
    let mut out: Vec<WordSpan> = Vec::new();
    let mut next = pieces.into_iter();
    for w in words {
        let texts: Vec<&str> = w
            .text
            .split(is_lua_space)
            .filter(|t| !t.is_empty())
            .collect();
        let step = (w.end - w.start).max(0.0) / texts.len().max(1) as f64;
        for (i, text) in texts.into_iter().enumerate() {
            let Some((line, from, to, piece)) = next.next() else {
                return Vec::new();
            };
            if piece != text {
                return Vec::new();
            }
            let start = w.start + step * i as f64;
            out.push(WordSpan {
                word: out.len(),
                line,
                from,
                to,
                start,
                end: start + step,
            });
        }
    }
    if next.next().is_some() {
        return Vec::new();
    }
    out
}

/// `lines`' words wrapped again, greedily, at `chars` characters a line; lines already that
/// short are kept as they are.
fn rewrap(lines: &[String], chars: usize) -> Vec<String> {
    if lines.iter().all(|l| l.chars().count() <= chars) {
        return lines.to_vec();
    }
    lines
        .iter()
        .flat_map(|l| l.split_whitespace())
        .fold(Vec::new(), |out, w| wrap_onto(out, w, chars))
}

#[cfg(test)]
mod tests;
