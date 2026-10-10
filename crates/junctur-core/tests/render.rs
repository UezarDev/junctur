//! Smart render end to end on generated clips: frame count, audio tracks, copy vs fallback.
//! Ignored by default: it needs an NVIDIA GPU (NVENC/NVDEC). Run with `cargo test -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

use junctur_core::edit::{CutSettings, EditList, Range};
use junctur_core::render::{self, RenderOptions};

fn workdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("junctur-render-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 20 s, 30 fps, keyframe every second, B-frames, two AAC tracks.
fn clip(path: &Path, codec: &str) {
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i"])
        .args([
            "testsrc2=d=20:s=640x360:r=30",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=20",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=f=660:d=20",
            "-map",
            "0:v",
            "-map",
            "1:a",
        ])
        .args([
            "-map", "2:a", "-c:v", codec, "-g", "30", "-bf", "2", "-pix_fmt", "yuv420p",
        ])
        .args(["-c:a", "aac"])
        .arg(path)
        .status()
        .expect("ffmpeg on PATH")
        .success();
    assert!(ok);
}

fn edit(source: &Path) -> EditList {
    EditList {
        source: source.to_path_buf(),
        duration: 20.0,
        settings: CutSettings::default(),
        keep: vec![
            Range {
                start: 1.1,
                end: 7.3,
            },
            Range {
                start: 10.0,
                end: 15.55,
            },
        ],
        windows: vec![],
    }
}

fn count(path: &Path, select: &str, entry: &str) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", select, "-count_packets"])
        .args(["-show_entries", entry, "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
#[ignore = "needs NVENC"]
fn h264_is_smart_rendered_frame_exact() {
    let d = workdir("h264");
    let src = d.join("in.mp4");
    clip(&src, "libx264");
    let out = d.join("out.mp4");
    let r = render::render(&edit(&src), &out, &RenderOptions::default(), &mut |_| {}).unwrap();
    assert_eq!(r.full_reason, None);
    assert!(r.frames_copied > 0 && r.frames_encoded > 0);
    // Frames 33..219 and 300..467: 186 + 167.
    assert_eq!(r.frames_copied + r.frames_encoded, 353);
    assert_eq!(count(&out, "v:0", "stream=nb_read_packets"), "353");
    assert_eq!(count(&out, "a", "stream=index").lines().count(), 2);
    // Never over the source; not over an existing file without asking.
    assert!(render::render(&edit(&src), &src, &RenderOptions::default(), &mut |_| {}).is_err());
    assert!(render::render(&edit(&src), &out, &RenderOptions::default(), &mut |_| {}).is_err());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
#[ignore = "needs NVENC"]
fn hevc_falls_back_to_a_full_encode_and_says_why() {
    let d = workdir("hevc");
    let src = d.join("in.mp4");
    clip(&src, "libx265");
    let out = d.join("out.mp4");
    let r = render::render(&edit(&src), &out, &RenderOptions::default(), &mut |_| {}).unwrap();
    assert!(r.full_reason.unwrap().contains("hevc"));
    assert_eq!(r.frames_copied, 0);
    assert_eq!(count(&out, "v:0", "stream=nb_read_packets"), "353");
    std::fs::remove_dir_all(&d).unwrap();
}
