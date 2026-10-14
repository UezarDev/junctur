//! The edit list's text form. One fact per line, so a git diff of a hand edit is one line:
//!
//! ```text
//! junctur-edit 3
//! source C:\Videos\stream.mp4
//! duration 6304.021
//! settings pad_before=0.25 pad_after=0.35 merge_gap=1 snap=0.15 crossfade=0.03 window=5
//! # kept 4012.3 s of 6304.0 s (64%) in 812 ranges
//! keep 1.234 5.678
//! captions on chars=42 lines=2 seconds=6
//! speaker 1 color=#F2B33D Speaker 1
//! speaker 2 track=1 color=#5FB3F2 Lucía
//! turn 1.300 4.100 1
//! fix 1 2.340 Lucía
//! window 0 0.820 0.820 14
//! ```
//!
//! `#` lines are comments. `source` and a speaker's name (after its id and the optional `track=<n>`
//! and `color=#RRGGBB`) take the rest of the line, so they need no quoting. Times are seconds with
//! millisecond precision. A word in a `fix`, `span` or `time` line is `<start>`, or `<start>+<n>`
//! when `n` earlier words on its track start in that millisecond: `fix <track> <word> <text>`,
//! `span <track> <first> <last> <text>` for words fixed as one, `time <track> <word> <start> <end>`.

use std::fmt::Write;
use std::path::PathBuf;

use super::captions_line;
use super::{
    check_fix, check_span, default_color, default_name, fixes, parse_color, times, CutSettings,
    EditList, Fix, Range, Retime, SpeakerName, Speakers, Turn, Window, WordAt,
};

/// The newest version this build writes; it reads 1 to this. A list is written at the lowest
/// version that holds what it uses, so older builds still read it: 6 with a `time` line, 5 with a
/// `span`, 4 with a `+<n>` word, 3 with a `fix`, 2 with speakers or captions, else 1.
pub const FORMAT_VERSION: u32 = 6;

pub fn write(e: &EditList) -> String {
    let s = &e.settings;
    let mut out = String::new();
    let version = if !e.times.is_empty() {
        FORMAT_VERSION
    } else if e.fixes.iter().any(Fix::is_span) {
        5
    } else if e.fixes.iter().any(|f| f.nth > 0) {
        4
    } else if !e.fixes.is_empty() {
        3
    } else if e.speakers.is_empty() && e.captions.is_none() {
        1
    } else {
        2
    };
    let _ = writeln!(out, "junctur-edit {version}");
    let _ = writeln!(out, "source {}", e.source.display());
    let _ = writeln!(out, "duration {:.3}", e.duration);
    let _ = writeln!(
        out,
        "settings pad_before={} pad_after={} merge_gap={} snap={} crossfade={} window={}",
        s.pad_before, s.pad_after, s.merge_gap, s.snap, s.crossfade, s.window
    );
    let share = if e.duration > 0.0 {
        e.kept() / e.duration * 100.0
    } else {
        0.0
    };
    let _ = writeln!(
        out,
        "# kept {:.1} s of {:.1} s ({share:.0}%) in {} ranges",
        e.kept(),
        e.duration,
        e.keep.len()
    );
    let _ = writeln!(out, "# keep <start> <end>");
    for r in &e.keep {
        let _ = writeln!(out, "keep {:.3} {:.3}", r.start, r.end);
    }
    if let Some(c) = &e.captions {
        let _ = writeln!(out, "{}", captions_line::HELP);
        let _ = writeln!(out, "{}", captions_line::write(c));
    }
    let sp = &e.speakers;
    if !sp.is_empty() {
        let _ = writeln!(
            out,
            "# speaker <id> [track=<n>] color=<#RRGGBB> <name>: rename or recolour by editing"
        );
        for n in &sp.names {
            let track = n.track.map(|t| format!("track={t} ")).unwrap_or_default();
            let _ = writeln!(out, "speaker {} {track}color={} {}", n.id, n.color, n.name);
        }
        let _ = writeln!(out, "# turn <start> <end> <speaker id>");
        for t in &sp.turns {
            let _ = writeln!(out, "turn {:.3} {:.3} {}", t.start, t.end, t.speaker);
        }
    }
    if !e.fixes.is_empty() {
        let _ = writeln!(
            out,
            "# fix <track> <start>[+<n>] <text>: a word's spelling, fixed by hand (+n: the n-th later word starting in that millisecond)"
        );
        if e.fixes.iter().any(Fix::is_span) {
            let _ = writeln!(
                out,
                "# span <track> <first start>[+<n>] <last start>[+<n>] <text>: the words from first to last, shown as one text"
            );
        }
        let at = |start: f64, nth: usize| {
            let nth = if nth > 0 {
                format!("+{nth}")
            } else {
                String::new()
            };
            format!("{}{nth}", fixes::written(start))
        };
        let mut fixes = e.fixes.clone();
        fixes::sort(&mut fixes);
        for f in &fixes {
            let first = at(f.start, f.nth);
            let _ = match f.through {
                Some(last) => writeln!(
                    out,
                    "span {} {first} {} {}",
                    f.track,
                    at(last.start, last.nth),
                    f.text
                ),
                None => writeln!(out, "fix {} {first} {}", f.track, f.text),
            };
        }
    }
    if !e.times.is_empty() {
        let _ = writeln!(
            out,
            "# time <track> <start>[+<n>] <new start> <new end>: a word retimed by moving a cut's edge (start as heard)"
        );
        let mut times = e.times.clone();
        times::sort(&mut times);
        for t in &times {
            let _ = writeln!(out, "time {}", times::line(t));
        }
    }
    let _ = writeln!(out, "# window <start> <score> <speech share> <words>");
    for w in &e.windows {
        let _ = writeln!(
            out,
            "window {} {:.3} {:.3} {}",
            w.start, w.score, w.speech, w.words
        );
    }
    out
}

