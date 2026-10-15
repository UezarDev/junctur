use super::*;
use crate::test_support::scratch;
use junctur_core::edit::{CutSettings, EditList, Range};
use junctur_core::keep::{rules, Judged, Judgement, Kind};

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
        hash: String::new(),
    }
}

#[test]
fn before_smart_keep_ran_the_view_has_the_prompt_and_no_suggestion() {
    let (d, file, p) = project("fresh");
    let v = view(&file, Some(d.join("cache")), None).unwrap();
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
        line: Some("keep: starting".into()),
        error: None,
    };
    let v = view(&file, Some(d.join("cache")), Some(run.clone())).unwrap();
    assert_eq!(v.prompt.as_deref(), Some("League"));
    assert_eq!(v.run, Some(run));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_rules_keeps_are_suggested_and_a_laya_file_shows_nothing() {
    let (d, file, p) = project("judged");
    // No speech found: the list itself stands for the automatic cut, so nothing is a hand edit yet.
    list(&p, &[(10.0, 20.0), (30.0, 40.0)]);
    let mut j = Judged {
        model: "laya-multilingual".into(),
        prompt: "League highlights".into(),
        version: 2,
        parts: vec![
            part(0.0, 10.0, Kind::Silent, 2.0),
            part(10.0, 20.0, Kind::Talk, -2.0),
            part(20.0, 30.0, Kind::Silent, 0.1),
        ],
    };
    keep::save_choices(
        &p.dir,
        &keep::Choices {
            prompt: Some("League highlights".into()),
            answers: vec![],
        },
    )
    .unwrap();
    // 3b's file, with a cut of talk in it: not read, nothing suggested.
    keep::save(&p.dir, &j).unwrap();
    let v = view(&file, Some(d.join("cache")), None).unwrap();
    assert_eq!((v.judged_prompt, v.parts, v.items.len()), (None, 0, 0));
    // The rules' spans are keeps of silence only.
    j.model = rules::NAME.into();
    j.version = rules::VERSION;
    j.parts.remove(1);
    keep::save(&p.dir, &j).unwrap();
    let v = view(&file, Some(d.join("cache")), None).unwrap();
    assert_eq!(
        (v.judged_prompt.as_deref(), v.parts),
        (Some("League highlights"), 2)
    );
    assert_eq!(
        v.reading.as_deref(),
        Some("strict, every silent stretch holding the busiest 10% of the silent windows")
    );
    let at: Vec<(f64, bool, bool)> = v
        .items
        .iter()
        .map(|i| (i.start, i.keep, i.uncertain))
        .collect();
    assert_eq!(at, [(0.0, true, false), (20.0, true, true)]);
    assert_eq!(
        (v.summary.keep, v.summary.cut, v.summary.uncertain),
        (2, 0, 1)
    );
    assert!(!v.outdated);
    // The prompt changed since it ran: its stretches are not shown, and the view says why (3e).
    keep::save_choices(
        &p.dir,
        &keep::Choices {
            prompt: Some("A podcast".into()),
            answers: vec![],
        },
    )
    .unwrap();
    let v = view(&file, Some(d.join("cache")), None).unwrap();
    assert!(v.outdated && v.items.is_empty() && v.judged_prompt.is_none());
    let _ = std::fs::remove_dir_all(&d);
}
