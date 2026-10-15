use super::*;
use crate::test_support::scratch;
use junctur_core::edit::{CutSettings, EditList, Range};
use junctur_core::keep::{Judged, Judgement, Kind};

fn project(name: &str) -> (PathBuf, PathBuf, Project) {
    let d = scratch("keep", name);
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
    (d, file, p)
}

fn list(p: &Project, keep: &[(f64, f64)]) {
    let e = EditList {
        source: p.file.clone(),
        duration: 40.0,
        settings: CutSettings::default(),
        keep: keep
            .iter()
            .map(|&(start, end)| Range { start, end })
            .collect(),
        speakers: Default::default(),
        captions: Default::default(),
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![],
    };
    std::fs::write(p.edit_path(), e.to_text()).unwrap();
}

fn part(start: f64, end: f64, kind: Kind, margin: f64) -> Judgement {
    Judgement {
        start,
        end,
        kind,
        margin,
        state: String::new(),
        hash: format!("{start}"),
    }
}

#[test]
fn before_laya_judged_the_view_has_the_prompt_and_no_suggestion() {
    let (d, file, p) = project("fresh");
    let v = view(&file, Some(d.join("cache")), None, None).unwrap();
    assert_eq!((v.prompt, v.judged_prompt, v.parts), (None, None, 0));
    assert!(v.items.is_empty() && !v.transcribed);
    assert_eq!(v.presets.len(), keep::PRESETS.len());
    let mine = keep::Choices {
        prompt: Some("League".into()),
        answers: vec![],
    };
    keep::save_choices(&p.dir, &mine).unwrap();
    let run = Run {
        file: file.display().to_string(),
        running: true,
        line: Some("keep: 10% judged".into()),
        error: None,
    };
    let v = view(&file, Some(d.join("cache")), Some(run.clone()), None).unwrap();
    assert_eq!(v.prompt.as_deref(), Some("League"));
    assert_eq!(v.run, Some(run));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_judged_recording_shows_its_suggestions_and_its_hand_edits_become_pairs() {
    let (d, file, p) = project("judged");
    // No speech found: the list itself stands for the automatic cut, so nothing is a hand edit yet.
    list(&p, &[(10.0, 20.0), (30.0, 40.0)]);
    let j = Judged {
        model: "laya".into(),
        prompt: "League".into(),
        version: 1,
        parts: vec![
            part(0.0, 10.0, Kind::Silent, 2.0),
            part(10.0, 20.0, Kind::Talk, 1.0),
            part(20.0, 30.0, Kind::Silent, -1.0),
            part(30.0, 40.0, Kind::Talk, -2.0),
        ],
    };
    keep::save(&p.dir, &j).unwrap();
    let pairs = d.join("pairs.json");
    let v = view(&file, Some(d.join("cache")), None, Some(&pairs)).unwrap();
    assert_eq!((v.judged_prompt.as_deref(), v.parts), (Some("League"), 4));
    let at: Vec<(f64, bool)> = v.items.iter().map(|i| (i.start, i.keep)).collect();
    assert_eq!(at, [(0.0, true), (30.0, false)]);
    assert_eq!((v.summary.keep, v.summary.cut), (1, 1));
    assert!(keep::load_pairs(&pairs).is_empty());
    let _ = std::fs::remove_dir_all(&d);
}
