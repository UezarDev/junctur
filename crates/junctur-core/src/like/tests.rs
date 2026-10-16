use super::*;
use crate::ffmpeg::ANALYSIS_RATE;

const SR: f64 = ANALYSIS_RATE as f64;

/// `secs` of quiet noise, the same every run.
fn noise(secs: f64) -> Vec<f32> {
    let mut x: u32 = 12345;
    (0..(secs * SR) as usize)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.04
        })
        .collect()
}

/// Adds a 0.3 s sweep from `f0` to `f1` Hz at `at` seconds, `gain` loud, under a Hann envelope.
fn sweep(x: &mut [f32], at: f64, f0: f64, f1: f64, gain: f32) {
    let n = (0.3 * SR) as usize;
    let first = (at * SR) as usize;
    let mut phase = 0.0f64;
    for i in 0..n {
        let t = i as f64 / n as f64;
        phase += std::f64::consts::TAU * (f0 + (f1 - f0) * t) / SR;
        let env = 0.5 - 0.5 * (std::f64::consts::TAU * t).cos();
        if let Some(v) = x.get_mut(first + i) {
            *v += gain * (env * phase.sin()) as f32;
        }
    }
}

const CHIRPS: [f64; 4] = [2.0, 7.0, 12.3, 18.0];
const LOOKALIKE: f64 = 15.0;
/// The lookalike's sweep.
const LOOK: (f64, f64) = (1150.0, 3300.0);

/// 22 s of noise with an up-sweep at [`CHIRPS`] (the third one at a quarter of the loudness) and a
/// down-sweep over the same band at [`LOOKALIKE`].
fn track() -> Spectrogram {
    let mut x = noise(22.0);
    for (i, &at) in CHIRPS.iter().enumerate() {
        sweep(&mut x, at, 1000.0, 3000.0, if i == 2 { 0.08 } else { 0.3 });
    }
    sweep(&mut x, LOOKALIKE, LOOK.0, LOOK.1, 0.3);
    Spectrogram::of_samples(&x)
}

fn near(ms: &[Match], at: f64) -> Option<&Match> {
    ms.iter().find(|m| (m.start - at).abs() < 0.12)
}

