use super::super::{CutSettings, Range};
use super::*;
use crate::test_support::scratch;

fn list(end: f64) -> EditList {
    EditList {
        source: "in.mp4".into(),
        duration: 10.0,
        settings: CutSettings::default(),
        keep: vec![Range { start: 0.0, end }],
        speakers: Default::default(),
        captions: Default::default(),
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![],
    }
}

#[test]
fn backs_up_only_what_someone_else_wrote() {
    let dir = scratch("save", "bak");
    let path = dir.join("edit.junctur");
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();

    // Nothing there yet: nothing to keep.
    save(&path, &list(1.0)).unwrap();
    assert_eq!(read("edit.junctur.bak"), None);
    // An untouched save (this session's or an earlier one's) is replaced without a backup.
    let second = save(&path, &list(2.0)).unwrap();
    assert_eq!(second.backup, None);
    assert_eq!(read("edit.junctur").as_deref(), Some(second.text.as_str()));
    assert_eq!(super::super::parse(&second.text).unwrap(), list(2.0));
    // A hand edit is kept (its stamp no longer matches), the next one goes to .bak2, and one
    // that does not parse is said so.
    let hand = second.text.replace("keep 0.000 2.000", "keep 0.000 2.500");
    std::fs::write(&path, &hand).unwrap();
    let third = save(&path, &list(3.0)).unwrap();
    assert_eq!(read("edit.junctur.bak").as_deref(), Some(hand.as_str()));
    assert!(!third.replaced_unreadable);
    std::fs::write(&path, "hand edit 2").unwrap();
    let fourth = save(&path, &list(4.0)).unwrap();
    assert_eq!(read("edit.junctur.bak2").as_deref(), Some("hand edit 2"));
    assert!(fourth.replaced_unreadable);
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.ends_with(".tmp")),
        "renamed away: {names:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn amend_rewrites_only_what_loses_nothing() {
    use super::super::Window;
    let dir = scratch("save", "amend");
    let path = dir.join("edit.junctur");
    let counted = |e: &mut EditList| {
        e.windows = vec![Window {
            start: 0.0,
            score: 1.0,
            speech: 1.0,
            words: 7,
        }]
    };
    assert_eq!(amend(&path, counted).unwrap(), Amended::Missing);
    // As the pipeline writes it: written plain, nothing backed up.
    std::fs::write(&path, list(5.0).to_text()).unwrap();
    let Amended::Written(e) = amend(&path, counted).unwrap() else {
        panic!("not written")
    };
    assert_eq!(e.windows[0].words, 7);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!is_untouched_save(&text) && text == e.to_text());
    assert_eq!(amend(&path, counted).unwrap(), Amended::Same(e));
    // An editor save (the user cut while it transcribed; its first save keeps the pipeline's
    // list as a backup, as ever): their ranges kept, still an untouched save, so neither the
    // amend nor the editor's next save backs anything up.
    save(&path, &list(2.0)).unwrap();
    std::fs::remove_file(dir.join("edit.junctur.bak")).unwrap();
    let Amended::Written(e) = amend(&path, counted).unwrap() else {
        panic!("not written")
    };
    assert_eq!((e.keep[0].end, e.windows.len()), (2.0, 1));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(is_untouched_save(&text));
    assert_eq!(save(&path, &list(1.5)).unwrap().backup, None);
    assert!(!dir.join("edit.junctur.bak").exists());
    // A hand edit with a comment, or one that does not parse: left alone.
    let hand = format!("# mine\n{}", list(3.0).to_text());
    std::fs::write(&path, &hand).unwrap();
    assert_eq!(amend(&path, counted).unwrap(), Amended::Left);
    std::fs::write(&path, "keep x").unwrap();
    assert_eq!(amend(&path, counted).unwrap(), Amended::Left);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep x");
    assert!(!dir.join("edit.junctur.bak").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_list_the_parser_would_refuse_is_never_written() {
    // A kept-as-heard span longer than a span's text may be would fail the next open.
    use super::super::Fix;
    let dir = scratch("save", "refused");
    let path = dir.join("edit.junctur");
    let mut bad = list(5.0);
    bad.fixes = vec![Fix {
        track: 0,
        start: 1.0,
        nth: 0,
        through: Some(super::super::WordAt { start: 2.0, nth: 0 }),
        text: "x".repeat(super::super::MAX_SPAN + 1),
    }];
    assert!(bad.checked_text().is_err());
    let e = save(&path, &bad).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    assert!(!path.exists(), "nothing written");
    // As the pipeline writes it, and as an editor save: amend refuses too, the file as it was.
    for first in [list(5.0).to_text(), save(&path, &list(5.0)).unwrap().text] {
        std::fs::write(&path, &first).unwrap();
        let e = amend(&path, |e| e.fixes = bad.fixes.clone()).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_retried_save_does_not_pile_up_backups() {
    let dir = scratch("save", "retry");
    let path = dir.join("edit.junctur");
    std::fs::write(&path, "hand").unwrap();
    // An earlier attempt backed it up, then failed to rename: the list is still the hand edit.
    std::fs::write(dir.join("edit.junctur.bak"), "hand").unwrap();
    let s = save(&path, &list(1.0)).unwrap();
    assert_eq!(s.backup, None);
    assert!(!dir.join("edit.junctur.bak2").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
