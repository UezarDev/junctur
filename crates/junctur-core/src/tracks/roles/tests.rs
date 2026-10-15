use super::*;

fn act(mean_db: f64) -> Activity {
    Activity {
        mean_db,
        peak_db: mean_db + 15.0,
        near_silent_share: 0.2,
        silent: false,
    }
}

fn cx(faint: bool, substantial: usize) -> Context {
    Context {
        sum_of_others: None,
        faint,
        substantial,
    }
}

fn sp(share: f64, quiet_gaps: f64) -> Option<Speech> {
    Some(Speech { share, quiet_gaps })
}

#[test]
fn faint_track_is_named_by_speech() {
    assert_eq!(
        decide(&act(-70.0), sp(0.3, 0.9), &cx(true, 1)).0,
        Role::Voice
    );
    assert_eq!(
        decide(&act(-70.0), sp(0.02, 0.9), &cx(true, 1)).0,
        Role::GamePc
    );
    assert_eq!(decide(&act(-70.0), None, &cx(true, 1)).0, Role::Unknown);
}

#[test]
fn single_track_mix_versus_voice_only() {
    assert_eq!(
        decide(&act(-25.0), sp(0.4, 0.1), &cx(false, 1)).0,
        Role::Mix
    );
    assert_eq!(
        decide(&act(-25.0), sp(0.4, 0.9), &cx(false, 1)).0,
        Role::Voice
    );
    assert_eq!(
        decide(&act(-25.0), sp(0.0, 0.0), &cx(false, 1)).0,
        Role::GamePc
    );
}

#[test]
fn sum_and_silence_win_over_speech() {
    let mut c = cx(false, 3);
    c.sum_of_others = Some(1e-4);
    assert_eq!(decide(&act(-20.0), sp(0.0, 0.0), &c).0, Role::Mix);
    let mut s = act(-120.0);
    s.silent = true;
    assert_eq!(decide(&s, sp(0.5, 0.5), &cx(false, 1)).0, Role::Silent);
}

#[test]
fn reasons_read_as_one_line() {
    // Every branch: a lost line continuation would leave runs of spaces in these.
    let mut silent = act(-120.0);
    silent.silent = true;
    let mut sum = cx(false, 3);
    sum.sum_of_others = Some(1e-4);
    let cases = [
        decide(&silent, None, &cx(false, 1)),
        decide(&act(-20.0), sp(0.3, 0.5), &sum),
        decide(&act(-70.0), None, &cx(true, 1)),
        decide(&act(-70.0), sp(0.3, 0.9), &cx(true, 1)),
        decide(&act(-70.0), sp(0.02, 0.9), &cx(true, 1)),
        decide(&act(-25.0), None, &cx(false, 1)),
        decide(&act(-25.0), None, &cx(false, 2)),
        decide(&act(-25.0), sp(0.0, 0.0), &cx(false, 1)),
        decide(&act(-25.0), sp(0.4, 0.9), &cx(false, 2)),
        decide(&act(-25.0), sp(0.4, 0.1), &cx(false, 1)),
        decide(&act(-25.0), sp(0.4, 0.1), &cx(false, 2)),
    ];
    for (_, _, why) in cases {
        assert!(!why.contains("  "), "{why:?}");
    }
}

#[test]
fn measure_counts_quiet_gaps() {
    // 20 frames of speech probability then 20 of none; the gap samples are silent.
    let mut probs = vec![0.9f32; 20];
    probs.extend([0.0; 20]);
    let mut samples = vec![0.3f32; 20 * vad::FRAME];
    samples.extend(vec![0.0; 20 * vad::FRAME]);
    let s = measure(&probs, &samples);
    assert!((s.share - 0.5).abs() < 1e-9);
    assert_eq!(s.quiet_gaps, 1.0);
}
