//! End-to-end track analysis on tiny clips generated with ffmpeg `lavfi` sources, one per layout
//! the real recordings showed. Clips and caches go to a per-run temp dir, never beside any input.

use std::path::{Path, PathBuf};
use std::process::Command;

use junctur_core::tracks::{Role, Sameness};
use junctur_core::{analyze, AnalyzeOptions};

fn workdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("junctur-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg(args: &[&str]) {
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg on PATH")
        .success();
    assert!(ok, "ffmpeg failed: {args:?}");
}

fn opts(dir: &Path) -> AnalyzeOptions {
    AnalyzeOptions {
        windows: 4,
        window_seconds: 3.0,
        cache_dir: Some(dir.join("cache")),
        use_cache: false,
    }
}

#[test]
fn four_identical_aac_tracks_collapse() {
    let d = workdir("four");
    let f = d.join("four.mp4");
    let f_s = f.to_str().unwrap();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:d=20",
        "-filter_complex",
        "[0]asplit=4[a][b][c][e]",
        "-map",
        "[a]",
        "-map",
        "[b]",
        "-map",
        "[c]",
        "-map",
        "[e]",
        "-c:a",
        "aac",
        "-ac",
        "2",
        f_s,
    ]);
    let r = analyze(&f, &opts(&d)).unwrap();
    assert_eq!(r.tracks.len(), 4);
    assert_eq!(r.windows.len(), 4);
    assert_eq!(r.logical.len(), 1);
    assert_eq!(r.logical[0].members, vec![0, 1, 2, 3]);
    assert_eq!(r.logical[0].sameness, Sameness::Exact);
    assert_eq!(r.logical[0].role, Role::Mix);
    std::fs::remove_dir_all(d).ok();
}

#[test]
// lavfi `sine` defaults to amplitude 1/8, so volume=0.005 lands near -75 dB, like the real faint track.
fn loud_faint_and_silent_tracks_are_told_apart() {
    let d = workdir("lfs");
    let f = d.join("lfs.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:d=20",
        "-f",
        "lavfi",
        "-i",
        "sine=f=700:d=20,volume=0.005",
        "-f",
        "lavfi",
        "-t",
        "20",
        "-i",
        "anullsrc=r=48000:cl=stereo",
        "-map",
        "0",
        "-map",
        "1",
        "-map",
        "2",
        "-c:a",
        "aac",
        f.to_str().unwrap(),
    ]);
    let r = analyze(&f, &opts(&d)).unwrap();
    let roles: Vec<Role> = r.logical.iter().map(|l| l.role).collect();
    assert_eq!(roles, vec![Role::Mix, Role::Unknown, Role::Silent]);
    std::fs::remove_dir_all(d).ok();
}

#[test]
fn a_track_that_sums_the_others_is_the_mix() {
    let d = workdir("sum");
    let f = d.join("sum.mkv");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=300:d=20,volume='if(lt(mod(t,2),1),1,0)':eval=frame",
        "-f",
        "lavfi",
        "-i",
        "sine=f=1700:d=20,volume=0.5",
        "-filter_complex",
        "[0]asplit[v1][v2];[1]asplit[g1][g2];[v2][g2]amix=inputs=2:normalize=0[m]",
        "-map",
        "[m]",
        "-map",
        "[v1]",
        "-map",
        "[g1]",
        "-c:a",
        "pcm_s16le",
        f.to_str().unwrap(),
    ]);
    let r = analyze(&f, &opts(&d)).unwrap();
    let roles: Vec<Role> = r.logical.iter().map(|l| l.role).collect();
    assert_eq!(roles, vec![Role::Mix, Role::Voice, Role::GamePc]);
    std::fs::remove_dir_all(d).ok();
}

#[test]
fn second_run_is_served_from_the_cache_dir() {
    let d = workdir("cache");
    let f = d.join("one.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:d=5",
        "-c:a",
        "aac",
        f.to_str().unwrap(),
    ]);
    let o = AnalyzeOptions {
        use_cache: true,
        ..opts(&d)
    };
    let first = analyze(&f, &o).unwrap();
    assert!(!first.from_cache);
    let second = analyze(&f, &o).unwrap();
    assert!(second.from_cache);
    assert_eq!(first.logical, second.logical);
    assert!(
        std::fs::read_dir(d.join("cache").join("tracks"))
            .unwrap()
            .count()
            == 1
    );
    std::fs::remove_dir_all(d).ok();
}

#[test]
fn missing_file_is_an_error_not_a_panic() {
    let d = workdir("missing");
    assert!(analyze(&d.join("nope.mp4"), &opts(&d)).is_err());
    std::fs::remove_dir_all(d).ok();
}
