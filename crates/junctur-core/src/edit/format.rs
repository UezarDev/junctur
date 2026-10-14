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
    e.keep = crate::spans::joined(e.keep, crate::spans::Join::Touching);
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
mod tests;