pub fn parse(text: &str) -> Result<EditList, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));
    let version = |l: &str| {
        l.strip_prefix("junctur-edit ")
            .and_then(|v| v.parse::<u32>().ok())
    };
    let read_version = match lines.next() {
        Some((_, l)) if version(l).is_some_and(|v| (1..=FORMAT_VERSION).contains(&v)) => {
            version(l).unwrap_or(FORMAT_VERSION)
        }
        _ => {
            return Err(format!(
                "not a junctur-edit file this build reads (versions 1 to {FORMAT_VERSION})"
            ))
        }
    };
    let mut e = EditList {
        source: PathBuf::new(),
        duration: 0.0,
        settings: CutSettings::default(),
        keep: Vec::new(),
        speakers: Speakers::default(),
        captions: None,
        fixes: Vec::new(),
        times: Vec::new(),
        windows: Vec::new(),
    };
    // Turn lines may come before the speaker lines they name; checked once all are read.
    let mut turn_lines: Vec<usize> = Vec::new();
    for (n, line) in lines {
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        let bad = |what: &str| format!("line {n}: {what}: {line}");
        let nums = || -> Result<Vec<f64>, String> {
            rest.split_whitespace()
                .map(|x| x.parse::<f64>().map_err(|_| bad("not a number")))
                .collect()
        };
        match key {
            "source" => e.source = PathBuf::from(rest),
            "duration" => e.duration = rest.trim().parse().map_err(|_| bad("not a number"))?,
            "settings" => {
                for kv in rest.split_whitespace() {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| bad("expected key=value"))?;
                    let v: f64 = v.parse().map_err(|_| bad("not a number"))?;
                    let s = &mut e.settings;
                    match k {
                        "pad_before" => s.pad_before = v,
                        "pad_after" => s.pad_after = v,
                        "merge_gap" => s.merge_gap = v,
                        "snap" => s.snap = v,
                        "crossfade" => s.crossfade = v,
                        "window" => s.window = v,
                        _ => return Err(bad("unknown setting")),
                    }
                }
            }
            "keep" => match nums()?[..] {
                [start, end] if end > start => e.keep.push(Range { start, end }),
                _ => return Err(bad("expected `keep <start> <end>` with end after start")),
            },
            "speaker" => {
                let (id, rest) = rest.split_once(' ').unwrap_or((rest, ""));
                let id: u32 = match id.parse() {
                    Ok(id) if id > 0 => id,
                    _ => return Err(bad("expected `speaker <id> [track=<n>] <name>`, id from 1")),
                };
                if e.speakers.names.iter().any(|n| n.id == id) {
                    return Err(bad("a second `speaker` line for this id"));
                }
                let mut rest = rest.trim();
                let mut track = None;
                let mut color = None;
                // `track=` then `color=`, each optional; the rest is the name.
                if let Some(r) = rest.strip_prefix("track=") {
                    let (t, r) = r.split_once(' ').unwrap_or((r, ""));
                    track = Some(t.parse().map_err(|_| bad("`track=` needs a number"))?);
                    rest = r.trim();
                }
                // `color=` only with a real colour after it: a list from before colours may
                // have a name like "color=red Ana", which stays a name.
                if let Some(r) = rest.strip_prefix("color=") {
                    let (c, r) = r.split_once(' ').unwrap_or((r, ""));
                    if let Some(c) = parse_color(c) {
                        color = Some(c);
                        rest = r.trim();
                    }
                }
                let name = rest;
                e.speakers.names.push(SpeakerName {
                    id,
                    // A blank name is read as the default rather than refused: it is what a
                    // half-finished hand rename leaves.
                    name: if name.is_empty() {
                        default_name(id)
                    } else {
                        name.to_string()
                    },
                    track,
                    color: color.unwrap_or_else(|| default_color(id)),
                });
            }
            "captions" => {
                e.captions = Some(captions_line::parse(rest).map_err(|why| bad(&why))?);
            }
            "fix" => {
                let mut parts = rest.splitn(3, ' ');
                let track = parts.next().and_then(|t| t.parse::<usize>().ok());
                let at = parts.next().and_then(word_at);
                let text = check_fix(parts.next().unwrap_or("")).map_err(|why| bad(&why))?;
                let expected = "expected `fix <track> <start>[+<n>] <text>`";
                let (Some(track), Some(at)) = (track, at) else {
                    return Err(bad(expected));
                };
                if text.is_empty() {
                    return Err(bad(expected));
                }
                let fix = Fix {
                    track,
                    start: at.start,
                    nth: at.nth,
                    through: None,
                    text,
                };
                add_fix(&mut e.fixes, fix).map_err(bad)?;
            }
            "span" => {
                let expected =
                    "expected `span <track> <first start>[+<n>] <last start>[+<n>] <text>`";
                let mut parts = rest.splitn(4, ' ');
                let track = parts.next().and_then(|t| t.parse::<usize>().ok());
                let first = parts.next().and_then(word_at);
                let last = parts.next().and_then(word_at);
                let text = check_span(parts.next().unwrap_or("")).map_err(|why| bad(&why))?;
                let (Some(track), Some(first), Some(last)) = (track, first, last) else {
                    return Err(bad(expected));
                };
                if text.is_empty() {
                    return Err(bad(expected));
                }
                if !fixes::after(&last, &first) {
                    return Err(bad("a span's last word is to come after its first"));
                }
                let fix = Fix {
                    track,
                    start: first.start,
                    nth: first.nth,
                    through: Some(last),
                    text,
                };
                add_fix(&mut e.fixes, fix).map_err(bad)?;
            }
            "time" => {
                // Written only into a version 6 list: one under an older header is not a list
                // Junctur wrote, and the build that wrote that version refused the line.
                if read_version < 6 {
                    return Err(bad("a `time` line needs a `junctur-edit 6` list"));
                }
                let expected = "expected `time <track> <start>[+<n>] <new start> <new end>`";
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let [track, at, from, to] = parts[..] else {
                    return Err(bad(expected));
                };
                let num = |x: &str| x.parse::<f64>().ok().filter(|x| x.is_finite());
                let (Ok(track), Some(at), Some(from), Some(to)) =
                    (track.parse::<usize>(), word_at(at), num(from), num(to))
                else {
                    return Err(bad(expected));
                };
                if to <= from {
                    return Err(bad("a retimed word's end is to come after its start"));
                }
                let t = Retime {
                    track,
                    start: at.start,
                    nth: at.nth,
                    from: times::to_ms(from),
                    to: times::to_ms(to),
                };
                if e.times.iter().any(|x| {
                    x.track == t.track
                        && fixes::same_millisecond(x.start, t.start)
                        && x.nth == t.nth
                }) {
                    return Err(bad("a second `time` line for this word"));
                }
                e.times.push(t);
            }
            "turn" => match nums()?[..] {
                [start, end, id] if end > start && id >= 1.0 && id.fract() == 0.0 => {
                    e.speakers.turns.push(Turn {
                        start,
                        end,
                        speaker: id as u32,
                    });
                    turn_lines.push(n);
                }
                _ => {
                    return Err(bad(
                        "expected `turn <start> <end> <speaker id>` with end after start",
                    ))
                }
            },
            "window" => match nums()?[..] {
                [start, score, speech, words] => e.windows.push(Window {
                    start,
                    score,
                    speech,
                    words: words as u32,
                }),
                _ => return Err(bad("expected `window <start> <score> <speech> <words>`")),
            },
            _ => return Err(bad("unknown line")),
        }
    }
    for (t, n) in e.speakers.turns.iter().zip(&turn_lines) {
        if e.speakers.name(t.speaker).is_none() {
            return Err(format!(
                "line {n}: turn names speaker {}, which no `speaker` line declares",
                t.speaker
            ));
        }
    }
    e.speakers.names.sort_by_key(|n| n.id);
    fixes::sort(&mut e.fixes);
    times::sort(&mut e.times);
    e.speakers.turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    // Hand edits may leave ranges out of order or overlapping; the renderer wants neither.
    e.keep.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut merged: Vec<Range> = Vec::new();
    for r in e.keep {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    e.keep = merged;
    Ok(e)
}

