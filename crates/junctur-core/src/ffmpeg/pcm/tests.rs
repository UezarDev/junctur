use super::*;

/// Reference: each case's arguments written out on their own.
fn merge_args_before(tracks: &[usize], rate: u32, pad: Option<f64>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(len) = pad {
        let mut g = String::new();
        for (i, t) in tracks.iter().enumerate() {
            g += &format!(
                "[0:a:{t}]aformat=sample_fmts=flt:sample_rates={rate}:channel_layouts=mono,apad[a{i}];"
            );
        }
        if tracks.len() == 1 {
            g.pop();
            g = g.replace("[a0]", "[out]");
        } else {
            for i in 0..tracks.len() {
                g += &format!("[a{i}]");
            }
            g += &format!("amerge=inputs={}[out]", tracks.len());
        }
        args.extend(["-filter_complex".into(), g, "-map".into(), "[out]".into()]);
        args.extend(["-t".into(), format!("{len:.3}")]);
    } else if let [t] = tracks {
        args.extend(["-map".into(), format!("0:a:{t}"), "-ac".into(), "1".into()]);
        args.extend(["-ar".into(), rate.to_string()]);
    } else {
        let mut g = String::new();
        for (i, t) in tracks.iter().enumerate() {
            g += &format!(
                "[0:a:{t}]aformat=sample_fmts=flt:sample_rates={rate}:channel_layouts=mono[a{i}];"
            );
        }
        for i in 0..tracks.len() {
            g += &format!("[a{i}]");
        }
        g += &format!("amerge=inputs={}[out]", tracks.len());
        args.extend(["-filter_complex".into(), g, "-map".into(), "[out]".into()]);
    }
    args.extend(["-f".into(), "f32le".into(), "-".into()]);
    args
}

#[test]
fn merge_args_are_the_reference_ones() {
    for tracks in [&[0][..], &[2], &[0, 1], &[3, 0, 1]] {
        for pad in [None, Some(12.0), Some(3600.25)] {
            assert_eq!(
                merge_args(tracks, 16_000, pad),
                merge_args_before(tracks, 16_000, pad),
                "{tracks:?} {pad:?}"
            );
        }
    }
    assert_eq!(
        merge_args(&[1], 8000, Some(2.0)).join(" "),
        "-filter_complex [0:a:1]aformat=sample_fmts=flt:sample_rates=8000:channel_layouts=mono,apad[out] -map [out] -t 2.000 -f f32le -"
    );
}

#[test]
fn decode_failure_carries_the_ffmpeg_message() {
    let missing = std::env::temp_dir().join("junctur-no-such-file.mp4");
    let mut r = PcmReader::open(&missing, 0, 16_000, 1, Span::default()).unwrap();
    assert!(r.read(1024).unwrap().is_empty());
    let e = r.finish().unwrap_err().to_string();
    assert!(e.contains("junctur-no-such-file"), "{e}");
}

#[test]
fn pcm_round_trips_through_ffmpeg() {
    let dir = std::env::temp_dir().join(format!("junctur-pcm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("ramp.wav");
    let ramp: Vec<f32> = (0..32_000).map(|i| (i as f32 / 32_000.0) - 0.5).collect();
    let mut w =
        PcmWriter::create_with(&wav, 16_000, 1, &["-c:a".into(), "pcm_f32le".into()]).unwrap();
    w.write(&ramp).unwrap();
    w.finish().unwrap();
    let mut r = PcmReader::open(&wav, 0, 16_000, 1, Span::default()).unwrap();
    let mut buf = vec![9.0; 3];
    r.read_into(&mut buf, 20_000).unwrap();
    assert_eq!(buf, ramp[..20_000]);
    r.read_into(&mut buf, 20_000).unwrap();
    assert_eq!(buf, ramp[20_000..]);
    r.read_into(&mut buf, 20_000).unwrap();
    assert!(buf.is_empty());
    r.finish().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn one_ffmpeg_decodes_every_track_as_separate_ones_did() {
    let dir = std::env::temp_dir().join(format!("junctur-windows-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("three.mka");
    // Stereo with different channels, mono, and stereo at another rate.
    let ok = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=300:d=4"])
        .args(["-f", "lavfi", "-i", "sine=f=500:d=4", "-f", "lavfi", "-i"])
        .args(["sine=f=700:d=3.5:sample_rate=44100", "-filter_complex"])
        .args(["[0:a][1:a]join=inputs=2:channel_layout=stereo[s];[2:a]asplit=2[m][x];[x]pan=stereo|c0=c0|c1=0.3*c0[t]"])
        .args(["-map", "[s]", "-map", "[m]", "-map", "[t]", "-c:a", "flac"])
        .arg(&f)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let together = decode_windows(&f, &[0, 1, 2], 1.0, 3.0).unwrap();
    let alone: Vec<Vec<f32>> = (0..3)
        .map(|i| decode_window(&f, i, 1.0, 3.0).unwrap())
        .collect();
    // Every track as long as the shortest (the 3.5 s one), as `window::align` cut them.
    let shortest = alone.iter().map(Vec::len).min().unwrap();
    for (i, got) in together.iter().enumerate() {
        assert_eq!(got.len(), shortest, "track {i}");
        // The same samples, but for the resampler's first millisecond, which settles from a
        // different start (17 samples, at most 0.0024 apart).
        let (head, rest) = got.split_at(32);
        assert_eq!(rest, &alone[i][32..shortest], "track {i}");
        assert!(head
            .iter()
            .zip(&alone[i])
            .all(|(a, b)| (a - b).abs() < 0.01));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
