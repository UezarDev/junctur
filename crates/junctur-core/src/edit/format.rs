//! The edit list's text form. One fact per line, so a git diff of a hand edit is one line:
//!
//! ```text
//! junctur-edit 2
//! source C:\Videos\stream.mp4
//! duration 6304.021
//! settings pad_before=0.25 pad_after=0.35 merge_gap=1 snap=0.15 crossfade=0.03 window=5
//! # kept 4012.3 s of 6304.0 s (64%) in 812 ranges
//! keep 1.234 5.678
//! captions on chars=42 lines=2 seconds=6
//! speaker 1 color=#F2B33D Speaker 1
//! speaker 2 track=1 color=#5FB3F2 Lucía
//! turn 1.300 4.100 1
//! window 0 0.820 0.820 14
//! ```
//!
//! `#` lines are comments. `source` takes the rest of the line, so paths need no quoting, and so
//! does a speaker's name (after its id and the optional `track=<n>` and `color=#RRGGBB`), so
//! renaming or recolouring a speaker by hand is editing one line. Times are seconds with
//! millisecond precision. Speaker and turn lines are written only when there are speakers, the
//! `captions` line only when it is not the default, and only then is the header `junctur-edit 2`
//! (5a): a list without them is byte-for-byte what an older build wrote and still reads there,
//! while a list with them tells an older build it is a newer format instead of failing on its
//! first new line. Both versions read here.

use std::fmt::Write;
use std::path::PathBuf;

use super::{
    default_color, default_name, parse_color, Captions, CutSettings, EditList, Range, SpeakerName,
    Speakers, Turn, Window,
};

/// The newest version this build writes (with speakers); lists without speakers are written as 1.
pub const FORMAT_VERSION: u32 = 2;

pub fn write(e: &EditList) -> String {
    let s = &e.settings;
    let mut out = String::new();
    let version = if e.speakers.is_empty() && e.captions == Captions::default() {
        1
    } else {
        FORMAT_VERSION
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
    let c = &e.captions;
    if *c != Captions::default() {
        let _ = writeln!(
            out,
            "# captions <on|off> chars=<per line> lines=<per speaker> seconds=<longest> \
             [preview=on] [bold=on] [italic=on]"
        );
        // 5d's switches only when on, so a list that does not use them reads in 5a's builds.
        let flags: String = [
            ("preview", c.preview),
            ("bold", c.bold),
            ("italic", c.italic),
        ]
        .iter()
        .filter(|(_, on)| *on)
        .map(|(k, _)| format!(" {k}=on"))
        .collect();
        let _ = writeln!(
            out,
            "captions {} chars={} lines={} seconds={}{flags}",
            if c.on { "on" } else { "off" },
            c.chars,
            c.lines,
            c.seconds
        );
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
    match lines.next() {
        Some((_, l)) if version(l).is_some_and(|v| (1..=FORMAT_VERSION).contains(&v)) => {}
        _ => {
            return Err(format!(
                "not a junctur-edit file this build reads (versions 1 to {FORMAT_VERSION})"
            ))
        }
    }
    let mut e = EditList {
        source: PathBuf::new(),
        duration: 0.0,
        settings: CutSettings::default(),
        keep: Vec::new(),
        speakers: Speakers::default(),
        captions: Captions::default(),
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
                let mut c = Captions::default();
                let mut parts = rest.split_whitespace();
                c.on = match parts.next() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err(bad("expected `captions <on|off> ...`")),
                };
                for kv in parts {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| bad("expected key=value"))?;
                    let whole = || v.parse::<u32>().map_err(|_| bad("not a whole number"));
                    let flag = || match v {
                        "on" => Ok(true),
                        "off" => Ok(false),
                        _ => Err(bad("expected on or off")),
                    };
                    match k {
                        "chars" => c.chars = whole()?,
                        "lines" => c.lines = whole()?,
                        "seconds" => c.seconds = v.parse().map_err(|_| bad("not a number"))?,
                        "preview" => c.preview = flag()?,
                        "bold" => c.bold = flag()?,
                        "italic" => c.italic = flag()?,
                        _ => return Err(bad("unknown caption setting")),
                    }
                }
                c.check().map_err(|why| bad(&why))?;
                e.captions = c;
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

#[cfg(test)]
mod tests {
    use super::*;

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
            captions: Captions::default(),
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
            "default captions are not written"
        );
        assert_eq!(parse(&text).unwrap(), e);
        let plain = sample().to_text();
        assert!(!plain.contains("speaker"));
        assert!(plain.starts_with("junctur-edit 1\n"));
        assert!(parse("junctur-edit 3\n").is_err());
        // Caption settings alone make a version-2 list, and round-trip.
        let c = EditList {
            captions: Captions {
                on: true,
                chars: 32,
                lines: 1,
                seconds: 4.5,
                ..Captions::default()
            },
            ..sample()
        };
        let text = c.to_text();
        assert!(text.starts_with("junctur-edit 2\n"));
        assert!(
            text.contains("captions on chars=32 lines=1 seconds=4.5\n"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap(), c);
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
        assert_eq!(e.captions, Captions::default());
        // Not a colour: part of the name, as an older list would have it.
        let old = parse(&format!("{head}speaker 1 color=red Ana\n")).unwrap();
        assert_eq!(old.speakers.name(1), Some("color=red Ana"));
        assert_eq!(old.speakers.color(1), Some(default_color(1).as_str()));
        let back = parse(&old.to_text()).unwrap();
        assert_eq!(back.speakers, old.speakers);
        assert!(parse(&format!("{head}captions on chars=42.9\n")).is_err());
        assert!(parse(&format!("{head}captions maybe\n")).is_err());
        assert!(parse(&format!("{head}captions on chars=500\n")).is_err());
        assert!(parse(&format!("{head}captions on size=3\n")).is_err());
        assert!(parse(&format!("{head}captions on bold=yes\n")).is_err());
    }

    #[test]
    fn caption_switches_and_styles_round_trip() {
        // Each written only when on, after 5a's settings; off by hand reads as off.
        let c = EditList {
            captions: Captions {
                preview: true,
                italic: true,
                ..Captions::default()
            },
            ..sample()
        };
        let text = c.to_text();
        assert!(
            text.contains("captions off chars=42 lines=2 seconds=6 preview=on italic=on\n"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap(), c);
        let head = "junctur-edit 2\nsource x.mp4\n";
        let e = parse(&format!("{head}captions on bold=on preview=off\n")).unwrap();
        assert!(e.captions.on && e.captions.bold && !e.captions.preview && !e.captions.italic);
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
