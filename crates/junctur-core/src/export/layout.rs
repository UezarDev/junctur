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

/// Places every cue's speakers by the caption settings' size, position, layout and protagonist.
pub fn place(cues: &[Cue], edit: &EditList, c: &Captions) -> Vec<Placed> {
    let protagonist = c.protagonist_in(&edit.speakers.names);
    cues.iter()
        .map(|cue| place_one(cue, protagonist, c))
        .collect()
}

fn place_one(cue: &Cue, protagonist: Option<u32>, c: &Captions) -> Placed {
    let em = c.size / 100.0;
    let bottom = c.position / 100.0;
    let height = |i: usize| cue.parts[i].lines.len().max(1) as f64 * em * LINE;
    // Who keeps the normal spot: the protagonist when they talk in this caption, else whoever
    // started first; the others follow in the order they started.
    let mut order: Vec<usize> = (0..cue.parts.len()).collect();
    if let Some(p) = protagonist.and_then(|id| cue.parts.iter().position(|q| q.speaker == id)) {
        order.remove(p);
        order.insert(0, p);
    }
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
    Placed {
        start: cue.start,
        end: cue.end,
        parts: cue
            .parts
            .iter()
            .zip(at)
            .map(|(p, (x, y))| PlacedPart {
                speaker: p.speaker,
                lines: p.lines.clone(),
                x,
                y,
            })
            .collect(),
    }
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
        place(std::slice::from_ref(cue), &edit(), c)[0]
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
}