#[test]
fn the_spectrogram_is_a_frame_every_10_ms_and_reads_back_from_its_file() {
    let s = Spectrogram::of_samples(&noise(2.0));
    assert_eq!(s.frames(), 1 + (32_000 - 400) / 160);
    // Streamed in pieces of any size, the same frames.
    let x = noise(2.0);
    let mut m = mel::Mel::new();
    for piece in x.chunks(777) {
        m.push(piece);
    }
    assert_eq!(m.finish(), s);
    let dir = std::env::temp_dir().join(format!("junctur-like-mel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    mel::save(&dir, 1, &s).unwrap();
    assert_eq!(mel::load(&dir, 1), Some(s));
    assert_eq!(mel::load(&dir, 0), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_chirp_is_found_in_noise_whatever_its_loudness() {
    let spec = track();
    let ex = [Example::span(CHIRPS[0], CHIRPS[0] + 0.3, true)];
    let found = find(&spec, &ex, None).unwrap();
    for at in CHIRPS {
        let m = near(&found, at).unwrap_or_else(|| panic!("no match at {at}: {found:?}"));
        assert!(m.score > 0.65, "{at}: {}", m.score);
        assert!(
            (m.point - m.start).abs() < 1e-9,
            "a dragged example's point is its start"
        );
    }
    // Noise alone never scores like the sound.
    let best_other = found
        .iter()
        .filter(|m| {
            CHIRPS
                .iter()
                .chain([&LOOKALIKE])
                .all(|&a| (m.start - a).abs() > 0.4)
        })
        .map(|m| m.score)
        .fold(0.0, f64::max);
    assert!(best_other < 0.5, "{best_other}");
}

#[test]
fn a_not_this_lowers_a_lookalike_and_leaves_the_real_ones() {
    let spec = track();
    let pos = Example::span(CHIRPS[0], CHIRPS[0] + 0.3, true);
    let score_at = |ms: &[Match], at| near(ms, at).map_or(0.0, |m| m.score);
    let before = find(&spec, &[pos], None).unwrap();
    // Its ✗: the match's own window, as the page sends it.
    let m = near(&before, LOOKALIKE).unwrap();
    assert!(
        m.score > THRESHOLD,
        "a lookalike worth rejecting: {}",
        m.score
    );
    let neg = Example::span(m.start, m.end, false);
    let after = find(&spec, &[pos, neg], None).unwrap();
    let (was, now) = (score_at(&before, LOOKALIKE), score_at(&after, LOOKALIKE));
    assert!(now < was - 0.3, "{was} -> {now}");
    assert!(now < THRESHOLD, "{now}");
    for at in CHIRPS {
        assert_eq!(score_at(&after, at), score_at(&before, at), "{at}");
    }
}

#[test]
fn a_range_limits_the_search_and_a_click_points_at_the_click() {
    let spec = track();
    let ex = [Example::click(CHIRPS[1] + 0.15, true)];
    assert_eq!((ex[0].start, ex[0].end), (6.9, 7.4));
    let found = find(&spec, &ex, Some((10.0, 20.0))).unwrap();
    assert!(found.iter().all(|m| m.start >= 10.0 && m.end <= 20.0));
    let m = found
        .iter()
        .find(|m| (m.point - (CHIRPS[2] + 0.15)).abs() < 0.08);
    assert!(m.is_some(), "{found:?}");
    assert!(
        found
            .iter()
            .any(|m| (m.point - (CHIRPS[3] + 0.15)).abs() < 0.08),
        "{found:?}"
    );
    // A range shorter than the example finds nothing.
    assert!(find(&spec, &ex, Some((3.0, 3.2))).unwrap().is_empty());
}

#[test]
fn an_example_needs_sound_and_a_positive() {
    let spec = Spectrogram::of_samples(&vec![0.0; 32_000]);
    let e = find(&spec, &[Example::click(1.0, true)], None).unwrap_err();
    assert!(e.to_string().contains("no sound"), "{e}");
    let e = find(&track(), &[Example::click(1.0, false)], None).unwrap_err();
    assert!(e.to_string().contains("mark a moment"), "{e}");
    // A long drag is its first 3 s.
    let s = Example::span(10.0, 4.0, true);
    assert_eq!((s.start, s.end, s.point), (4.0, 7.0, 4.0));
}

#[test]
fn a_spread_is_a_sliding_max() {
    let x = [0.0, 3.0, 1.0, 0.0, 0.0, 2.0, 0.0];
    assert_eq!(
        search::spread_for_tests(&x, 1),
        vec![3.0, 3.0, 3.0, 1.0, 2.0, 2.0, 2.0]
    );
    assert_eq!(search::spread_for_tests(&x, 0), x.to_vec());
}

#[test]
fn peaks_keep_the_highest_apart_and_filter_like_a_higher_floor() {
    let score = [0.1, 0.5, 0.4, 0.9, 0.2, 0.7, 0.8, 0.1, 0.65, 0.1];
    assert_eq!(
        search::peaks(&score, 0.3, 3, 10),
        vec![(3, 0.9), (6, 0.8)],
        "1 and 8 are within 3 frames of higher ones"
    );
    assert_eq!(
        search::peaks(&score, 0.3, 2, 10),
        vec![(1, 0.5), (3, 0.9), (6, 0.8), (8, 0.65)]
    );
    let at = |f: f32| -> Vec<usize> {
        search::peaks(&score, f, 2, 10)
            .into_iter()
            .map(|p| p.0)
            .collect()
    };
    let filtered: Vec<usize> = search::peaks(&score, 0.3, 2, 10)
        .into_iter()
        .filter(|p| p.1 >= 0.7)
        .map(|p| p.0)
        .collect();
    assert_eq!(at(0.7), filtered);
    assert_eq!(search::peaks(&score, 0.3, 1, 1), vec![(3, 0.9)]);
}

fn like_with(points: &[(f64, f64)]) -> Like {
    Like {
        matches: points
            .iter()
            .map(|&(point, score)| Match {
                start: point,
                end: point + 0.5,
                point,
                score,
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn keep_around_keeps_a_quarter_second_round_each_shown_match_and_cuts_the_rest() {
    let mut l = like_with(&[(10.0, 0.9), (10.2, 0.8), (30.0, 0.7), (50.0, 0.4)]);
    l.range = Some((5.0, 60.0));
    let p = l.propose(Kind::KeepAround, 100.0).unwrap().clone();
    assert_eq!(p.matches, 3, "the 0.4 one is under the slider");
    assert_eq!(p.range, (5.0, 60.0));
    assert_eq!(p.spans, vec![(9.9, 10.35), (29.9, 30.15)]);
    assert_eq!(
        p.edits(),
        vec![(5.0, 60.0, false), (9.9, 10.35, true), (29.9, 30.15, true)]
    );
    // A tick shows one under the slider.
    l.confirmed.push(50.0);
    assert_eq!(l.shown().len(), 4);
    l.proposal = None;
    let p = l.propose(Kind::CutEach, 100.0).unwrap();
    assert_eq!(p.range, (9.9, 50.15));
    assert!(p.edits().iter().all(|e| !e.2));
    assert!(like_with(&[]).propose(Kind::CutEach, 10.0).is_err());
}

#[test]
fn an_accepted_proposal_undoes_to_what_the_list_kept() {
    let mut l = like_with(&[(10.0, 0.9)]);
    l.range = Some((0.0, 20.0));
    l.propose(Kind::KeepAround, 20.0).unwrap();
    let now = [
        Range {
            start: 2.0,
            end: 4.0,
        },
        Range {
            start: 15.0,
            end: 25.0,
        },
    ];
    let edits = l.accept(&now).unwrap();
    assert_eq!(edits, vec![(0.0, 20.0, false), (9.9, 10.15, true)]);
    assert!(l.proposal.is_none());
    let id = l.applied[0].id;
    assert_eq!(
        l.undo(id),
        vec![(0.0, 20.0, false), (2.0, 4.0, true), (15.0, 20.0, true)]
    );
    assert!(l.applied.is_empty());
    assert!(l.undo(id).is_empty());
    assert!(l.accept(&now).is_none(), "nothing proposed");
}

#[test]
fn sent_matches_become_moments_and_can_be_taken_back() {
    let mut l = like_with(&[(40.0, 0.9), (100.0, 0.95)]);
    l.before = 2.0;
    l.after = 3.0;
    let mark = l.send(SendKind::Mark, 200.0).unwrap().id;
    l.send(SendKind::Clip, 200.0).unwrap();
    let ms = crate::moments::moments(l.markers(), 200.0);
    let spans: Vec<(f64, f64)> = ms.iter().map(|m| (m.start, m.end)).collect();
    assert_eq!(
        spans,
        vec![(10.0, 40.0), (38.0, 43.0), (70.0, 100.0), (98.0, 103.0)]
    );
    assert!(ms
        .iter()
        .all(|m| m.source == Source::Like && m.label == "Like this"));
    assert!(l.take_back(mark));
    assert!(!l.take_back(mark));
    assert_eq!(crate::moments::moments(l.markers(), 200.0).len(), 2);
}

#[test]
fn a_new_search_drops_ticks_it_no_longer_finds_and_the_proposal() {
    let mut l = like_with(&[(10.0, 0.9), (20.0, 0.9)]);
    l.confirmed = vec![10.0, 20.0];
    l.propose(Kind::CutEach, 30.0).unwrap();
    let searched = Searched {
        track: 0,
        range: None,
        examples: 1,
        seconds: 0.1,
        built: false,
        capped: false,
    };
    l.found(like_with(&[(20.0, 0.7)]).matches, searched);
    assert_eq!(l.confirmed, vec![20.0]);
    assert!(l.proposal.is_none());
    // The file reads back as written, and an old one without fields reads with the defaults.
    let back: Like = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    assert_eq!(back, l);
    let old: Like = serde_json::from_str("{\"examples\": []}").unwrap();
    assert_eq!(
        (old.threshold, old.before, old.after),
        (THRESHOLD, BEFORE_S, AFTER_S)
    );
}

/// The scores do not depend on how the frames were shared out: across a chunk's edge they are what
/// one patch compared window by window gives.
#[test]
fn scores_agree_across_chunks() {
    let frames = 40_000;
    let mut x: u32 = 99;
    let mut bytes: Vec<u8> = (0..frames * mel::BANDS)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            100 + (x >> 27) as u8
        })
        .collect();
    // The same rising pattern at a chunk's edge and far from one.
    for at in [16_380, 30_000] {
        for f in 0..20 {
            for b in 0..mel::BANDS {
                bytes[(at + f) * mel::BANDS + b] = (60 + 4 * f + (b % 7) * 3) as u8;
            }
        }
    }
    let spec = Spectrogram::from_bytes(bytes);
    let p = search::Patch::of(&spec, 30_000, 20).unwrap();
    let neg = search::Patch::of(&spec, 1_000, 20).unwrap();
    let s = search::scores(&spec, 0, frames, std::slice::from_ref(&p), &[]);
    assert!(s.score[16_380] > 0.99 && s.score[30_000] > 0.99);
    for t in [16_370, 16_383, 16_384, 16_390, 39_980] {
        let one = search::scores(&spec, t, t + 20, std::slice::from_ref(&p), &[]);
        assert!((one.score[0] - s.score[t]).abs() < 1e-4, "{t}");
    }
    assert!(
        s.score[39_981] == -1.0,
        "a window past the end matches nothing"
    );
    // Two negatives of different lengths: each spreads over its own.
    let short = search::Patch::of(&spec, 2_000, 4).unwrap();
    let both = search::scores(&spec, 0, frames, std::slice::from_ref(&p), &[neg, short]);
    assert_eq!(both.score.len(), frames);
    assert!(
        both.score[30_000] > 0.99,
        "the example is not like either negative"
    );
}