/// A word as a `fix` or `span` line names it: `<start>` or `<start>+<n>`, a bare start being
/// the millisecond's first word. Split at the last `+` so an exponent's sign (`1e+1`) stays the
/// start's.
fn word_at(t: &str) -> Option<WordAt> {
    let (start, nth) = match t.rsplit_once('+') {
        Some((s, n)) if !s.ends_with(['e', 'E']) && !s.is_empty() => (s, n.parse::<usize>().ok()?),
        _ => (t, 0),
    };
    let start = start.parse::<f64>().ok().filter(|s| s.is_finite())?;
    Some(WordAt { start, nth })
}

/// `fix` added to the list read so far, refused when it shares a word with a fix already there: a
/// second line for one word, or a span over a fixed word or another span, would give the word two
/// texts.
fn add_fix(fixes: &mut Vec<Fix>, fix: Fix) -> Result<(), &'static str> {
    match fixes.iter().find(|f| f.overlaps(&fix)) {
        Some(f) if !f.is_span() && !fix.is_span() => Err("a second `fix` line for this word"),
        Some(_) => Err("a `span` over words another `fix` or `span` line fixes"),
        None => {
            fixes.push(fix);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::Captions;

    fn sample() -> EditList {
        EditList {
            source: PathBuf::from(r"C:\Videos\my stream.mp4"),
            duration: 100.0,
            settings: CutSettings::default(),
            keep: vec![
                Range {
                    start: 1.5,
                    end: 4.25,
                },
                Range {
                    start: 10.0,
                    end: 12.0,
                },
            ],
            speakers: Speakers::default(),
            captions: None,
            fixes: Vec::new(),
            times: Vec::new(),
            windows: vec![Window {
                start: 0.0,
                score: 0.5,
                speech: 0.5,
                words: 7,
            }],
        }
    }

    fn with_speakers() -> EditList {
        EditList {
            speakers: Speakers {
                names: vec![
                    SpeakerName {
                        id: 1,
                        name: "Speaker 1".into(),
                        track: None,
                        color: "#F2B33D".into(),
                    },
                    SpeakerName {
                        id: 2,
                        name: "Lucía de la Cruz".into(),
                        track: Some(1),
                        color: "#123ABC".into(),
                    },
                ],
                turns: vec![
                    Turn {
                        start: 1.5,
                        end: 3.0,
                        speaker: 1,
                    },
                    Turn {
                        start: 2.5,
                        end: 4.0,
                        speaker: 2,
                    },
                ],
            },
            ..sample()
        }
    }

    #[test]
    fn speakers_round_trip_and_stay_out_of_lists_without_them() {
        let e = with_speakers();
        let text = e.to_text();
        assert!(text.starts_with("junctur-edit 2\n"), "{text}");
        assert!(
            text.contains("speaker 2 track=1 color=#123ABC Lucía de la Cruz\n"),
            "{text}"
        );
        assert!(text.contains("turn 2.500 4.000 2\n"));
        assert!(
            !text.contains("captions"),
            "a list without its own captions writes none"
        );
        assert_eq!(parse(&text).unwrap(), e);
        let plain = sample().to_text();
        assert!(!plain.contains("speaker"));
        assert!(plain.starts_with("junctur-edit 1\n"));
        let newer = format!("junctur-edit {}\n", FORMAT_VERSION + 1);
        assert!(parse(&newer).is_err());
        // Caption settings alone make a version-2 list, and round-trip.
        let c = EditList {
            captions: Some(Captions {
                on: true,
                chars: 32,
                lines: 1,
                seconds: 4.5,
                ..Captions::default()
            }),
            ..sample()
        };
        let text = c.to_text();
        assert!(text.starts_with("junctur-edit 2\n"));
        assert!(
            text.contains("captions on chars=32 lines=1 seconds=4.5\n"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap(), c);
        // Own settings equal to the built-in default are still the list's own: written, and
        // read back as set rather than as following the user's defaults.
        let own = EditList {
            captions: Some(Captions::default()),
            ..sample()
        };
        let text = own.to_text();
        assert!(text.starts_with("junctur-edit 2\n"), "{text}");
        assert!(
            text.contains("captions off chars=42 lines=2 seconds=6\n"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap().captions, Some(Captions::default()));
    }

    #[test]
    fn hand_edited_colours_and_captions_are_read_or_refused() {
        let head = "junctur-edit 2\nsource x.mp4\n";
        let e = parse(&format!(
            "{head}speaker 1 color=#a0b0c0 Ana\nspeaker 2 track=3 Bo\ncaptions off\n"
        ))
        .unwrap();
        assert_eq!(e.speakers.color(1), Some("#A0B0C0"));
        assert_eq!(e.speakers.color(2), Some(default_color(2).as_str()));
        assert_eq!(e.speakers.name(2), Some("Bo"));
        assert_eq!(e.captions, Some(Captions::default()));
        // Not a colour: part of the name, as an older list would have it.
        let old = parse(&format!("{head}speaker 1 color=red Ana\n")).unwrap();
        assert_eq!(old.speakers.name(1), Some("color=red Ana"));
        assert_eq!(old.speakers.color(1), Some(default_color(1).as_str()));
        let back = parse(&old.to_text()).unwrap();
        assert_eq!(back.speakers, old.speakers);
        assert!(parse(&format!("{head}captions on chars=42.9\n")).is_err());
        assert!(parse(&format!("{head}captions maybe\n")).is_err());
        assert!(parse(&format!("{head}captions on chars=500\n")).is_err());
        assert!(parse(&format!("{head}captions on shadow=3\n")).is_err());
        // `size`: 3 % is in range, 1 % is not.
        assert!(parse(&format!("{head}captions on size=3\n")).is_ok());
        assert!(parse(&format!("{head}captions on size=1\n")).is_err());
        assert!(parse(&format!("{head}captions on bold=yes\n")).is_err());
        assert!(parse(&format!("{head}captions on underline=yes\n")).is_err());
    }

    #[test]
    fn caption_switches_and_styles_round_trip() {
        // Each written only when on, after the cue settings; off by hand reads as off.
        let c = EditList {
            captions: Some(Captions {
                preview: true,
                italic: true,
                underline: true,
                ..Captions::default()
            }),
            ..sample()
        };
        let text = c.to_text();
        assert!(
            text.contains(
                "captions off chars=42 lines=2 seconds=6 preview=on italic=on underline=on\n"
            ),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap(), c);
        let head = "junctur-edit 2\nsource x.mp4\n";
        let e = parse(&format!("{head}captions on bold=on preview=off\n")).unwrap();
        let c = e.captions.unwrap();
        assert!(c.on && c.bold && !c.preview && !c.italic && !c.underline);
        let e = parse(&format!("{head}captions on underline=off\n")).unwrap();
        assert!(!e.captions.unwrap().underline);
    }

    #[test]
    fn hand_edited_speakers_are_read_or_refused_by_line() {
        let head = "junctur-edit 2\nsource x.mp4\n";
        // A rename by hand, a blank name, turns before their speakers and out of order.
        let e = parse(&format!(
            "{head}turn 5 6 2\nturn 1 2 1\nspeaker 2   Ana  \nspeaker 1\n"
        ))
        .unwrap();
        assert_eq!(e.speakers.name(1), Some("Speaker 1"));
        assert_eq!(e.speakers.name(2), Some("Ana"));
        assert_eq!(e.speakers.turns[0].speaker, 1);
        let err = parse(&format!("{head}speaker 1 A\nturn 1 2 3\n")).unwrap_err();
        assert!(
            err.starts_with("line 4") && err.contains("speaker 3"),
            "{err}"
        );
        assert!(parse(&format!("{head}speaker 0 A\n")).is_err());
        assert!(parse(&format!("{head}speaker 1 A\nspeaker 1 B\n")).is_err());
        assert!(parse(&format!("{head}speaker 1 track=x A\n")).is_err());
        assert!(parse(&format!("{head}speaker 1 A\nturn 2 1 1\n")).is_err());
    }

    #[test]
    fn fixed_words_round_trip_as_version_3_and_bad_lines_are_refused() {
        let e = EditList {
            fixes: vec![
                Fix {
                    track: 0,
                    start: 2.5,
                    nth: 0,
                    through: None,
                    text: "Tsun Heto,".into(),
                },
                Fix {
                    track: 1,
                    start: 1.25,
                    nth: 0,
                    through: None,
                    text: "Uezar".into(),
                },
            ],
            ..with_speakers()
        };
        let text = e.to_text();
        assert!(text.starts_with("junctur-edit 3\n"), "{text}");
        assert!(
            text.contains("fix 1 1.250 Uezar\nfix 0 2.500 Tsun Heto,\n"),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back.fixes[0].text, "Uezar", "in time order");
        assert_eq!(back.fixes.len(), 2);
        assert_eq!(parse(&back.to_text()).unwrap(), back);
        // Without fixes a list is the version it was.
        assert!(sample().to_text().starts_with("junctur-edit 1\n"));
        // A start half a millisecond in survives the round trip (written as its key's millisecond).
        let half = EditList {
            fixes: vec![Fix {
                track: 0,
                start: 12.3125,
                nth: 0,
                through: None,
                text: "Uezar".into(),
            }],
            ..sample()
        };
        let text = half.to_text();
        assert!(text.contains("fix 0 12.313 Uezar\n"), "{text}");
        let back = parse(&text).unwrap();
        assert!(back.fixes[0].same_word(&half.fixes[0]));
        let head = "junctur-edit 3\nsource x.mp4\n";
        let e = parse(&format!("{head}fix 2 10.5   Delay  \n")).unwrap();
        assert_eq!(e.fixes[0].text, "Delay");
        for bad in [
            "fix 1 2.000\n",
            "fix x 2.000 Ana\n",
            "fix 1 soon Ana\n",
            "fix -1 2.000 Ana\n",
            "fix 1 2.000 Ana\nfix 1 2.0001 Bo\n",
            "fix 1 2.000+ Ana\n",
            "fix 1 2.000+x Ana\n",
            "fix 1 2.000+-1 Ana\n",
            "fix 1 2.000+1 Ana\nfix 1 2.000+1 Bo\n",
        ] {
            assert!(parse(&format!("{head}{bad}")).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_later_word_of_one_millisecond_is_written_with_its_place_as_version_4() {
        // "en" and "él" both start at 54.944; each word its own fix.
        let e = EditList {
            fixes: vec![
                Fix {
                    track: 0,
                    start: 54.944,
                    nth: 1,
                    through: None,
                    text: "miel".into(),
                },
                Fix {
                    track: 0,
                    start: 54.944,
                    nth: 0,
                    through: None,
                    text: "de".into(),
                },
            ],
            ..sample()
        };
        let text = e.to_text();
        assert!(text.starts_with("junctur-edit 4\n"), "{text}");
        assert!(
            text.contains("fix 0 54.944 de\nfix 0 54.944+1 miel\n"),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back.fixes, [e.fixes[1].clone(), e.fixes[0].clone()]);
        assert!(!back.fixes[0].same_word(&back.fixes[1]));
        assert_eq!(back.to_text(), text);
        // A bare line names the first word of its millisecond and is written back byte for byte,
        // still version 3.
        let old = EditList {
            fixes: vec![Fix {
                track: 0,
                start: 54.944,
                nth: 0,
                through: None,
                text: "mel".into(),
            }],
            ..sample()
        }
        .to_text();
        assert!(old.starts_with("junctur-edit 3\n"), "{old}");
        assert!(old.contains("\nfix 0 54.944 mel\n"), "{old}");
        let back = parse(&old).unwrap();
        assert_eq!(back.fixes[0].nth, 0);
        assert_eq!(back.to_text(), old);
        // An exponent's sign is the start's, not a place.
        let e = parse("junctur-edit 4\nsource x.mp4\nfix 0 1e+1 Ana\nfix 0 1E+1+2 Bo\n").unwrap();
        assert_eq!((e.fixes[0].start, e.fixes[0].nth), (10.0, 0));
        assert_eq!((e.fixes[1].start, e.fixes[1].nth), (10.0, 2));
    }

    #[test]
    fn a_span_is_its_own_line_and_makes_the_list_version_5() {
        // A run of words fixed as one text.
        let at = |start, nth| WordAt { start, nth };
        let e = EditList {
            fixes: vec![
                Fix {
                    track: 1,
                    start: 54.944,
                    nth: 1,
                    through: Some(at(56.0, 2)),
                    text: "Uezar went home.".into(),
                },
                Fix {
                    track: 0,
                    start: 2.5,
                    nth: 0,
                    through: None,
                    text: "Ana".into(),
                },
            ],
            ..sample()
        };
        let text = e.to_text();
        assert!(text.starts_with("junctur-edit 5\n"), "{text}");
        assert!(
            text.contains("fix 0 2.500 Ana\nspan 1 54.944+1 56.000+2 Uezar went home.\n"),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back.fixes, [e.fixes[1].clone(), e.fixes[0].clone()]);
        assert_eq!(back.to_text(), text);
        // Without a span the list is written at the older version.
        let plain = EditList {
            fixes: vec![e.fixes[1].clone()],
            ..sample()
        };
        assert!(plain.to_text().starts_with("junctur-edit 3\n"));
        assert!(!plain.to_text().contains("span"));
        // Refused: a span whose last word is not after its first, one over a fixed word or another
        // span, a missing text or word.
        let head = "junctur-edit 5\nsource x.mp4\n";
        for bad in [
            "span 0 3.000 2.000 x\n",
            "span 0 2.000 2.000 x\n",
            "span 0 2.000+1 2.000 x\n",
            "fix 0 2.500 Ana\nspan 0 2.000 3.000 x\n",
            "span 0 2.000 3.000 x\nfix 0 3.000 Ana\n",
            "span 0 2.000 3.000 x\nspan 0 3.000 4.000 y\n",
            "span 0 2.000 3.000\n",
            "span 0 2.000 x y\n",
        ] {
            assert!(parse(&format!("{head}{bad}")).is_err(), "{bad}");
        }
        // Touching but not sharing a word, or on another track, is fine.
        let ok = parse(&format!(
            "{head}span 0 2.000 3.000 x\nspan 0 3.000+1 4.000 y\nspan 1 2.000 4.000 z\nfix 0 4.001 w\n"
        ))
        .unwrap();
        assert_eq!(ok.fixes.len(), 4);
    }

    #[test]
    fn a_retimed_word_is_a_time_line_and_makes_the_list_version_6() {
        // A word's start and end moved with a cut's edge, named by its heard start.
        let t = |track, start, nth, from, to| Retime {
            track,
            start,
            nth,
            from,
            to,
        };
        let e = EditList {
            times: vec![t(1, 54.944, 1, 54.95, 55.3), t(0, 2.5, 0, 2.5, 2.9)],
            ..sample()
        };
        let text = e.to_text();
        assert!(text.starts_with("junctur-edit 6\n"), "{text}");
        assert!(
            text.contains("time 0 2.500 2.500 2.900\ntime 1 54.944+1 54.950 55.300\n"),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back.times, [e.times[1], e.times[0]]);
        assert_eq!(back.to_text(), text);
        // Every older version reads as before, with no times, and is written back byte for byte.
        let older = [
            sample(),
            EditList {
                fixes: vec![Fix {
                    track: 0,
                    start: 2.5,
                    nth: 1,
                    through: Some(WordAt { start: 3.0, nth: 0 }),
                    text: "Ana went".into(),
                }],
                ..sample()
            },
        ];
        for old in older {
            let text = old.to_text();
            assert!(!text.contains("time "), "{text}");
            let back = parse(&text).unwrap();
            assert!(back.times.is_empty());
            assert_eq!(back.to_text(), text);
        }
        // Refused: a second line for one word, an end not after its start, a missing number.
        let head = "junctur-edit 6\nsource x.mp4\n";
        for bad in [
            "time 0 2.000 2.100 2.000\n",
            "time 0 2.000 2.100\n",
            "time 0 2.000 2.100 x\n",
            "time x 2.000 2.100 2.300\n",
            "time 0 2.000 2.1 2.3\ntime 0 2.0004 2.0 2.4\n",
        ] {
            assert!(parse(&format!("{head}{bad}")).is_err(), "{bad}");
        }
        let ok = parse(&format!(
            "{head}time 0 2.000 2.1 2.3\ntime 0 2.000+1 2.3 2.4\n"
        ))
        .unwrap();
        assert_eq!(ok.times.len(), 2);
        // Two tracks' words of one millisecond keep their order through a round trip.
        let one_ms = EditList {
            times: vec![t(1, 5.0001, 0, 5.0, 5.2), t(0, 5.0004, 0, 5.0, 5.3)],
            ..sample()
        };
        let text = one_ms.to_text();
        assert!(
            text.contains("time 0 5.000 5.000 5.300\ntime 1 5.000 5.000 5.200\n"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap().to_text(), text);
        // A `time` line under an older header is refused, and so is a version this build does
        // not know.
        assert!(parse("junctur-edit 5\nsource x.mp4\ntime 0 2.000 2.1 2.3\n").is_err());
        assert!(parse("junctur-edit 7\nsource x.mp4\n").is_err());
    }

    #[test]
    fn round_trips() {
        let e = sample();
        let text = e.to_text();
        assert!(text.contains("keep 1.500 4.250\n"));
        assert_eq!(parse(&text).unwrap(), e);
    }

    #[test]
    fn hand_edits_are_normalised() {
        let text = "junctur-edit 1\n# note\nsource x.mp4\nkeep 5 8\nkeep 1 2\nkeep 7 9\n";
        let e = parse(text).unwrap();
        assert_eq!(
            e.keep,
            vec![
                Range {
                    start: 1.0,
                    end: 2.0
                },
                Range {
                    start: 5.0,
                    end: 9.0
                }
            ]
        );
    }

    #[test]
    fn errors_name_the_line() {
        assert!(parse("nope").is_err());
        let err = parse("junctur-edit 1\nkeep 3 1\n").unwrap_err();
        assert!(err.starts_with("line 2"), "{err}");
        assert!(parse("junctur-edit 1\nfoo 1\n").is_err());
    }
}
