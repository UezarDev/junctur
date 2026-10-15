use super::*;

#[test]
fn levels_map_decibels_onto_a_byte() {
    assert_eq!(level(0.0), 0);
    assert_eq!(level(-0.5), 0, "a peak is a magnitude");
    assert_eq!(level(1.0), 255);
    assert_eq!(level(2.0), 255);
    assert_eq!(level(0.001), 0, "-60 dBFS is the floor");
    assert_eq!(level(0.0001), 0);
    // -30 dBFS, half way.
    assert!((level(10f32.powf(-1.5)) as i32 - 128).abs() <= 1);
    assert_eq!(level(f32::NAN), 0);
}

fn two_tracks() -> Waves {
    // Track 0 rises 0..99, track 1 is 7 throughout; 100 hops = 2 s.
    let mut w = Waves::new(2);
    for h in 0..100u8 {
        w.data.extend([h, 7]);
    }
    w
}

#[test]
fn columns_take_each_tracks_loudest_hop() {
    let w = two_tracks();
    assert_eq!(w.hops(), 100);
    assert!((w.seconds() - 2.0).abs() < 1e-9);
    let c = w.columns(0.0, 2.0, 4);
    assert_eq!(c, vec![24, 49, 74, 99, 7, 7, 7, 7]);
    // Past what was read: zeros, and nothing before 0.
    let c = w.columns(1.5, 3.5, 4);
    assert_eq!(&c[..4], &[99, 0, 0, 0]);
    let c = w.columns(-1.0, 1.0, 2);
    assert_eq!(&c[..2], &[0, 49]);
}

#[test]
fn columns_narrower_than_a_hop_repeat_it() {
    let w = two_tracks();
    // 0.1 s over 10 columns: 5 hops, two columns each.
    let c = w.columns(0.0, 0.1, 10);
    assert_eq!(&c[..10], &[0, 0, 1, 1, 2, 2, 3, 3, 4, 4]);
}

#[test]
fn columns_of_nothing_are_empty_or_zero() {
    assert!(Waves::new(2).columns(0.0, 1.0, 0).is_empty());
    assert_eq!(Waves::new(2).columns(0.0, 1.0, 3), vec![0; 6]);
    assert_eq!(two_tracks().columns(1.0, 1.0, 2), vec![0; 4]);
    assert_eq!(Waves::default().hops(), 0);
}

/// `columns` without the pyramid: every hop of every column read.
fn scanned(w: &Waves, from: f64, to: f64, columns: usize) -> Vec<u8> {
    let flat = Waves {
        tracks: w.tracks,
        data: w.data.clone(),
        levels: Vec::new(),
    };
    flat.columns(from, to, columns)
}

