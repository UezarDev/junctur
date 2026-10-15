use super::*;

const FPS: Fps = Fps { num: 30, den: 1 };

fn r(start: f64, end: f64) -> Range {
    Range { start, end }
}

/// Kept 1-2 s and 5-7 s of a 10 s source: frames [30, 60) and [150, 210).
fn sample() -> Timeline {
    Timeline::new(&[r(1.0, 2.0), r(5.0, 7.0)], 10.0, FPS)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn frames_match_frame_ranges() {
    let t = sample();
    assert_eq!(t.ranges, vec![(30, 60), (150, 210)]);
    assert_eq!((t.total, t.output_frames), (300, 90));
    assert!(close(t.output_duration(), 3.0));
    assert!(close(t.source_duration(), 10.0));
    assert_eq!(t.boundaries().len(), 1);
    // Same rounding as the render: a range off the frame grid rounds outward.
    let t = Timeline::new(&[r(1.01, 1.99)], 10.0, FPS);
    assert_eq!(t.ranges, vec![(30, 60)]);
}

#[test]
fn source_to_output() {
    let t = sample();
    let at = |s: f64| t.to_output(s);
    assert!(close(at(1.5).0, 0.5) && !at(1.5).1);
    assert!(close(at(5.0).0, 1.0) && !at(5.0).1);
    assert!(close(at(6.5).0, 2.5));
    // Before the first range, between ranges, and after the last.
    assert!(close(at(0.2).0, 0.0) && at(0.2).1);
    assert!(close(at(3.0).0, 1.0) && at(3.0).1);
    assert!(at(2.0).1, "a range's end is cut");
    assert!(close(at(9.0).0, 3.0) && at(9.0).1);
    assert_eq!(t.output_frame(5.0), 30);
    assert_eq!(t.output_frame(3.0), 30, "a cut shows the next kept frame");
    assert_eq!(t.output_frame(9.0), 90);
}

#[test]
fn output_to_source_stays_on_kept_frames() {
    let t = sample();
    assert!(close(t.to_source(0.5), 1.5));
    assert!(close(t.to_source(1.0), 5.0));
    assert!(close(t.to_source(2.9), 6.9));
    assert_eq!(
        FPS.frame_at(t.to_source(99.0)),
        209,
        "past the end: the last kept frame"
    );
    // In mpv's timeline a segment starts a bias before its first frame: until that frame
    // shows, the previous segment's last frame is on screen, then the new first frame.
    assert_eq!(FPS.frame_at(t.to_source(1.0 - EDL_BIAS / 2.0)), 59);
    assert_eq!(FPS.frame_at(t.to_source(1.0 + EDL_BIAS / 2.0)), 150);
    for f in (30..60).chain(150..210) {
        let (o, cut) = t.to_output(FPS.seconds(f));
        assert!(!cut);
        assert_eq!(FPS.frame_at(t.to_source(o)), f);
    }
}

#[test]
fn seeks_land_on_kept_material() {
    let t = sample();
    assert_eq!(t.snap(1.5), Some(1.5));
    assert_eq!(t.snap(3.3), Some(5.0));
    assert_eq!(t.snap(7.0), None);
    let (o, s) = t.seek_target(3.3);
    assert_eq!(s, 5.0);
    assert_eq!(FPS.frame_at(t.to_source(o)), 150);
    // Past the last range: its last frame.
    let (o, s) = t.seek_target(9.0);
    assert_eq!(FPS.frame_at(s), 209);
    assert_eq!(FPS.frame_at(t.to_source(o)), 209);
}

#[test]
fn empty_keeps_nothing() {
    let t = Timeline::new(&[], 10.0, FPS);
    assert!(t.is_empty());
    assert_eq!(t.to_output(4.0), (0.0, true));
    assert_eq!(t.to_source(1.0), 0.0);
    assert_eq!(t.snap(1.0), None);
    assert!(t.boundaries().is_empty());
}

#[test]
fn ntsc_rate_frames() {
    let fps = Fps {
        num: 30000,
        den: 1001,
    };
    let t = Timeline::new(&[r(1.0, 2.0), r(2.01, 3.0)], 10.0, fps);
    // As frame_ranges' own test: the two touch after rounding and merge.
    assert_eq!(t.ranges, vec![(29, 90)]);
    assert_eq!(fps.frame_at(fps.seconds(1234)), 1234);
}

/// mpv shows frame `a + k` at the sum of the rounded lengths before it plus `k/fps + β`, β from
/// 0 to the bias: every range's first and last frame must map back to the same frame.
#[test]
fn agrees_with_mpv_timeline_over_ten_thousand_ranges() {
    let fps = Fps {
        num: 30000,
        den: 1001,
    };
    let keep: Vec<Range> = (0..10_000)
        .map(|i| {
            let a = fps.seconds(i * 25);
            r(a, a + fps.seconds(10))
        })
        .collect();
    let t = Timeline::new(&keep, fps.seconds(25 * 10_000 + 10), fps);
    assert_eq!(t.ranges.len(), 10_000);
    let mut mpv_start = 0.0;
    for (i, (&(a, b), &(_, len))) in t.ranges.iter().zip(t.segments()).enumerate() {
        for f in [a, b - 1] {
            for beta in [0.0, EDL_BIAS / 2.0, EDL_BIAS] {
                let shown = mpv_start + fps.seconds(f - a) + beta;
                assert_eq!(fps.frame_at(t.to_source(shown)), f, "range {i}");
                assert_eq!(t.output_frame(t.to_source(shown)), t.out_before[i] + f - a);
            }
        }
        // A seek to the range's start targets no later than its first frame shows.
        let (o, _) = t.seek_target(fps.seconds(a));
        assert!(o <= mpv_start + 1e-9, "range {i}");
        mpv_start += len;
    }
    assert_eq!(t.output_duration(), mpv_start);
}
