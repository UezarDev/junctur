//! The final mux: the video pieces arrive concatenated on stdin as one MPEG-TS stream, each audio
//! track is a finished AAC file, and everything is copied into the output, with the source opened
//! once more (not read) for its metadata: track titles and languages.

use std::path::{Path, PathBuf};

/// The muxing ffmpeg's arguments for `tracks` (in source order) into `out`.
pub fn args(source: &Path, tracks: &[PathBuf], out: &Path) -> Vec<String> {
    let n = tracks.len();
    let mut a: Vec<String> = ["-v", "error", "-y", "-f", "mpegts", "-i", "-"]
        .map(String::from)
        .to_vec();
    for t in tracks {
        a.push("-i".into());
        a.push(t.to_string_lossy().into_owned());
    }
    a.push("-i".into());
    a.push(source.to_string_lossy().into_owned());
    a.extend(["-map".into(), "0:v:0".into()]);
    for i in 0..n {
        a.extend(["-map".into(), format!("{}:a:0", i + 1)]);
        a.extend([
            format!("-map_metadata:s:a:{i}"),
            format!("{}:s:a:{i}", n + 1),
        ]);
    }
    a.extend([
        "-map_metadata".into(),
        (n + 1).to_string(),
        "-c".into(),
        "copy".into(),
    ]);
    a.push(out.to_string_lossy().into_owned());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_video_tracks_and_source_metadata() {
        let tracks = [PathBuf::from("a0.m4a"), PathBuf::from("a1.m4a")];
        let a = args(Path::new("in.mp4"), &tracks, Path::new("out.mp4"));
        let s = a.join(" ");
        assert!(s.contains("-i - -i a0.m4a -i a1.m4a -i in.mp4"), "{s}");
        assert!(
            s.contains("-map 0:v:0 -map 1:a:0 -map_metadata:s:a:0 3:s:a:0"),
            "{s}"
        );
        assert!(s.contains("-map 2:a:0 -map_metadata:s:a:1 3:s:a:1"), "{s}");
        assert!(s.ends_with("-map_metadata 3 -c copy out.mp4"), "{s}");
    }
}
