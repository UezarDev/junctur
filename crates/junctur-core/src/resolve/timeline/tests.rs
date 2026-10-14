use super::*;

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| s.to_string()).collect()
}

#[test]
fn named_after_the_recording_as_the_fcpxml_names_it() {
    let edit = EditList {
        source: r"C:\Videos\2026-06-15 00-09-04.mp4".into(),
        duration: 1.0,
        settings: Default::default(),
        keep: Vec::new(),
        speakers: Default::default(),
        captions: None,
        fixes: Vec::new(),
        times: Vec::new(),
        windows: Vec::new(),
    };
    assert_eq!(name(&edit), "2026-06-15 00-09-04 (Junctur)");
}

#[test]
fn a_free_name_is_built_without_asking() {
    let d = decide(&names(&["Timeline 1", "rec"]), "rec (Junctur)", None).unwrap();
    assert_eq!(
        d,
        Decision::Build {
            name: "rec (Junctur)".into(),
            replace: false
        }
    );
    // An answer given for a name that is free by now changes nothing.
    let d = decide(&[], "rec (Junctur)", Some(Answer::Replace)).unwrap();
    assert!(matches!(d, Decision::Build { replace: false, .. }));
}

#[test]
fn a_taken_name_asks_until_answered() {
    let n = names(&["REC (Junctur)", "rec (Junctur) 2", "rec (Junctur) 4"]);
    assert_eq!(decide(&n, "rec (Junctur)", None).unwrap(), Decision::Ask);
    assert_eq!(
        decide(&n, "rec (Junctur)", Some(Answer::New)).unwrap(),
        Decision::Build {
            name: "rec (Junctur) 3".into(),
            replace: false
        }
    );
    assert_eq!(
        decide(&n, "rec (Junctur)", Some(Answer::Replace)).unwrap(),
        Decision::Build {
            name: "rec (Junctur)".into(),
            replace: true
        }
    );
}

#[test]
fn replace_is_refused_when_several_have_the_name_and_names_compare_as_the_script_does() {
    let n = names(&["rec (Junctur)", " REC (JUNCTUR)\t"]);
    let e = decide(&n, "rec (Junctur)", Some(Answer::Replace)).unwrap_err();
    assert!(
        e.contains("2 timelines") && e.contains("nothing was changed"),
        "{e}"
    );
    // Create new still works, and asking still asks.
    assert_eq!(
        decide(&n, "rec (Junctur)", Some(Answer::New)).unwrap(),
        Decision::Build {
            name: "rec (Junctur) 2".into(),
            replace: false
        }
    );
    assert_eq!(decide(&n, "rec (Junctur)", None).unwrap(), Decision::Ask);
    // Only ASCII case is folded, as Lua's byte-wise compare does: "É" and "é" differ.
    assert_eq!(
        decide(&names(&["Égal (Junctur)"]), "égal (Junctur)", None).unwrap(),
        Decision::Build {
            name: "égal (Junctur)".into(),
            replace: false
        }
    );
}

#[test]
fn answers_read_as_the_app_sends_them() {
    let a: Answer = serde_json::from_str("\"new\"").unwrap();
    assert_eq!(a, Answer::New);
    let a: Answer = serde_json::from_str("\"replace\"").unwrap();
    assert_eq!(a, Answer::Replace);
    let a: Answer = serde_json::from_str("\"update\"").unwrap();
    assert_eq!(a, Answer::Update);
    assert!(serde_json::from_str::<Answer>("\"overwrite\"").is_err());
}

#[test]
fn update_needs_exactly_one_timeline_of_the_name() {
    let n = names(&["Main", " REC (Junctur)"]);
    assert_eq!(
        decide(&n, "rec (Junctur)", Some(Answer::Update)).unwrap(),
        Decision::Update {
            name: "rec (Junctur)".into()
        }
    );
    // Gone since the pop-up: refused, never built.
    let e = decide(&names(&["Main"]), "rec (Junctur)", Some(Answer::Update)).unwrap_err();
    assert!(
        e.contains("no longer has") && e.contains("nothing was changed"),
        "{e}"
    );
    let n = names(&["rec (Junctur)", "REC (JUNCTUR)"]);
    let e = decide(&n, "rec (Junctur)", Some(Answer::Update)).unwrap_err();
    assert!(e.contains("2 timelines") && e.contains("update"), "{e}");
}

fn timeline(fps: f64, start: i64) -> Timeline {
    Timeline {
        name: "rec (Junctur)".into(),
        id: "T-1".into(),
        fps,
        start,
        end: start + 1,
        width: 1920,
        height: 1080,
        template: String::new(),
        template_fps: fps,
    }
}

#[test]
fn a_cut_that_changed_is_said_and_the_same_cut_is_not() {
    let mut edit = EditList {
        source: "rec.mp4".into(),
        duration: 60.0,
        settings: Default::default(),
        keep: vec![
            crate::edit::Range {
                start: 0.0,
                end: 10.0,
            },
            crate::edit::Range {
                start: 20.0,
                end: 25.0,
            },
        ],
        speakers: Default::default(),
        captions: None,
        fixes: Vec::new(),
        times: Vec::new(),
        windows: Vec::new(),
    };
    // 15 s at 24 fps is 360 frames; Resolve's end one frame either way is the same cut.
    let t = timeline(24.0, 86400);
    assert_eq!(cut_note(&edit, &t, Some(86400 + 360)), None);
    assert_eq!(cut_note(&edit, &t, Some(86400 + 361)), None);
    assert_eq!(cut_note(&edit, &t, None), None);
    let note = cut_note(&edit, &t, Some(86400 + 240)).unwrap();
    assert!(
        note.contains("\"rec (Junctur)\"'s clips end 10.00 s in")
            && note.contains("Junctur's cut is 15.00 s long")
            && note.contains("Replace")
            && !note.contains("changed"),
        "{note}"
    );
    // NTSC: the timeline's 29.97 is taken back to 30000/1001, the FCPXML's grid. 1 s to 2 s
    // is frames 29 (29.97 floored) to 60 (59.94 ceiled) there: 31, where 30 fps gives 30.
    edit.keep = vec![crate::edit::Range {
        start: 1.0,
        end: 2.0,
    }];
    let t = timeline(30000.0 / 1001.0, 0);
    assert_eq!(cut_frames(&edit, t.fps), 31);
    assert_eq!(cut_frames(&edit, 30.0), 30);
    assert_eq!(cut_note(&edit, &t, Some(31)), None);
    assert!(cut_note(&edit, &t, Some(36)).is_some());
}
