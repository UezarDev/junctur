use super::*;
use crate::cuts::timeline::EDL_BIAS;
use crate::cuts::Fps;
use junctur_core::edit::Range;

fn prop(id: u64, value: Value) -> Event {
    Event::Property { id, value }
}

fn source(file: &str) -> Load {
    Load {
        entry: 1,
        file: file.into(),
        form: Form::Source,
    }
}

/// Kept 1-2 s and 5-7 s of 10 s.
fn timeline() -> Arc<Timeline> {
    let keep = [
        Range {
            start: 1.0,
            end: 2.0,
        },
        Range {
            start: 5.0,
            end: 7.0,
        },
    ];
    Arc::new(Timeline::new(&keep, 10.0, Fps { num: 30, den: 1 }))
}

#[test]
fn folds_properties() {
    let mut s = PlayerState::default();
    s.start(source("C:/a.mp4"));
    assert_eq!(s.apply(&prop(DURATION, Value::Double(12.5))), Change::Other);
    assert_eq!(s.apply(&prop(PAUSE, Value::Flag(true))), Change::Other);
    assert_eq!(
        s.apply(&prop(TIME_POS, Value::Double(3.0))),
        Change::Position
    );
    let tracks = r#"[{"id":1,"type":"audio"},{"id":2,"type":"audio","selected":true}]"#;
    s.apply(&prop(TRACK_LIST, Value::Str(tracks.into())));
    s.apply(&prop(HWDEC, Value::Str("nvdec".into())));
    assert_eq!(s.hwdec.as_deref(), Some("nvdec"));
    assert_eq!(s.aspect, None, "no picture yet");
    s.apply(&prop(VIDEO_W, Value::Double(1920.0)));
    assert_eq!(s.aspect, None, "half a size is none");
    s.apply(&prop(VIDEO_H, Value::Double(1080.0)));
    assert!((s.aspect.unwrap() - 16.0 / 9.0).abs() < 1e-9);
    assert_eq!((s.width, s.height), (Some(1920), Some(1080)));
    // A phone recording stored landscape and rotated: mpv reads it as above with `rotate` 90
    // or 270 (as the bundled libmpv does), and it is portrait.
    s.apply(&prop(ROTATE, Value::Double(270.0)));
    assert_eq!((s.width, s.height), (Some(1080), Some(1920)));
    assert!(s.aspect.unwrap() < 1.0);
    s.apply(&prop(VIDEO_W, Value::None));
    assert_eq!(s.aspect, None, "the video went away");
    assert_eq!(s.path.as_deref(), Some("C:/a.mp4"));
    assert_eq!(s.duration, Some(12.5));
    assert!(s.paused);
    assert_eq!(s.position, Some(3.0));
    assert_eq!(s.audio_tracks.len(), 2);
    assert!(s.audio_tracks[1].selected);
    // No edit list: no output clock.
    assert_eq!((s.output, s.cuts, s.in_cut), (None, false, false));
}

#[test]
fn closing_leaves_nothing_playing() {
    let mut s = PlayerState::default();
    s.start(source("C:/a.mp4"));
    s.apply(&prop(TIME_POS, Value::Double(3.0)));
    s.apply(&prop(DURATION, Value::Double(12.5)));
    s.apply(&prop(VIDEO_W, Value::Double(1920.0)));
    s.apply(&prop(VIDEO_H, Value::Double(1080.0)));
    s.apply(&prop(HWDEC, Value::Str("nvdec".into())));
    s.close();
    assert_eq!(s.path, None, "the start window shows");
    assert_eq!((s.position, s.duration, s.aspect), (None, None, None));
    assert_eq!(s.hwdec.as_deref(), Some("nvdec"));
    // mpv's own events after `stop` keep it empty.
    s.apply(&prop(TIME_POS, Value::None));
    assert_eq!((s.path, s.position), (None, None));
}

#[test]
fn unavailable_properties_clear() {
    // mpv reports a property with no value (MPV_FORMAT_NONE) when no file is loaded.
    let mut s = PlayerState::default();
    s.start(source("x"));
    s.apply(&prop(TIME_POS, Value::Double(1.0)));
    s.apply(&prop(DURATION, Value::Double(2.0)));
    s.apply(&prop(TIME_POS, Value::None));
    s.apply(&prop(DURATION, Value::None));
    assert_eq!((s.position, s.duration), (None, None));
    assert_eq!(s.apply(&prop(99, Value::None)), Change::None);
}

