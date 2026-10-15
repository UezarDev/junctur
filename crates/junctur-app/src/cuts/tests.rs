use super::*;
use crate::test_support::scratch;
use junctur_core::edit::{CutSettings, EditList, Range};

const FPS: Fps = Fps { num: 30, den: 1 };

fn list(keep: Vec<Range>) -> String {
    EditList {
        source: "rec.mp4".into(),
        duration: 10.0,
        settings: CutSettings::default(),
        keep,
        speakers: Default::default(),
        captions: Default::default(),
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![],
    }
    .to_text()
}

#[test]
fn reads_an_edit_list() {
    let d = scratch("cuts", "read");
    let p = d.join("edit.junctur");
    assert!(read(&p, FPS).is_none(), "no file yet");

    std::fs::write(
        &p,
        list(vec![Range {
            start: 1.0,
            end: 2.0,
        }]),
    )
    .unwrap();
    let e = read(&p, FPS).unwrap();
    assert_eq!(e.timeline.as_ref().unwrap().ranges, vec![(30, 60)]);
    let defaults = edit::Captions {
        on: true,
        chars: 30,
        ..Default::default()
    };
    let s = EditState::new(&e, true, Some(FPS), &EditorStatus::default(), &defaults);
    assert_eq!(s.ranges, vec![30, 60]);
    // No `captions` line: the list follows the defaults.
    assert_eq!((s.captions, s.captions_own), (defaults.clone(), false));
    assert_eq!((s.total_frames, s.output_frames), (300, 30));
    assert_eq!(s.summary.unwrap().ranges, 1);

    // Its own settings, even the built-in ones, win over the defaults.
    let own = EditList {
        captions: Some(edit::Captions::default()),
        ..edit::parse(&list(vec![Range {
            start: 1.0,
            end: 2.0,
        }]))
        .unwrap()
    };
    std::fs::write(&p, own.to_text()).unwrap();
    let e = read(&p, FPS).unwrap();
    let s = EditState::new(&e, true, Some(FPS), &EditorStatus::default(), &defaults);
    assert_eq!(
        (s.captions, s.captions_own),
        (edit::Captions::default(), true)
    );

    std::fs::write(&p, list(vec![])).unwrap();
    let e = read(&p, FPS).unwrap();
    assert!(e.timeline.is_none());
    assert_eq!(e.error.as_deref(), Some("the edit list keeps nothing"));
    assert_eq!(e.summary.unwrap().ranges, 0);

    std::fs::write(&p, "garbage").unwrap();
    let e = read(&p, FPS).unwrap();
    assert!(e.timeline.is_none() && e.error.is_some());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ranges_in_play_survive_a_missing_or_broken_file() {
    let d = scratch("cuts", "merge");
    let p = d.join("edit.junctur");
    std::fs::write(
        &p,
        list(vec![Range {
            start: 1.0,
            end: 2.0,
        }]),
    )
    .unwrap();
    let playing = read(&p, FPS).unwrap();
    let t = playing.timeline.clone().unwrap();

    let gone = merge(&playing, None);
    assert!(Arc::ptr_eq(gone.timeline.as_ref().unwrap(), &t));
    assert!(gone.error.is_some());

    std::fs::write(
        &p,
        "junctur-edit 1
keep 1",
    )
    .unwrap();
    let broken = merge(&playing, read(&p, FPS));
    assert!(Arc::ptr_eq(broken.timeline.as_ref().unwrap(), &t));
    assert!(broken.error.is_some());

    // Same frames: the same timeline (nothing reloads); new frames replace it.
    std::fs::write(
        &p,
        list(vec![Range {
            start: 1.001,
            end: 2.0,
        }]),
    )
    .unwrap();
    let same = merge(&playing, read(&p, FPS));
    assert!(Arc::ptr_eq(same.timeline.as_ref().unwrap(), &t));
    std::fs::write(
        &p,
        list(vec![Range {
            start: 3.0,
            end: 4.0,
        }]),
    )
    .unwrap();
    let new = merge(&playing, read(&p, FPS));
    assert_eq!(new.timeline.unwrap().ranges, vec![(90, 120)]);
    // With nothing in play, a missing file is just no edit list.
    assert!(merge(&Edit::default(), None).timeline.is_none());
    let _ = std::fs::remove_dir_all(&d);
}
