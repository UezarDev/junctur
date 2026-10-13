//! FCPXML 1.10 timeline for DaVinci Resolve: one asset (the untouched source) and one clip on the
//! spine per kept range, laid end to end.
//!
//! Times are whole frames written as rationals (`3003/30000s`): Resolve rejects or drifts on
//! times off the frame grid, so each range is widened to whole frames (start down, end up) and
//! ranges that then touch are merged. Crossfades are not written: Resolve's FCPXML import drops
//! audio transitions on asset clips, so fades belong to the renderer (smart render), not here.

use std::fmt::Write;
use std::path::Path;

use crate::edit::{frame_ranges, frames_ceil, EditList};
use crate::ffmpeg::VideoStream;

/// Used for the timeline when the source has no video.
const AUDIO_ONLY: VideoStream = VideoStream {
    width: 1920,
    height: 1080,
    fps_num: 30,
    fps_den: 1,
};

pub struct Media {
    pub video: Option<VideoStream>,
    pub audio_tracks: usize,
    pub audio_rate: u32,
}

pub fn write(edit: &EditList, media: &Media, name: &str) -> String {
    let v = media.video.unwrap_or(AUDIO_ONLY);
    let (num, den) = (v.fps_num, v.fps_den);
    let t = |frames: u64| {
        if frames == 0 {
            "0s".to_string()
        } else {
            format!("{}/{num}s", frames * den)
        }
    };
    let total = frames_ceil(edit.duration, num, den);
    let ranges = frame_ranges(&edit.keep, num, den, total);
    let seq_len: u64 = ranges.iter().map(|(a, b)| b - a).sum();
    let name = xml_escape(name);

    let mut x = String::new();
    let _ = writeln!(x, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(x, "<!DOCTYPE fcpxml>");
    let _ = writeln!(x, r#"<fcpxml version="1.10">"#);
    let _ = writeln!(x, "  <resources>");
    let _ = writeln!(
        x,
        r#"    <format id="r1" frameDuration="{den}/{num}s" width="{}" height="{}"/>"#,
        v.width, v.height
    );
    let _ = writeln!(
        x,
        r#"    <asset id="r2" name="{name}" start="0s" duration="{}" hasVideo="{}" hasAudio="{}" format="r1" audioSources="{}" audioChannels="2" audioRate="{}">"#,
        t(total),
        u8::from(media.video.is_some()),
        u8::from(media.audio_tracks > 0),
        media.audio_tracks,
        media.audio_rate
    );
    let _ = writeln!(
        x,
        r#"      <media-rep kind="original-media" src="{}"/>"#,
        file_url(&edit.source)
    );
    let _ = writeln!(x, "    </asset>");
    let _ = writeln!(x, "  </resources>");
    let _ = writeln!(x, r#"  <library>"#);
    let _ = writeln!(x, r#"    <event name="Junctur">"#);
    let _ = writeln!(x, r#"      <project name="{name} (Junctur)">"#);
    let _ = writeln!(
        x,
        r#"        <sequence format="r1" duration="{}" tcStart="0s" tcFormat="NDF" audioLayout="stereo" audioRate="48k">"#,
        t(seq_len)
    );
    let _ = writeln!(x, "          <spine>");
    let mut offset = 0;
    for (a, b) in ranges {
        let _ = writeln!(
            x,
            r#"            <asset-clip ref="r2" name="{name}" offset="{}" start="{}" duration="{}" format="r1" tcFormat="NDF"/>"#,
            t(offset),
            t(a),
            t(b - a)
        );
        offset += b - a;
    }
    let _ = writeln!(x, "          </spine>");
    let _ = writeln!(x, "        </sequence>");
    let _ = writeln!(x, "      </project>");
    let _ = writeln!(x, "    </event>");
    let _ = writeln!(x, "  </library>");
    let _ = writeln!(x, "</fcpxml>");
    x
}

/// `file:///C:/Videos/my%20stream.mp4`: forward slashes, everything but unreserved bytes escaped.
fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if p.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{CutSettings, Range};
    use std::path::PathBuf;

    fn edit(keep: Vec<Range>) -> EditList {
        EditList {
            source: PathBuf::from(r"C:\Videos\a & b.mp4"),
            duration: 10.0,
            settings: CutSettings::default(),
            keep,
            speakers: Default::default(),
            captions: Default::default(),
            fixes: Vec::new(),
            windows: vec![],
        }
    }

    fn r(start: f64, end: f64) -> Range {
        Range { start, end }
    }

    #[test]
    fn writes_clips_end_to_end() {
        let media = Media {
            video: Some(VideoStream {
                width: 1920,
                height: 1080,
                fps_num: 30,
                fps_den: 1,
            }),
            audio_tracks: 4,
            audio_rate: 48000,
        };
        let x = write(&edit(vec![r(1.0, 2.0), r(5.0, 5.5)]), &media, "a & b");
        assert!(x.contains(r#"frameDuration="1/30s""#));
        assert!(x.contains(r#"offset="0s" start="30/30s" duration="30/30s""#));
        assert!(x.contains(r#"offset="30/30s" start="150/30s" duration="15/30s""#));
        assert!(x.contains(r#"<sequence format="r1" duration="45/30s""#));
        assert!(x.contains("file:///C:/Videos/a%20%26%20b.mp4"));
        assert!(x.contains(r#"name="a &amp; b""#));
        assert!(x.contains(r#"audioSources="4""#));
    }

    #[test]
    fn empty_edit_is_an_empty_spine() {
        let media = Media {
            video: None,
            audio_tracks: 1,
            audio_rate: 48000,
        };
        let x = write(&edit(vec![]), &media, "x");
        assert!(!x.contains("<asset-clip"));
        assert!(x.contains(r#"hasVideo="0""#));
    }
}