#[test]
fn errors_last_until_a_file_loads() {
    let mut s = PlayerState::default();
    s.start(source("bad.mp4"));
    // A replaced load's failure is not this one's.
    s.apply(&Event::EndFile {
        entry: 0,
        reason: EndReason::Error,
        error: Some("old".into()),
    });
    assert_eq!(s.error, None);
    s.apply(&Event::EndFile {
        entry: 1,
        reason: EndReason::Error,
        error: Some("unrecognized file format".into()),
    });
    assert_eq!(s.error.as_deref(), Some("unrecognized file format"));
    assert_eq!(s.path, None, "nothing is playing");
    // Replacing a file ends the old one with Stop: not an error, and it keeps the message.
    assert_eq!(
        s.apply(&Event::EndFile {
            entry: 1,
            reason: EndReason::Stop,
            error: None
        }),
        Change::None
    );
    assert!(s.error.is_some());
    s.apply(&Event::FileLoaded);
    assert_eq!(s.error, None);
}

#[test]
fn whole_file_maps_to_output_time() {
    let mut s = PlayerState::default();
    s.start(source("a.mp4"));
    s.set_reference(Some(timeline()));
    s.apply(&prop(TIME_POS, Value::Double(5.5)));
    assert_eq!(s.position, Some(5.5));
    assert!((s.output.unwrap() - 1.5).abs() < 1e-3);
    assert_eq!((s.frame, s.output_frame), (Some(165), Some(45)));
    assert!(!s.in_cut && !s.cuts);
    s.apply(&prop(TIME_POS, Value::Double(3.0)));
    assert!(s.in_cut);
    assert_eq!(s.output_frame, Some(30), "a cut shows the next kept frame");
    assert!((s.output_duration.unwrap() - 3.0).abs() < 1e-9);
    // The edit list's length, not mpv's, so both forms show the same total.
    s.apply(&prop(DURATION, Value::Double(10.02)));
    assert_eq!(s.duration, Some(10.0));
}

#[test]
fn a_requested_position_stands_in_until_mpv_reaches_it() {
    let mut s = PlayerState::default();
    s.start(source("a.mp4"));
    s.apply(&prop(TIME_POS, Value::Double(1.0)));
    assert_eq!(s.resume_from(), Some(1.0));
    s.expect(40.0);
    // mpv still reports where it was before the seek.
    s.apply(&prop(TIME_POS, Value::Double(1.03)));
    assert_eq!(s.resume_from(), Some(40.0));
    s.apply(&prop(TIME_POS, Value::Double(40.0)));
    s.apply(&prop(TIME_POS, Value::Double(40.5)));
    assert_eq!(s.resume_from(), Some(40.5));
    // A fresh load has no position yet.
    s.expect(0.0);
    s.start(source("a.mp4"));
    assert_eq!((s.position, s.resume_from()), (None, Some(0.0)));
}

#[test]
fn kept_ranges_map_to_source_time() {
    let mut s = PlayerState::default();
    s.start(Load {
        entry: 1,
        file: "a.mp4".into(),
        form: Form::Cuts(timeline()),
    });
    // In the EDL mpv counts output time: frame 165 (5.5 s of source) shows at 1.5 s + the bias.
    s.apply(&prop(TIME_POS, Value::Double(1.5 + EDL_BIAS)));
    s.apply(&prop(DURATION, Value::Double(3.0)));
    assert!(s.cuts);
    assert_eq!(s.path.as_deref(), Some("a.mp4"));
    assert_eq!(s.output, Some(1.5 + EDL_BIAS));
    assert!((s.position.unwrap() - 5.5).abs() < 1e-3);
    assert_eq!((s.frame, s.output_frame), (Some(165), Some(45)));
    assert!((s.duration.unwrap() - 10.0).abs() < 1e-9, "source length");
    // The next load starts clean, in its own form.
    s.start(source("a.mp4"));
    assert_eq!((s.position, s.cuts), (None, false));
}

#[test]
fn observed_ids_are_distinct() {
    let ids: std::collections::HashSet<u64> = OBSERVED.iter().map(|o| o.0).collect();
    assert_eq!(ids.len(), OBSERVED.len());
}
