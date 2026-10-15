//! The kept ranges as an mpv EDL (`edl://`), one segment per range. mpv plays it gapless, decoding
//! the next range ahead, so skipping cuts does not stall on long-GOP recordings as seeking would.
//! In that timeline mpv's `time-pos` is output time.

use super::timeline::{Timeline, EDL_DECIMALS};

/// `edl://` for `file` playing only `t`'s kept ranges. `t` must keep something. The segments'
/// times come from the timeline already rounded, so mpv's timeline and the app's mapping agree
/// (see `timeline`).
pub fn url(file: &str, t: &Timeline) -> String {
    // `%<bytes>%` escapes the path, so commas, semicolons or non-ASCII in it need no quoting.
    let escaped = format!("%{}%{}", file.len(), file);
    let mut s = String::with_capacity(16 + t.ranges.len() * (escaped.len() + 24));
    s.push_str("edl://!no_chapters;");
    for &(start, length) in t.segments() {
        let d = EDL_DECIMALS;
        s.push_str(&format!("{escaped},{start:.d$},{length:.d$};"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::super::timeline::{Fps, Timeline};
    use super::*;
    use junctur_core::edit::Range;

    #[test]
    fn one_segment_per_range_with_escaped_path() {
        let keep = [
            Range {
                start: 0.0,
                end: 1.0,
            },
            Range {
                start: 5.0,
                end: 7.5,
            },
        ];
        let t = Timeline::new(&keep, 10.0, Fps { num: 30, den: 1 });
        let file = r"C:\Vídeos\a, b;c.mp4";
        let u = url(file, &t);
        let n = file.len();
        assert_eq!(n, 21, "length in bytes, not chars");
        assert_eq!(
            u,
            format!(
                "edl://!no_chapters;%{n}%{file},0.000000,1.000000;%{n}%{file},4.999900,2.500000;"
            )
        );
    }
}