#[test]
fn the_pyramid_gives_the_columns_every_hop_gives() {
    // Peaks no pattern lines up with, appended in uneven reads as the decoder hands them over.
    let mut x: u32 = 12345;
    let mut next = || {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (x >> 16) as u8
    };
    let mut w = Waves::new(3);
    let mut hops = 0;
    for read in [1usize, 7, 8, 9, 100, 250, 1000, 3, 4000] {
        let more: Vec<u8> = (0..read * 3).map(|_| next()).collect();
        w.extend(&more);
        hops += read;
        assert_eq!(w.hops(), hops);
        let secs = w.seconds();
        for (from, to, columns) in [
            (0.0, secs, 1),
            (0.0, secs, 7),
            (0.0, secs, 640),
            (0.013, secs * 0.77, 333),
            (secs * 0.31, secs * 0.32, 50),
            (-3.0, secs + 5.0, 999),
            (0.0, 0.1, 10),
        ] {
            assert_eq!(
                w.columns(from, to, columns),
                scanned(&w, from, to, columns),
                "{hops} hops, {from}..{to} over {columns}"
            );
        }
    }
    assert!(w.levels.len() >= 6, "{} levels", w.levels.len());
    // Loaded from the folder, the pyramid is built whole, and the file holds the bytes alone.
    let dir = std::env::temp_dir().join(format!("junctur-waves-pyr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    save(&dir, &w).unwrap();
    assert_eq!(
        std::fs::metadata(dir.join(FILE)).unwrap().len() as usize,
        16 + w.data.len()
    );
    let back = load(&dir, 3).unwrap();
    assert_eq!(back, w);
    assert_eq!(back.levels, w.levels);
    assert_eq!(
        back.columns(0.0, w.seconds(), 800),
        scanned(&w, 0.0, w.seconds(), 800)
    );
    // Loaded, then read on: the pyramid goes on from the loaded one as from one appended to.
    let mut back = back;
    for read in [5usize, 3000, 11] {
        let more: Vec<u8> = (0..read * 3).map(|_| next()).collect();
        back.extend(&more);
        w.extend(&more);
        assert_eq!(back.levels, w.levels);
        let secs = back.seconds();
        for (from, to, columns) in [(0.0, secs, 640), (secs * 0.4, secs, 77)] {
            assert_eq!(
                back.columns(from, to, columns),
                scanned(&back, from, to, columns)
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn saved_peaks_load_only_for_the_same_track_count() {
    let dir = std::env::temp_dir().join(format!("junctur-waves-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(load(&dir, 2).is_none());
    let w = two_tracks();
    save(&dir, &w).unwrap();
    assert_eq!(load(&dir, 2), Some(w));
    assert!(load(&dir, 3).is_none());
    std::fs::write(dir.join(FILE), b"JWAVES1\n\x02\0\0\0\x32\0\0\0\x01").unwrap();
    assert!(load(&dir, 2).is_none(), "half a hop");
    std::fs::write(dir.join(FILE), b"junk").unwrap();
    assert!(load(&dir, 2).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn one_decode_reads_every_tracks_peaks() {
    let dir = std::env::temp_dir().join(format!("junctur-waves-read-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("two.mka");
    // A full-scale tone on track 1 for 3 s, a -20 dB one on track 2 that ends after 1 s (a
    // shorter track: it must not cut track 1's peaks short).
    let ok = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=3"])
        .args(["-f", "lavfi", "-i", "sine=f=300:d=1"])
        .args(["-filter_complex", "[0:a]volume=8[a];[1:a]volume=0.8[b]"])
        .args(["-map", "[a]", "-map", "[b]", "-c:a", "flac"])
        .arg(&f)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let mut w = Waves::new(2);
    let hops = read(&f, 2, Some(3.0), &AtomicBool::new(false), &mut |b| {
        w.data.extend_from_slice(b)
    })
    .unwrap()
    .unwrap();
    assert_eq!(hops, w.hops());
    assert_eq!(
        w.hops(),
        150,
        "3 s at 50 a second, past the short track's end"
    );
    let c = w.columns(0.0, 3.0, 3);
    // lavfi's sine is at 1/8 full scale; track 1 is turned up to full, track 2 to 1/10.
    assert!(c[0] > 245 && c[2] > 245, "{c:?}");
    assert!((c[3] as i32 - 170).abs() < 8, "-20 dB: {c:?}");
    // Silent after its first second, but for the resampler ringing out (about -52 dB).
    assert!(
        c[4] < 60 && c[5] == 0,
        "silence after its first second: {c:?}"
    );
    // One track alone is padded the same way.
    let mut one = 0;
    read(&f, 1, Some(3.0), &AtomicBool::new(false), &mut |b| {
        one += b.len()
    })
    .unwrap();
    assert_eq!(one, 150);
    let stopped = read(&f, 2, Some(3.0), &AtomicBool::new(true), &mut |_| {}).unwrap();
    assert!(stopped.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The levels with one reused peaks buffer are the ones a new buffer per hop gave, a short last
/// hop included.
#[test]
fn hop_levels_match_a_new_buffer_per_hop() {
    for tracks in [1, 3] {
        let buf: Vec<f32> = (0..PER_HOP * tracks * 7 + tracks * 5)
            .map(|i| {
                (((i * 7919) % 2001) as f32 / 1000.0 - 1.0) * if i % 3 == 0 { 0.01 } else { 1.0 }
            })
            .collect();
        let mut want = Vec::new();
        for hop in buf.chunks(PER_HOP * tracks) {
            let mut peaks = vec![0f32; tracks];
            for s in hop.chunks_exact(tracks) {
                for (p, x) in peaks.iter_mut().zip(s) {
                    *p = p.max(x.abs());
                }
            }
            want.extend(peaks.into_iter().map(level));
        }
        let (mut peaks, mut out) = (vec![0f32; tracks], vec![9u8; 3]);
        assert_eq!(hop_levels(&buf, tracks, &mut peaks, &mut out), 8);
        assert_eq!(out, want);
    }
}
