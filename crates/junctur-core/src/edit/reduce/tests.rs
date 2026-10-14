use super::*;

fn r(start: f64, end: f64) -> Region {
    Region { start, end }
}

fn flat() -> Envelope {
    Envelope(vec![1.0; 10_000])
}

#[test]
fn no_speech_keeps_nothing() {
    assert!(keep_ranges(&[], &flat(), 100.0, &CutSettings::default()).is_empty());
}

#[test]
fn pads_merges_and_clamps() {
    let s = CutSettings::default();
    let k = keep_ranges(
        &[r(10.0, 12.0), r(0.1, 1.0), r(12.5, 13.0), r(30.0, 40.0)],
        &flat(),
        40.1,
        &s,
    );
    assert_eq!(k.len(), 3);
    assert_eq!(k[0].start, 0.0);
    assert!((k[1].start - 9.75).abs() < 1e-9 && (k[1].end - 13.35).abs() < 1e-9);
    assert!((k[2].end - 40.1).abs() < 1e-9);
}

#[test]
fn overlapping_tracks_union() {
    let k = keep_ranges(
        &[r(5.0, 8.0), r(6.0, 7.0), r(7.5, 9.0)],
        &flat(),
        100.0,
        &CutSettings::default(),
    );
    assert_eq!(k.len(), 1);
    assert!((k[0].end - 9.35).abs() < 1e-9);
}

#[test]
fn snaps_to_quiet_but_not_into_speech() {
    // Loud everywhere except a dip at 9.70 s (before the pad start of 9.75) and one at 10.05 s,
    // inside the speech: the start must take the first and ignore the second.
    let mut e = vec![1.0f32; 2000];
    e[970] = 0.0;
    e[1005] = 0.0;
    let k = keep_ranges(
        &[r(10.0, 11.0)],
        &Envelope(e),
        100.0,
        &CutSettings::default(),
    );
    assert!((k[0].start - 9.70).abs() < 1e-6, "{:?}", k[0]);
}

#[test]
fn hand_boundaries_pad_snap_and_stay_in_the_gap() {
    let s = CutSettings::default();
    // Flat: just the pad. After kept speech ending at 10.0, 0.35 s later; before speech at
    // 12.0, 0.25 s earlier.
    let snap = |env: &Envelope, gap, kept_before| snap_boundary(env, gap, kept_before, &s).unwrap();
    let b = snap(&flat(), (10.0, 12.0), true);
    assert!((b - 10.35).abs() < 1e-9, "{b}");
    let b = snap(&flat(), (10.0, 12.0), false);
    assert!((b - 11.75).abs() < 1e-9, "{b}");
    // A pause shorter than the pad: the far edge of the gap, never into the next word.
    let b = snap(&flat(), (10.0, 10.1), true);
    assert!((b - 10.1).abs() < 1e-9, "{b}");
    // A quiet step within `snap` wins, but not one past the next word's start.
    let mut e = vec![1.0f32; 2000];
    e[1040] = 0.0;
    e[1060] = 0.0;
    let b = snap(&Envelope(e.clone()), (10.0, 10.5), true);
    assert!((b - 10.40).abs() < 1e-6, "{b}");
    // Before speech at 10.45: the target is 10.20, and 10.40 is beyond `snap` of it.
    let b = snap(&Envelope(e), (10.0, 10.45), false);
    assert!((b - 10.20).abs() < 1e-6, "{b}");
    // Overlapping speech (two tracks): no pause to place it in.
    assert_eq!(snap_boundary(&flat(), (5.0, 4.0), true, &s), None);
}

#[test]
fn word_spans_never_clip_and_say_when_there_is_no_pause() {
    let s = CutSettings::default();
    let env = Envelope::default();
    // Cut a run 12.0-12.5 with speech ending at 11.0 before and starting at 14.0 after.
    let (a, b) = word_span(Some(11.0), (12.0, 12.5), Some(14.0), false, &env, &s, 60.0);
    assert!(
        (a - 11.35).abs() < 1e-9 && (b - 13.75).abs() < 1e-9,
        "{a} {b}"
    );
    // Keep it: pads around the run itself.
    let (a, b) = word_span(Some(11.0), (12.0, 12.5), Some(14.0), true, &env, &s, 60.0);
    assert!(
        (a - 11.75).abs() < 1e-9 && (b - 12.85).abs() < 1e-9,
        "{a} {b}"
    );
    // A word on another track runs 10-15, past the run: no pause either side, so the run's
    // own edges, not a point inside that word claimed as a clean pause.
    let (a, b) = word_span(Some(15.0), (12.0, 12.5), Some(12.2), false, &env, &s, 60.0);
    assert_eq!((a, b), (12.0, 12.5));
    // Ends of the transcript: neighbours both pads away, within the recording.
    let (a, b) = word_span(None, (0.2, 1.0), None, false, &env, &s, 1.1);
    assert!(
        (0.0..=0.2).contains(&a) && (1.0..=1.1).contains(&b),
        "{a} {b}"
    );
}
