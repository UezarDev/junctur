//! The edit list's text form. One fact per line, so a git diff of a hand edit is one line:
//!
//! ```text
//! junctur-edit 1
//! source C:\Videos\stream.mp4
//! duration 6304.021
//! settings pad_before=0.25 pad_after=0.35 merge_gap=1 snap=0.15 crossfade=0.03 window=5
//! # kept 4012.3 s of 6304.0 s (64%) in 812 ranges
//! keep 1.234 5.678
//! window 0 0.820 0.820 14
//! ```
//!
//! `#` lines are comments. `source` takes the rest of the line, so paths need no quoting.
//! Times are seconds with millisecond precision.

use std::fmt::Write;
use std::path::PathBuf;

use super::{CutSettings, EditList, Range, Window};

pub const FORMAT_VERSION: u32 = 1;

pub fn write(e: &EditList) -> String {
    let s = &e.settings;
    let mut out = String::new();
    let _ = writeln!(out, "junctur-edit {FORMAT_VERSION}");
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
    match lines.next() {
        Some((_, l)) if l == format!("junctur-edit {FORMAT_VERSION}") => {}
        _ => return Err(format!("not a junctur-edit {FORMAT_VERSION} file")),
    }
    let mut e = EditList {
        source: PathBuf::new(),
        duration: 0.0,
        settings: CutSettings::default(),
        keep: Vec::new(),
        windows: Vec::new(),
    };
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
            windows: vec![Window {
                start: 0.0,
                score: 0.5,
                speech: 0.5,
                words: 7,
            }],
        }
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
