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
