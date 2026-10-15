//! The SRT text of the cues.

use std::fmt::Write;

use super::Cue;
use crate::edit::{Captions, EditList};

/// `HH:MM:SS,mmm`.
pub(super) fn stamp(s: f64) -> String {
    let ms = (s.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// The SRT text: CRLF line ends (what Resolve's own export writes), each line in `<font color>` of
/// its part's speaker, closed per line since importers disagree on a tag spanning a break; a word
/// nobody was heard saying is left white. Parts follow line by line with no blank line between
/// (it would end the cue). Bold, italic and underline wrap each line outside its colour, in the
/// order Resolve's own export writes; the rest of a subtitle's look is Resolve's Track Style.
pub fn write(cues: &[Cue], edit: &EditList, c: &Captions) -> String {
    let style = c.style();
    let open: String = style.tags().map(|t| format!("<{t}>")).collect();
    let close: String = style.tags().rev().map(|t| format!("</{t}>")).collect();
    let mut out = String::new();
    for (n, c) in cues.iter().enumerate() {
        let _ = write!(
            out,
            "{}\r\n{} --> {}\r\n",
            n + 1,
            stamp(c.start),
            stamp(c.end)
        );
        for part in &c.parts {
            let color = edit.speakers.color(part.speaker);
            for line in &part.lines {
                match color {
                    Some(col) => {
                        let _ = write!(out, "{open}<font color=\"{col}\">{line}</font>{close}\r\n");
                    }
                    None => {
                        let _ = write!(out, "{open}{line}{close}\r\n");
                    }
                }
            }
        }
        out.push_str("\r\n");
    }
    out
}
