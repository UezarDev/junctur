//! Where each speaker's caption sits in the picture (5g): the preview over the picture and the Text+
//! clips sent to Resolve both place captions from here, so what the editor shows is what Resolve
//! gets. The SRT does not use it: a subtitle track has one position for every caption (5d), so it
//! keeps 5e's one block, first speaker first.
//!
//! Positions are the centre of each speaker's block as fractions of the picture, x across from
//! the left and y up from the bottom (Fusion's `Center`). Heights are in picture heights: a block
//! is `lines × size × LINE` tall, the size being the font's em.

use serde::Serialize;

use super::srt::Cue;
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
                lines,
            })
            .collect(),
    }
}

/// `lines`' words wrapped again, greedily, at `chars` characters a line; lines already that
/// short are kept as they are.
fn rewrap(lines: &[String], chars: usize) -> Vec<String> {
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
                },
                Part {
                    speaker: 2,
                    lines: vec!["x".into()],
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
}
