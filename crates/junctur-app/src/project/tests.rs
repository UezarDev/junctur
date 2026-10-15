use super::*;
use crate::test_support::scratch;
use junctur_core::edit::{CutSettings, EditList, Range};
use junctur_core::pipeline::STAGES;

#[test]
fn new_project_has_nothing_yet() {
    let d = scratch("app", "new");
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let s = status(&file, Some(d.join("cache"))).unwrap();
    assert_eq!(s.stages.len(), STAGES.len());
    assert!(s.stages.iter().all(|st| st.state == StageState::Pending));
    assert!(!s.transcript);
    assert_eq!((s.edit, s.edit_error), (None, None));
    assert!(Path::new(&s.dir).is_dir());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn reports_edit_list_and_transcript() {
    let d = scratch("app", "edit");
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
    let e = EditList {
        source: p.file.clone(),
        duration: 60.0,
        settings: CutSettings::default(),
        keep: vec![
            Range {
                start: 0.0,
                end: 10.0,
            },
            Range {
                start: 20.0,
                end: 25.0,
            },
        ],
        speakers: Default::default(),
        captions: Default::default(),
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![],
    };
    std::fs::write(p.edit_path(), e.to_text()).unwrap();
    std::fs::write(transcribe::path(&p.dir), b"{}").unwrap();
    let s = status(&file, Some(d.join("cache"))).unwrap();
    assert!(s.transcript);
    let summary = s.edit.unwrap();
    assert_eq!(summary.ranges, 2);
    assert!((summary.kept - 15.0).abs() < 1e-9);
    assert_eq!(summary.duration, 60.0);

    std::fs::write(p.edit_path(), "garbage").unwrap();
    let s = status(&file, Some(d.join("cache"))).unwrap();
    assert_eq!(s.edit, None);
    assert!(s.edit_error.is_some());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_kept_parse_says_what_parsing_the_list_again_says() {
    let d = scratch("app", "parsed");
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
    let list = |ends: &[f64]| EditList {
        source: p.file.clone(),
        duration: 90.0,
        settings: CutSettings::default(),
        keep: ends
            .iter()
            .map(|&end| Range {
                start: end - 5.0,
                end,
            })
            .collect(),
        speakers: Default::default(),
        captions: Default::default(),
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![],
    };
    // The list read and parsed afresh, with no kept parse.
    let afresh = || match p.edit() {
        Ok(e) => (
            Some(EditSummary {
                ranges: e.keep.len(),
                kept: e.kept(),
                duration: e.duration,
            }),
            None,
        ),
        Err(e) => (None, Some(e.to_string())),
    };
    let texts = [
        list(&[10.0, 30.0]).to_text(),
        list(&[10.0, 30.0]).to_text(),
        // One digit apart: the same length, another summary.
        list(&[10.0, 40.0]).to_text(),
        "garbage".to_string(),
        "junctur-edit 999\n".to_string(),
        list(&[10.0, 30.0, 50.0]).to_text(),
        list(&[10.0, 30.0]).to_text(),
    ];
    for text in &texts {
        std::fs::write(p.edit_path(), text).unwrap();
        for _ in 0..2 {
            let s = status(&file, Some(d.join("cache"))).unwrap();
            assert_eq!((s.edit, s.edit_error), afresh(), "{text}");
        }
    }
    // Not UTF-8: refused as reading it refuses it.
    std::fs::write(p.edit_path(), [0xff, 0xfe, 0x00]).unwrap();
    let s = status(&file, Some(d.join("cache"))).unwrap();
    assert_eq!((s.edit, s.edit_error.clone()), afresh());
    assert!(s.edit_error.is_some());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_recordings_info_and_still() {
    let d = scratch("app", "info");
    let file = d.join("rec.mkv");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=d=3:s=640x360:r=25",
        ])
        .args(["-f", "lavfi", "-i", "sine=d=3", "-shortest"])
        .arg(&file)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let cache = Some(d.join("cache"));
    let i = info(&file, cache.clone()).unwrap();
    assert_eq!((i.ranges, i.stages_done, i.stages), (None, 0, STAGES.len()));
    assert!(
        (i.duration.unwrap() - 3.0).abs() < 0.1,
        "from ffprobe: {i:?}"
    );
    let jpg = thumbnail(&file, cache.clone()).unwrap();
    assert_eq!(&jpg[..2], &[0xFF, 0xD8], "a JPEG");
    let dir = PathBuf::from(status(&file, cache.clone()).unwrap().dir);
    assert!(dir.join(THUMBNAIL).is_file());
    // Made once: what is there is what comes back.
    std::fs::write(dir.join(THUMBNAIL), b"kept").unwrap();
    assert_eq!(thumbnail(&file, cache.clone()).unwrap(), b"kept");
    assert!(thumbnail(&d.join("nope.mkv"), cache).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn missing_file_is_an_error() {
    let d = scratch("app", "missing");
    assert!(status(&d.join("nope.mp4"), Some(d.join("cache"))).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

/// The lane's labels: none before the analysis found the speech and tracks (whatever file is there;
/// the file's own checks are core's, `pipeline::tests`), and a run that had them off says so.
#[test]
fn labels_are_none_before_the_analysis_and_off_is_noted() {
    use junctur_core::labels::{self, Labels};
    let d = scratch("app", "labels");
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let cache = Some(d.join("cache"));
    let settings = labels::Settings::default();
    assert_eq!(labels_with(&file, cache.clone(), &settings).unwrap(), None);
    let p = Project::open(&file, cache.clone(), None).unwrap();
    let l = Labels {
        model: labels::model::NAME.into(),
        kind: labels::Kind::Generic,
        labels: vec![],
        prompts: vec![],
        busy_track: None,
        pieces: vec![],
    };
    labels::save(&p.dir, &l).unwrap();
    assert_eq!(labels_with(&file, cache.clone(), &settings).unwrap(), None);
    let off = Some(pipeline::LABELS_OFF.to_string());
    p.jobs()
        .set(&p.dir, StageName::Labels, StageState::Done, None, off)
        .unwrap();
    let s = status(&file, cache).unwrap();
    let st = s.stages.iter().find(|s| s.name == "labels").unwrap();
    assert_eq!(st.note.as_deref(), Some("off"));
    let _ = std::fs::remove_dir_all(&d);
}
