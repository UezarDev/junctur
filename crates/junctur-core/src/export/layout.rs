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
mod tests {
    use super::*;
    use crate::edit::{SpeakerName, Speakers};
    use crate::export::srt::Part;

    fn cue(parts: &[(u32, usize)]) -> Cue {
        Cue {
            start: 1.0,
            end: 2.0,
            parts: parts
                .iter()
                .map(|&(speaker, lines)| Part {
                    speaker,
                    lines: vec!["x".into(); lines],
                    words: Vec::new(),
                })
                .collect(),
        }
    }

    fn edit() -> EditList {
        let name = |id: u32, n: &str| SpeakerName {
            id,
            name: n.into(),
            track: None,
            color: "#FFFFFF".into(),
        };
        EditList {
            source: "x.mp4".into(),
            duration: 10.0,
            settings: Default::default(),
            keep: Vec::new(),
            speakers: Speakers {
                names: vec![name(1, "Ana"), name(2, "Bo"), name(3, "Cy")],
                turns: Vec::new(),
            },
            captions: None,
            fixes: Vec::new(),
            times: Vec::new(),
            windows: Vec::new(),
        }
    }

    // Size 5 % and position 10 % so the numbers are round: one line is 0.06 tall, the gap 0.015.
    fn settings(layout: Layout, protagonist: &str) -> Captions {
        Captions {
            size: 5.0,
            position: 10.0,
            layout,
            protagonist: protagonist.into(),
            ..Default::default()
        }
    }

    fn spots(cue: &Cue, c: &Captions) -> Vec<(u32, f64, f64)> {
        place(std::slice::from_ref(cue), &edit(), c, 9.0 / 16.0)[0]
            .parts
            .iter()
            .map(|p| {
                let r = |v: f64| (v * 1000.0).round() / 1000.0;
                (p.speaker, r(p.x), r(p.y))
            })
            .collect()
    }

    #[test]
    fn one_speaker_sits_at_the_normal_spot_whatever_the_layout() {
        for layout in [Layout::Stacked, Layout::Side, Layout::TopBottom] {
            // Two lines: the bottom edge stays at the position, so the centre is half the block up.
            assert_eq!(
                spots(&cue(&[(1, 2)]), &settings(layout, "")),
                [(1, 0.5, 0.16)]
            );
        }
    }

    #[test]
    fn stacked_puts_each_later_speaker_above() {
        // Ana (2 lines) from 0.1 to 0.22, gap to 0.235, Bo (1 line) centred 0.03 above that.
        assert_eq!(
            spots(&cue(&[(1, 2), (2, 1)]), &settings(Layout::Stacked, "")),
            [(1, 0.5, 0.16), (2, 0.5, 0.265)]
        );
        // The protagonist keeps the normal spot though they started second.
        assert_eq!(
            spots(&cue(&[(1, 1), (2, 2)]), &settings(Layout::Stacked, "bo")),
            [(1, 0.5, 0.265), (2, 0.5, 0.16)]
        );
        // A protagonist who is not talking changes nothing.
        assert_eq!(
            spots(&cue(&[(1, 1), (2, 1)]), &settings(Layout::Stacked, "Cy")),
            spots(&cue(&[(1, 1), (2, 1)]), &settings(Layout::Stacked, ""))
        );
    }

    #[test]
    fn side_by_side_spreads_across_or_round_the_protagonist() {
        assert_eq!(
            spots(&cue(&[(1, 1), (2, 1)]), &settings(Layout::Side, "")),
            [(1, 0.25, 0.13), (2, 0.75, 0.13)]
        );
        let three = spots(&cue(&[(1, 1), (2, 1), (3, 1)]), &settings(Layout::Side, ""));
        assert_eq!(
            three.iter().map(|s| s.1).collect::<Vec<_>>(),
            [0.167, 0.5, 0.833]
        );
        // Bo centred, Ana to the right, Cy to the left.
        assert_eq!(
            spots(
                &cue(&[(1, 1), (2, 1), (3, 1)]),
                &settings(Layout::Side, "Bo")
            ),
            [(1, 0.8, 0.13), (2, 0.5, 0.13), (3, 0.2, 0.13)]
        );
    }

