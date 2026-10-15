use super::*;

#[test]
fn sample_ranges_follow_the_frames() {
    // 30 fps at 48 kHz: 1600 samples a frame.
    assert_eq!(
        sample_ranges(&[(3, 33)], 48_000, 30, 1),
        vec![(4800, 52_800)]
    );
    // 29.97 fps at 44.1 kHz: frame 1 is 1471.47 samples -> 1471.
    assert_eq!(
        sample_ranges(&[(1, 2)], 44_100, 30_000, 1001),
        vec![(1471, 2943)]
    );
}

#[test]
fn output_guard() {
    let dir = std::env::temp_dir().join(format!("junctur-guard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.mp4");
    std::fs::write(&src, b"x").unwrap();
    assert!(check_output(&src, &src, true).is_err(), "never the source");
    assert!(check_output(&src, &dir.join("./in.mp4"), true).is_err());
    let link = dir.join("link.mp4");
    std::fs::hard_link(&src, &link).unwrap();
    assert!(
        check_output(&src, &link, true).is_err(),
        "a hard link is the source"
    );
    assert!(check_output(&src, &src.with_file_name("out.mp4"), false).is_ok());
    std::fs::write(dir.join("out.mp4"), b"y").unwrap();
    assert!(check_output(&src, &dir.join("out.mp4"), false).is_err());
    assert!(check_output(&src, &dir.join("out.mp4"), true).is_ok());
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        scratch_dir(Path::new("x/out.mp4")),
        PathBuf::from("x/out.mp4.junctur-parts")
    );
}

#[test]
fn a_finished_render_that_cannot_replace_the_output_is_kept() {
    let dir = std::env::temp_dir().join(format!("junctur-install-{}", std::process::id()));
    let work = dir.join("out.mp4.junctur-parts");
    std::fs::create_dir_all(&work).unwrap();
    let partial = work.join("out.mp4");
    // A folder where the output goes: no rename replaces it, on any system.
    let out = dir.join("out.mp4");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(&partial, b"render").unwrap();
    let e = install(&partial, &out).unwrap_err().to_string();
    let beside = dir.join("out.junctur-new.mp4");
    assert!(e.contains("out.junctur-new.mp4"), "{e}");
    assert_eq!(std::fs::read(&beside).unwrap(), b"render");
    // The name beside is taken too: the render stays in the parts folder.
    std::fs::write(&partial, b"render 2").unwrap();
    let e = install(&partial, &out).unwrap_err().to_string();
    assert!(e.contains("junctur-parts"), "{e}");
    assert_eq!(std::fs::read(&partial).unwrap(), b"render 2");
    // The usual case replaces the output.
    std::fs::remove_dir_all(&out).unwrap();
    std::fs::write(&out, b"old").unwrap();
    install(&partial, &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"render 2");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Every frame of `file`'s video, decoded small (32x24 luma).
fn small_frames(file: &Path) -> Vec<Vec<u8>> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(file)
        .args([
            "-vf",
            "scale=32:24",
            "-pix_fmt",
            "gray",
            "-fps_mode",
            "passthrough",
        ])
        .args(["-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(out.status.success());
    out.stdout
        .chunks_exact(32 * 24)
        .map(<[u8]>::to_vec)
        .collect()
}

fn mse(a: &[u8], b: &[u8]) -> f64 {
    let d = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (x as f64 - y as f64).powi(2));
    d.sum::<f64>() / a.len() as f64
}

#[test]
#[ignore = "needs NVENC"]
fn a_batch_lands_on_the_frames_its_pieces_did() {
    let dir = std::env::temp_dir().join(format!("junctur-batch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // testsrc2 moves every frame, so a frame is told from its neighbours.
    let src = dir.join("in.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x240:r=30:d=10",
        ])
        .args([
            "-c:v", "libx264", "-g", "30", "-bf", "2", "-pix_fmt", "yuv420p",
        ])
        .arg(&src)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let info = source::inspect(&src).unwrap();
    let kf = &info.keyframes;
    let pieces = plan::pieces(&[(10, 50), (70, 130), (140, 200)], kf, plan::gop(kf));
    let batches = plan::batches(&pieces, 150, 900);
    let mut joined = 0;
    for b in batches.iter().filter(|b| b.len() > 1) {
        let batch = &pieces[b.clone()];
        let one = dir.join("batch.ts");
        ffmpeg::ffmpeg(&video::args(&src, batch, &info.video, true, &one)).unwrap();
        let mut apart = Vec::new();
        for p in batch {
            let each = dir.join("piece.ts");
            ffmpeg::ffmpeg(&video::args(&src, &[*p], &info.video, true, &each)).unwrap();
            apart.extend(small_frames(&each));
        }
        // Compared with the pieces encoded one by one, not the source: on these clips both
        // land one frame late, and batching must not change where.
        let together = small_frames(&one);
        assert_eq!(together.len(), apart.len());
        for (k, f) in together.iter().enumerate() {
            let here = mse(f, &apart[k]);
            for n in [k.wrapping_sub(1), k + 1] {
                if let Some(other) = apart.get(n) {
                    assert!(here < mse(f, other), "batch frame {k} is piece frame {n}");
                }
            }
        }
        joined += 1;
    }
    assert!(joined > 0, "no batch in {batches:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
