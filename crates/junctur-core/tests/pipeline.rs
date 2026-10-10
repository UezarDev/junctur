//! The whole pipeline on a generated clip with no speech: every stage runs and is recorded, the
//! edit list is empty, the FCPXML is still a valid (empty) timeline, and a rerun reuses results.
//! Ignored by default: it downloads the VAD model into the user cache.

use std::process::Command;

use junctur_core::export;
use junctur_core::pipeline::{self, PipelineOptions, Project, StageState, STAGES};

#[test]
#[ignore = "downloads models"]
fn no_speech_gives_an_empty_edit_and_an_empty_timeline() {
    let d = std::env::temp_dir().join(format!("junctur-pipeline-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("game.mp4");
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i"])
        .args([
            "testsrc=d=20:s=320x240:r=30",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=20",
        ])
        .args(["-c:v", "libx264", "-c:a", "aac", "-shortest"])
        .arg(&f)
        .status()
        .unwrap()
        .success();
    assert!(ok);

    let p = Project::open(&f, None, Some(d.join("project"))).unwrap();
    let opts = PipelineOptions::default();
    let mut lines = Vec::new();
    let out = pipeline::run(&p, &opts, &mut |l| lines.push(l.to_string())).unwrap();
    assert!(out.edit.keep.is_empty());
    assert!(out.transcript.unwrap().segments.is_empty());
    let jobs = p.jobs();
    for s in STAGES {
        assert_eq!(jobs.get(s).unwrap().state, StageState::Done, "{s}");
    }

    // The edit file reads back, and exports.
    let e = p.edit().unwrap();
    assert_eq!(e.duration, out.edit.duration);
    let x = d.join("out.fcpxml");
    export::write_fcpxml(&e, &x).unwrap();
    let xml = std::fs::read_to_string(&x).unwrap();
    assert!(xml.contains(r#"frameDuration="1/30s""#) && !xml.contains("<asset-clip"));

    // A rerun reuses speech detection.
    lines.clear();
    pipeline::run(&p, &opts, &mut |l| lines.push(l.to_string())).unwrap();
    assert!(
        lines.iter().any(|l| l.contains("speech: reusing")),
        "{lines:?}"
    );
    std::fs::remove_dir_all(&d).ok();
}