    #[test]
    fn top_bottom_sends_the_second_to_the_top() {
        // Bo's one line: top edge 0.1 below the top, so centred at 1 - 0.1 - 0.03.
        assert_eq!(
            spots(&cue(&[(1, 1), (2, 1)]), &settings(Layout::TopBottom, "")),
            [(1, 0.5, 0.13), (2, 0.5, 0.87)]
        );
        // With Bo the protagonist, Bo is the one kept at the bottom; a third stacks above them.
        assert_eq!(
            spots(
                &cue(&[(1, 1), (2, 1), (3, 1)]),
                &settings(Layout::TopBottom, "Bo")
            ),
            [(1, 0.5, 0.87), (2, 0.5, 0.13), (3, 0.5, 0.205)]
        );
    }

    #[test]
    fn side_by_side_wraps_each_block_to_its_column() {
        let long = "una frase bastante larga que llena la línea";
        let cue = Cue {
            start: 1.0,
            end: 2.0,
            parts: [1, 2]
                .map(|speaker| Part {
                    speaker,
                    lines: vec![long.into()],
                    words: Vec::new(),
                })
                .to_vec(),
        };
        // 5 % text on 16:9: a character is about 0.014 of the width, half the picture holds 32.
        let placed = &place(
            std::slice::from_ref(&cue),
            &edit(),
            &settings(Layout::Side, ""),
            9.0 / 16.0,
        )[0];
        for p in &placed.parts {
            assert!(p.lines.len() > 1, "{:?}", p.lines);
            assert!(
                p.lines.iter().all(|l| l.chars().count() <= 32),
                "{:?}",
                p.lines
            );
            assert_eq!(p.lines.join(" "), long);
        }
        // Two lines tall now: the bottom edge stays at the position.
        assert!((placed.parts[0].y - (0.1 + 0.06)).abs() < 1e-9);
        // Stacked, the lines are the cue's own.
        let stacked = &place(&[cue], &edit(), &settings(Layout::Stacked, ""), 9.0 / 16.0)[0];
        assert_eq!(stacked.parts[0].lines, [long]);
        assert_eq!(rewrap(&["a b".into(), "c".into()], 3), ["a b", "c"]);
    }

