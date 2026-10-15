use super::*;

fn labelled(start: f64, end: f64) -> Labelled {
    Labelled {
        start,
        end,
        frames: 2,
        top: vec![Share {
            label: "fighting".into(),
            score: 0.6,
        }],
        busy: Some(0.8),
        level_db: Some(-21.5),
    }
}

fn file(settings: &Settings, pieces: &[Piece]) -> Labels {
    let list = settings.labels();
    Labels {
        model: model::NAME.into(),
        kind: settings.kind,
        labels: list.iter().map(|l| l.text.clone()).collect(),
        prompts: list.iter().map(|l| l.prompt.clone()).collect(),
        busy_track: None,
        pieces: pieces.iter().map(|p| labelled(p.start, p.end)).collect(),
    }
}

#[test]
fn the_file_reads_back_and_a_broken_one_is_none() {
    let d = crate::test_support::scratch("labels", "file");
    let pieces = [Piece {
        start: 2.25,
        end: 9.0 + 1.0 / 3.0,
    }];
    let l = file(&Settings::default(), &pieces);
    save(&d, &l).unwrap();
    assert_eq!(load(&d), Some(l));
    std::fs::write(d.join(FILE), b"{").unwrap();
    assert_eq!(load(&d), None);
    let _ = std::fs::remove_dir_all(&d);
}

/// 3b reads these names; a rename is a change for Laya's input too.
#[test]
fn the_file_is_shaped_as_laya_reads_it() {
    let mut l = file(
        &Settings::default(),
        &[Piece {
            start: 1.0,
            end: 3.0,
        }],
    );
    l.busy_track = Some(BusyTrack {
        track: 2,
        role: crate::tracks::Role::GamePc,
    });
    let v: serde_json::Value = serde_json::to_value(&l).unwrap();
    assert_eq!(v["kind"], "generic");
    assert_eq!(v["busy_track"]["role"], "game_pc");
    let p = &v["pieces"][0];
    assert_eq!(p["top"][0]["label"], "fighting");
    assert_eq!(p["busy"], 0.8);
    assert_eq!(p["level_db"], -21.5);
    // No busy track: the fields are left out, not null.
    let mut quiet = labelled(1.0, 3.0);
    quiet.busy = None;
    quiet.level_db = None;
    let q = serde_json::to_value(&quiet).unwrap();
    assert!(q.get("busy").is_none() && q.get("level_db").is_none());
}

#[test]
fn labels_are_reused_only_for_the_same_list_pieces_and_model() {
    let pieces = [
        Piece {
            start: 0.0,
            end: 3.0,
        },
        Piece {
            start: 10.0,
            end: 14.0,
        },
    ];
    let s = Settings {
        on: true,
        kind: Kind::League,
        ..Default::default()
    };
    let l = file(&s, &pieces);
    assert!(answers(&l, &s.labels(), &pieces, None));
    // A label added.
    let mut more = s.clone();
    more.added.league.push("baron".into());
    assert!(!answers(&l, &more.labels(), &pieces, None));
    // Another kind.
    assert!(!answers(&l, &Settings::default().labels(), &pieces, None));
    // A busy track now, where they were made without one (or its read failed).
    let game = Some(BusyTrack {
        track: 1,
        role: crate::tracks::Role::GamePc,
    });
    assert!(!answers(&l, &s.labels(), &pieces, game));
    // The speech moved a piece.
    let moved = [
        pieces[0],
        Piece {
            start: 10.0,
            end: 15.0,
        },
    ];
    assert!(!answers(&l, &s.labels(), &moved, None));
    // Another model.
    let mut other = l.clone();
    other.model = "siglip2-so400m".into();
    assert!(!answers(&other, &s.labels(), &pieces, None));
}

/// `--force` makes them again: labels that answer the settings are not reused.
#[test]
fn forced_labels_are_never_reused() {
    let d = crate::test_support::scratch("labels", "force");
    let report = crate::tracks::TrackReport {
        file: d.join("rec.mp4"),
        duration: 20.0,
        windows: vec![],
        window_seconds: 1.0,
        tracks: vec![],
        logical: vec![],
        speech_error: None,
        from_cache: false,
    };
    let speech = [crate::vad::Region {
        start: 5.0,
        end: 8.0,
    }];
    let s = Settings {
        on: true,
        ..Default::default()
    };
    let l = file(&s, &pieces::pieces(&speech, 20.0));
    save(&d, &l).unwrap();
    let job = |force| Job {
        file: &report.file,
        dir: &d,
        cache_dir: &d,
        report: &report,
        speech: &speech,
        settings: &s,
        device: crate::runtime::DevicePref::Cpu,
        force,
    };
    assert_eq!(reused(&job(false)), Some(l));
    assert_eq!(reused(&job(true)), None);
    let _ = std::fs::remove_dir_all(&d);
}
