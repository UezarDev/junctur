//! The kept ranges as an mpv EDL (`edl://`), one segment per range of the same source. mpv plays
//! it as one gapless timeline: its timeline demuxer decodes the next range's lead-in from the
//! previous keyframe while the current range still plays, which is why skipping cuts this way does
//! not stall on long-GOP recordings (phase 2b measured 0 ms vs 100-300 ms for seeking at range
//! ends). In that timeline mpv's `time-pos` is output time.

use super::timeline::Timeline;

/// How far before its first frame a segment starts. mpv keeps frames with `start <= pts < start +
/// length`; starting exactly on the frame lets float rounding drop it (or let frame `end` in), so
/// every boundary sits a hair before its frame. Far below a frame and below an audio sample's
/// audibility (0.1 ms is ~5 samples at 48 kHz).
pub const BIAS: f64 = 1e-4;

/// `edl://` for `file` playing only `t`'s kept ranges. `t` must keep something.
pub fn url(file: &str, t: &Timeline) -> String {
    // `%<bytes>%` escapes the path, so commas, semicolons or non-ASCII in it need no quoting.
    let escaped = format!("%{}%{}", file.len(), file);
    let mut s = String::with_capacity(16 + t.ranges.len() * (escaped.len() + 24));
    s.push_str("edl://!no_chapters;");
    for &(a, b) in &t.ranges {
        let start = (t.fps.seconds(a) - BIAS).max(0.0);
        let length = t.fps.seconds(b - a);
        s.push_str(&format!("{escaped},{start:.6},{length:.6};"));
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