    /// Reference: the greedy wrap written out word by word.
    fn rewrap_before(lines: &[String], chars: usize) -> Vec<String> {
        if lines.iter().all(|l| l.chars().count() <= chars) {
            return lines.to_vec();
        }
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        for word in lines.iter().flat_map(|l| l.split_whitespace()) {
            if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > chars {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    #[test]
    fn a_column_wraps_as_the_cues_do() {
        let texts = [
            vec![],
            vec![""],
            vec!["   "],
            vec!["one"],
            vec!["a b c d e f g h i j k l m n o p"],
            vec![
                "supercalifragilistic is long",
                "and so is antidisestablishment",
            ],
            vec!["  spaced   out	words ", "", "next line"],
            vec!["ünïcødé wörds çan wräp", "日本語 の 字幕 です 長い 行"],
            vec!["exactly ten", "chars here", "x"],
            vec!["¿Qué tal? — bien, gracias.", "Ça va très bien, merci!"],
        ];
        for lines in texts {
            let lines: Vec<String> = lines.into_iter().map(String::from).collect();
            for chars in [0, 1, 3, 5, 7, 10, 11, 16, 24, 42, 200] {
                assert_eq!(
                    rewrap(&lines, chars),
                    rewrap_before(&lines, chars),
                    "{lines:?} at {chars}"
                );
            }
        }
    }

    #[test]
    fn blocks_stay_inside_the_picture() {
        // Three speakers of three lines at 15 %: the stack would run off the top.
        let big = Captions {
            size: 15.0,
            ..settings(Layout::Stacked, "")
        };
        for p in &place(&[cue(&[(1, 3), (2, 3), (3, 3)])], &edit(), &big, 9.0 / 16.0)[0].parts {
            let h = 3.0 * 0.15 * LINE;
            assert!(
                p.y - h / 2.0 >= -1e-9 && p.y + h / 2.0 <= 1.0 + 1e-9,
                "{}",
                p.y
            );
        }
        // A wide line beside a protagonist is pulled in from the edge.
        let wide = Cue {
            start: 1.0,
            end: 2.0,
            parts: vec![
                Part {
                    speaker: 1,
                    lines: vec!["extraordinariamente".into()],
                    words: Vec::new(),
                },
                Part {
                    speaker: 2,
                    lines: vec!["x".into()],
                    words: Vec::new(),
                },
            ],
        };
        let c = Captions {
            size: 15.0,
            ..settings(Layout::Side, "Bo")
        };
        let p = &place(&[wide], &edit(), &c, 9.0 / 16.0)[0].parts[0];
        let w = 19.0 * 0.15 * (9.0 / 16.0) * GLYPH;
        assert!((p.x - (1.0 - w / 2.0)).abs() < 1e-9, "{}", p.x);
    }
    #[test]
    fn the_highlight_finds_each_unit_on_the_placed_lines() {
        let said = |text: &str, start: f64| Spoken {
            text: text.into(),
            start,
            end: start + 0.4,
        };
        // A replaced phrase is one unit, but the animated caption lights a word at a time:
        // "¿qué tal?" is two words sharing its time, "a todos" two, re-wrapped over two lines.
        // Indices count characters, "¿" and "é" one each. A no-break space is not Lua's
        // whitespace, so "10 %" with one stays one word, as the macro counts it.
        let lines = vec![
            "hola ¿qué tal? a".to_string(),
            "todos 10\u{a0}%".to_string(),
        ];
        let words = [
            said("hola", 1.0),
            said("¿qué tal?", 1.6),
            said("a todos", 2.0),
            said("10\u{a0}%", 3.0),
        ];
        let ms = |t: f64| (t * 1000.0).round() as i64;
        let got: Vec<_> = spans(&lines, &words)
            .iter()
            .map(|s| (s.word, s.line, s.from, s.to, ms(s.start), ms(s.end)))
            .collect();
        assert_eq!(
            got,
            [
                (0, 0, 0, 4, 1000, 1400),
                (1, 0, 5, 9, 1600, 1800),
                (2, 0, 10, 14, 1800, 2000),
                (3, 0, 15, 16, 2000, 2200),
                (4, 1, 0, 5, 2200, 2400),
                (5, 1, 6, 10, 3000, 3400),
            ]
        );
        // Units that do not match the lines give nothing rather than the wrong word lit.
        assert!(spans(&lines, &words[..2]).is_empty());
        assert!(spans(&lines, &[said("adiós", 0.0)]).is_empty());
        // Only with the highlight on; side by side, the spans follow the re-wrapped lines.
        let long = [
            "una", "frase", "bastante", "larga", "que", "llena", "la", "línea",
        ];
        let cue = Cue {
            start: 1.0,
            end: 5.0,
            parts: [1, 2]
                .map(|speaker| Part {
                    speaker,
                    lines: vec![long.join(" ")],
                    words: long
                        .iter()
                        .enumerate()
                        .map(|(k, w)| said(w, 1.0 + k as f64 * 0.5))
                        .collect(),
                })
                .to_vec(),
        };
        let off = place(
            std::slice::from_ref(&cue),
            &edit(),
            &settings(Layout::Side, ""),
            9.0 / 16.0,
        );
        assert!(off[0].parts[0].words.is_empty());
        let on = Captions {
            highlight: true,
            ..settings(Layout::Side, "")
        };
        let p = &place(&[cue], &edit(), &on, 9.0 / 16.0)[0].parts[0];
        assert!(p.lines.len() > 1, "{:?}", p.lines);
        assert_eq!(p.words.len(), long.len());
        for s in &p.words {
            let shown: String = p.lines[s.line]
                .chars()
                .skip(s.from)
                .take(s.to - s.from)
                .collect();
            assert_eq!(shown, long[s.word]);
            assert_eq!(s.start, 1.0 + s.word as f64 * 0.5);
        }
    }
}
