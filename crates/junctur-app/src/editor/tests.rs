use super::*;
use crate::test_support::scratch;
use junctur_core::edit::Captions;
use junctur_core::edit::CutSettings;
use junctur_core::transcribe::Word;

/// The list after a step that must change something.
fn step(e: &mut Editor, l: &EditList, s: Step, fps: Fps) -> EditList {
    e.edit(l, s, fps).unwrap().expect("a change").list
}

const FPS: Fps = Fps { num: 30, den: 1 };

fn r(start: f64, end: f64) -> Range {
    Range { start, end }
}

fn list(keep: Vec<Range>) -> EditList {
    EditList {
        source: "rec.mp4".into(),
        duration: 60.0,
        settings: CutSettings::default(),
        keep,
        speakers: Default::default(),
        captions: Default::default(),
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![],
    }
}

fn frames(l: &EditList, fps: Fps) -> Vec<(u64, u64)> {
    let total = frames_ceil(l.duration, fps.num, fps.den);
    frame_ranges(&l.keep, fps.num, fps.den, total)
}

fn reread(path: &Path) -> EditList {
    edit::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn frame_edits_are_frame_exact_through_the_file() {
    let d = scratch("editor", "frames");
    let path = d.join("edit.junctur");
    for fps in [
        FPS,
        Fps {
            num: 30000,
            den: 1001,
        },
        Fps { num: 60, den: 1 },
    ] {
        let _ = std::fs::remove_file(&path);
        let mut e = Editor::default();
        e.reset(Some(path.clone()));
        let mut l = list(vec![r(0.0, 60.0)]);
        // Every cut and keep, at awkward frames, must leave exactly the frames asked for.
        for (start, end, keep) in [
            (31, 97, false),
            (40, 41, true),
            (500, 503, false),
            (95, 600, true),
        ] {
            l = step(&mut e, &l, Step::Do(Op::Frames { start, end, keep }), fps);
            assert_eq!(reread(&path), l, "the file holds the list in play");
        }
        let total = frames_ceil(60.0, fps.num, fps.den);
        assert_eq!(
            frames(&l, fps),
            vec![(0, 31), (40, 41), (95, total)],
            "{fps:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn undo_redo_and_no_op_edits() {
    let d = scratch("editor", "undo");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let l0 = list(vec![r(1.0, 10.0)]);
    let cut = Step::Do(Op::Frames {
        start: 90,
        end: 120,
        keep: false,
    });
    let l1 = step(&mut e, &l0, cut, FPS);
    assert_eq!(l1.keep, vec![r(1.0, 3.0), r(4.0, 10.0)]);
    assert!(e.can_undo() && !e.can_redo());
    // Cutting what is already cut changes nothing and writes nothing.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(e.edit(&l1, cut, FPS).unwrap().is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

    let back = step(&mut e, &l1, Step::Undo, FPS);
    assert_eq!(back.keep, l0.keep);
    assert_eq!(reread(&path), back);
    assert!(!e.can_undo() && e.can_redo());
    let again = step(&mut e, &back, Step::Redo, FPS);
    assert_eq!(again, l1);
    assert!(e.edit(&again, Step::Redo, FPS).is_err(), "nothing to redo");
    // A new edit after an undo drops the redo branch.
    e.edit(&again, Step::Undo, FPS).unwrap();
    e.edit(
        &back,
        Step::Do(Op::Frames {
            start: 0,
            end: 60,
            keep: true,
        }),
        FPS,
    )
    .unwrap();
    assert!(!e.can_redo());
    e.changed_elsewhere();
    assert!(!e.can_undo());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn several_stretches_are_one_undo_step_or_none() {
    let d = scratch("editor", "many");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let l0 = list(vec![r(0.0, 20.0)]);
    let cut = |start, end| Op::Frames {
        start,
        end,
        keep: false,
    };
    // No ops, or ops that change nothing, write nothing and leave no undo step.
    assert!(e.edit_all(&l0, &[], FPS).unwrap().is_none());
    assert!(!path.exists());
    // Two stretches apart and a third overlapping the first: one save, one undo step.
    let ops = [cut(30, 60), cut(300, 330), cut(45, 90)];
    let l1 = e.edit_all(&l0, &ops, FPS).unwrap().expect("a change").list;
    assert_eq!(l1.keep, vec![r(0.0, 1.0), r(3.0, 10.0), r(11.0, 20.0)]);
    assert_eq!(reread(&path), l1);
    assert!(e.edit_all(&l1, &ops, FPS).unwrap().is_none(), "already cut");
    let back = step(&mut e, &l1, Step::Undo, FPS);
    assert_eq!(back.keep, l0.keep, "one undo puts every stretch back");
    assert!(!e.can_undo() && e.can_redo());
    assert_eq!(step(&mut e, &back, Step::Redo, FPS), l1);

    // A stretch under a frame among others is left out and counted; the rest is one step.
    e.words = Some(Arc::new(Words::from_words(vec![
        Word {
            start: 12.0,
            end: 12.5,
            text: "uno".into(),
            p: 1.0,
            p_min: None,
        },
        Word {
            start: 12.51,
            end: 12.52,
            text: "y".into(),
            p: 1.0,
            p_min: None,
        },
        Word {
            start: 12.53,
            end: 13.0,
            text: "dos".into(),
            p: 1.0,
            p_min: None,
        },
    ])));
    let words = |first, last| Op::Words {
        first,
        last,
        keep: false,
    };
    let edited = e
        .edit_all(&l1, &[cut(450, 480), words(1, 1)], FPS)
        .unwrap()
        .expect("the frame cut is made");
    assert_eq!(edited.skipped, 1);
    let l2 = edited.list;
    assert_eq!(
        l2.keep,
        vec![r(0.0, 1.0), r(3.0, 10.0), r(11.0, 15.0), r(16.0, 20.0)]
    );
    // Only short stretches, or short ones and the rest already done: refused, nothing written.
    let text = std::fs::read_to_string(&path).unwrap();
    let all = e.edit_all(&l2, &[words(1, 1), words(1, 1)], FPS);
    assert!(all.unwrap_err().contains("each of the 2"));
    let rest = e.edit_all(&l2, &[cut(450, 480), words(1, 1)], FPS);
    assert!(rest.unwrap_err().contains("1 of the 2"));
    // A single short stretch keeps the plain refusal.
    assert_eq!(
        e.edit_all(&l2, &[words(1, 1)], FPS).unwrap_err(),
        UNDER_A_FRAME
    );
    // Any other refusal refuses the batch.
    assert!(e
        .edit_all(&l2, &[cut(510, 540), words(7, 9)], FPS)
        .unwrap_err()
        .contains("no such words"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn word_runs_are_sorted_joined_and_checked() {
    assert_eq!(
        join_runs(&[(20, 25), (3, 5), (4, 9), (10, 10), (30, 30)]).unwrap(),
        vec![(3, 10), (20, 25), (30, 30)],
        "overlapping and touching runs join, in order"
    );
    assert_eq!(join_runs(&[(7, 7), (7, 7)]).unwrap(), vec![(7, 7)]);
    assert_eq!(join_runs(&[(5, 9), (6, 7)]).unwrap(), vec![(5, 9)]);
    assert!(join_runs(&[]).is_err());
    assert!(join_runs(&[(1, 2), (9, 4)])
        .unwrap_err()
        .contains("backwards"));
}

#[test]
fn backs_up_a_list_it_did_not_write_once() {
    let d = scratch("editor", "bak");
    let path = d.join("edit.junctur");
    let l0 = list(vec![r(1.0, 10.0)]);
    std::fs::write(&path, l0.to_text()).unwrap();
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let op = |start| {
        Step::Do(Op::Frames {
            start,
            end: start + 3,
            keep: false,
        })
    };
    let l1 = step(&mut e, &l0, op(60), FPS);
    let l2 = step(&mut e, &l1, op(90), FPS);
    e.edit(&l2, Step::Undo, FPS).unwrap();
    assert_eq!(
        std::fs::read_to_string(d.join("edit.junctur.bak")).unwrap(),
        l0.to_text(),
        "the reduction's list is kept"
    );
    assert!(
        !d.join("edit.junctur.bak2").exists(),
        "the app's own saves are not"
    );
    assert!(e.is_own(&std::fs::read_to_string(&path).unwrap()));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn word_edits_snap_and_need_a_transcript() {
    let d = scratch("editor", "words");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let l0 = list(vec![r(9.0, 14.0)]);
    let cut = Step::Do(Op::Words {
        first: 1,
        last: 1,
        keep: false,
    });
    assert!(e.edit(&l0, cut, FPS).unwrap_err().contains("no transcript"));
    let w = |start, end, text: &str| Word {
        start,
        end,
        text: text.into(),
        p: 1.0,
        p_min: None,
    };
    e.words = Some(Arc::new(Words::from_words(vec![
        w(10.0, 10.4, "uno"),
        w(10.5, 11.0, "dos"),
        w(12.0, 12.5, "tres"),
        w(12.51, 12.52, "y"),
        w(12.53, 13.0, "z"),
    ])));
    let edited = e.edit(&l0, cut, FPS).unwrap().unwrap();
    assert!(edited.unsnapped, "no envelope: pads only, and said so");
    let l1 = edited.list;
    // "dos" cut from the pause after "uno" to 0.25 s before "tres"; neither neighbour clipped.
    assert_eq!(l1.keep, vec![r(9.0, 10.5), r(11.75, 14.0)]);
    e.envelope = Some(Arc::new(Envelope::default()));
    let keep = Step::Do(Op::Words {
        first: 1,
        last: 1,
        keep: true,
    });
    let edited = e.edit(&l1, keep, FPS).unwrap().unwrap();
    assert!(!edited.unsnapped);
    let l2 = edited.list;
    // Kept again with its own pads: 0.35 s after it, short of the old cut's end at 11.75 s
    // (restoring the whole cut stretch is a frame edit of that stretch).
    assert_eq!(l2.keep, vec![r(9.0, 11.35), r(11.75, 14.0)]);
    // "y" lasts 10 ms between two words with no room to pad: under a frame, refused rather
    // than joined back silently.
    let tiny = Step::Do(Op::Words {
        first: 3,
        last: 3,
        keep: false,
    });
    assert!(e.edit(&l2, tiny, FPS).unwrap_err().contains("frame"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn renames_are_saved_and_survive_undo() {
    use junctur_core::edit::{Speakers, Turn};
    let d = scratch("editor", "rename");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let mut l0 = list(vec![r(0.0, 30.0)]);
    l0.speakers = Speakers::new(
        vec![Turn {
            start: 1.0,
            end: 5.0,
            speaker: 1,
        }],
        &[None],
        None,
    );
    let l1 = step(
        &mut e,
        &l0,
        Step::Do(Op::Frames {
            start: 30,
            end: 60,
            keep: false,
        }),
        FPS,
    );
    let renamed = e.rename(&l1, 1, "  Ana María ").unwrap().unwrap().list;
    assert_eq!(renamed.speakers.name(1), Some("Ana María"));
    assert_eq!(reread(&path), renamed);
    assert!(e.is_own(&std::fs::read_to_string(&path).unwrap()));
    // The same name again writes nothing; bad names and ids are refused.
    assert!(e.rename(&renamed, 1, "Ana María").unwrap().is_none());
    assert!(e.rename(&renamed, 1, "  ").is_err());
    assert!(e.rename(&renamed, 1, "a\nb").is_err());
    assert!(e
        .rename(&renamed, 1, &"x".repeat(edit::MAX_NAME + 1))
        .is_err());
    assert!(e.rename(&renamed, 2, "Bea").is_err());
    assert!(e.rename(&renamed, 1, "track=2 Bob").is_err());
    assert!(e.rename(&renamed, 1, "color=#FF0000 Bob").is_err());
    let bob = e.rename(&renamed, 1, "Bob track=2").unwrap().unwrap().list;
    // Colours and caption settings save the same way, also not undo steps.
    let renamed = e.rename(&bob, 1, "Ana María").unwrap().unwrap().list;
    let red = e.recolor(&renamed, 1, " #ff0000").unwrap().unwrap().list;
    assert_eq!(red.speakers.color(1), Some("#FF0000"));
    assert!(e.recolor(&red, 1, "#FF0000").unwrap().is_none());
    assert!(e.recolor(&red, 1, "red").is_err());
    assert!(e.recolor(&red, 2, "#00FF00").is_err());
    let on = Captions {
        on: true,
        ..Captions::default()
    };
    let captioned = e.set_captions(&red, on.clone()).unwrap().unwrap().list;
    assert_eq!(reread(&path).captions, Some(on.clone()));
    assert!(e
        .set_captions(&captioned, Captions { lines: 9, ..on })
        .is_err());
    let renamed = captioned;
    // Undoing the cut keeps the name.
    let undone = step(&mut e, &renamed, Step::Undo, FPS);
    assert_eq!(undone.keep, l0.keep);
    assert_eq!(undone.speakers.name(1), Some("Ana María"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_fixed_word_is_saved_at_once_and_outlives_an_undo() {
    let d = scratch("editor", "fix");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let heard = Word {
        start: 12.34,
        end: 12.8,
        text: " Wezar,".into(),
        p: 0.6,
        p_min: None,
    };
    let l0 = list(vec![r(1.0, 30.0)]);
    let l1 = e
        .fix_word(&l0, 1, &heard, 0, "Uezar,")
        .unwrap()
        .expect("a change")
        .list;
    assert_eq!(l1.fixes[0].text, "Uezar,");
    assert_eq!(reread(&path), l1, "saved at once");
    assert!(!e.can_undo(), "not an undo step");
    assert!(e.fix_word(&l1, 1, &heard, 0, " Uezar, ").unwrap().is_none());
    // A cut, then its undo: the fix stays, as a rename does.
    let cut = Step::Do(Op::Frames {
        start: 90,
        end: 120,
        keep: false,
    });
    let l2 = step(&mut e, &l1, cut, FPS);
    let back = step(&mut e, &l2, Step::Undo, FPS);
    assert_eq!(back.keep, l0.keep);
    assert_eq!(reread(&path).fixes, l1.fixes);
    // The heard spelling again takes the fix away.
    let l3 = e
        .fix_word(&back, 1, &heard, 0, "Wezar,")
        .unwrap()
        .unwrap()
        .list;
    assert!(l3.fixes.is_empty());
    // Confirmed, the word keeps its heard text in a fix line, saved at once.
    let l4 = e.confirm_word(&l3, 1, &heard, 0).unwrap().unwrap().list;
    assert_eq!(l4.fixes[0].text, "Wezar,");
    assert_eq!(reread(&path), l4);
    assert!(!e.can_undo(), "not an undo step");
    assert!(e.confirm_word(&l4, 1, &heard, 0).unwrap().is_none(), "once");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_fix_lands_on_every_word_heard_alike_and_those_can_be_taken_back() {
    use super::Words;
    let d = scratch("editor", "spread");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let w = |start: f64, text: &str| Word {
        start,
        end: start + 0.3,
        text: format!(" {text}"),
        p: 1.0,
        p_min: None,
    };
    e.words = Some(Arc::new(Words::from_tracked(vec![
        (0, w(2.0, "Bane")),
        (1, w(3.0, "bane,")),
        (0, w(4.0, "Bane")),
        (0, w(5.0, "the")),
    ])));
    let words = e.words.as_ref().unwrap().list.clone();
    let l0 = list(vec![r(1.0, 30.0)]);
    let edited = e
        .fix_word(&l0, 0, &words[0], 0, "Vayne")
        .unwrap()
        .expect("a change");
    let l1 = edited.list;
    assert_eq!(
        l1.fixes.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
        ["Vayne", "Vayne,", "Vayne"]
    );
    assert_eq!(edited.others.len(), 2);
    assert_eq!(reread(&path), l1, "one save");
    assert!(!e.can_undo(), "not an undo step");
    assert_eq!(edited.heard, ["bane,", "Bane"]);
    // Correct's first step finds nothing more to spread.
    let (none, s) = e.spread_all(&l1).unwrap();
    assert!(none.is_none() && s.added.is_empty());
    // The note's Undo: the two others say what was heard again, the one typed stays.
    let (l2, n) = e.keep_heard(&l1, &edited.others, &edited.heard).unwrap();
    let l2 = l2.expect("a change").list;
    assert_eq!(n, 2);
    assert_eq!(
        l2.fixes.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
        ["Vayne", "bane,", "Bane"]
    );
    assert_eq!(reread(&path), l2);
    // And it sticks: Correct's first step does not give them the fix again.
    let (none, s) = e.spread_all(&l2).unwrap();
    assert!(none.is_none() && s.added.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_retyped_sentence_is_saved_at_once_and_replaces_its_word_fixes() {
    // One fix for the run, saved as a `span` line; the word fix inside it goes.
    use super::Words;
    let d = scratch("editor", "span");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let w = |start: f64, text: &str| Word {
        start,
        end: start + 0.3,
        text: format!(" {text}"),
        p: 1.0,
        p_min: None,
    };
    let words = Words::from_tracked(vec![
        (0, w(2.0, "the")),
        (0, w(2.5, "wezar")),
        (0, w(3.0, "went")),
    ]);
    let l0 = list(vec![r(1.0, 30.0)]);
    let l1 = e
        .fix_word(&l0, 0, &words.list[1], 0, "Uezar")
        .unwrap()
        .unwrap()
        .list;
    let run = words
        .retyped(0, 2, "Uezar left.", &Default::default(), &[])
        .unwrap();
    let l2 = e
        .fix_words(&l1, &run, "Uezar left.")
        .unwrap()
        .expect("a change")
        .list;
    assert_eq!(l2.fixes.len(), 1, "{:?}", l2.fixes);
    assert!(l2.fixes[0].is_span());
    assert_eq!(reread(&path), l2, "saved at once");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("\nspan 0 2.000 3.000 Uezar left.\n"));
    assert!(!e.can_undo(), "not an undo step");
    // The heard words again take it away.
    let l3 = e
        .fix_words(&l2, &run, "the  wezar went")
        .unwrap()
        .unwrap()
        .list;
    assert!(l3.fixes.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_moved_edge_moves_the_words_beside_it_and_undoes_with_it() {
    // Kept 10.0-11.0 at 30 fps; moving the end out to 11.4 would keep "tres" (10.9-11.6) by its
    // middle: "dos" follows the edge, "tres" starts at it and stays cut.
    let d = scratch("editor", "edge");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path.clone()));
    let w = |start, end, text: &str| Word {
        start,
        end,
        text: text.into(),
        p: 1.0,
        p_min: None,
    };
    e.words = Some(Arc::new(Words::from_words(vec![
        w(9.5, 9.95, "pre"),
        w(10.0, 10.2, "uno"),
        w(10.2, 10.8, "dos"),
        w(10.9, 11.6, "tres"),
    ])));
    let l0 = list(vec![r(10.0, 11.0)]);
    let spans = |l: &EditList| -> Vec<(f64, f64, f64)> {
        l.times.iter().map(|t| (t.start, t.from, t.to)).collect()
    };
    let l1 = e
        .move_edge(&l0, (330, 342, true), 330, FPS)
        .unwrap()
        .expect("a change")
        .list;
    assert_eq!(frames(&l1, FPS), vec![(300, 342)]);
    assert_eq!(spans(&l1), [(10.2, 10.2, 11.4), (10.9, 11.4, 11.6)]);
    assert_eq!(reread(&path), l1, "saved with the frames");
    // What the page gets: "tres" now starts at the edge, its middle past it.
    let v = TranscriptView::new(
        e.words.as_ref().unwrap(),
        &Default::default(),
        &[],
        &l1.times,
    );
    assert_eq!((v.start[2], v.end[2], v.start[3]), (10_200, 11_400, 11_400));
    // One undo puts the frames and the words back; redo both again.
    let back = step(&mut e, &l1, Step::Undo, FPS);
    assert_eq!(back.keep, l0.keep);
    assert!(back.times.is_empty());
    let again = step(&mut e, &back, Step::Redo, FPS);
    assert_eq!(again, l1);
    // Moving it back in a little: "dos", now running into it, ends at it; "tres" is not moved.
    let l2 = e
        .move_edge(&l1, (336, 342, false), 342, FPS)
        .unwrap()
        .unwrap()
        .list;
    assert_eq!(spans(&l2), [(10.2, 10.2, 11.2), (10.9, 11.4, 11.6)]);
    // The start out to 9.8 (frame 294): "uno" starts there, "pre" ends there and stays cut.
    let l3 = e
        .move_edge(&l2, (294, 300, true), 300, FPS)
        .unwrap()
        .unwrap()
        .list;
    assert_eq!(frames(&l3, FPS), vec![(294, 336)]);
    assert_eq!(
        spans(&l3),
        [
            (9.5, 9.5, 9.8),
            (10.0, 9.8, 10.2),
            (10.2, 10.2, 11.2),
            (10.9, 11.4, 11.6)
        ]
    );
    // A frame edit that moves no edge of the list (inside a range) leaves the words.
    let l4 = e
        .move_edge(&l3, (310, 312, false), 310, FPS)
        .unwrap()
        .unwrap()
        .list;
    assert_eq!(l4.times, l3.times);
    // Without a transcript it is the frame edit alone.
    e.words = None;
    let l5 = e
        .move_edge(&l0, (330, 342, true), 330, FPS)
        .unwrap()
        .unwrap()
        .list;
    assert!(l5.times.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_word_far_from_the_edge_is_not_stretched_and_one_passed_whole_turns_kept() {
    // A word not beside the edge is left as it is; an end moved over a whole short word keeps
    // that word, the word following the edge stopping at it.
    let d = scratch("editor", "edge-far");
    let path = d.join("edit.junctur");
    let mut e = Editor::default();
    e.reset(Some(path));
    let w = |start, end| Word {
        start,
        end,
        text: "w".into(),
        p: 1.0,
        p_min: None,
    };
    e.words = Some(Arc::new(Words::from_words(vec![
        w(14.0, 15.0),
        w(21.0, 21.5),
    ])));
    let l = list(vec![r(10.0, 20.0)]);
    let l1 = e
        .move_edge(&l, (600, 615, true), 600, FPS)
        .unwrap()
        .unwrap()
        .list;
    assert!(l1.times.is_empty(), "{:?}", l1.times);
    e.words = Some(Arc::new(Words::from_words(vec![
        w(10.0, 10.8),
        w(10.9, 11.0),
        w(11.5, 12.0),
    ])));
    let l = list(vec![r(10.0, 10.85)]);
    // 10.85 ends in frame 326; out to 11.1 (frame 333), past the word at 10.9-11.0 whole.
    let l2 = e
        .move_edge(&l, (326, 333, true), 326, FPS)
        .unwrap()
        .unwrap()
        .list;
    let t: Vec<_> = l2.times.iter().map(|t| (t.start, t.from, t.to)).collect();
    assert_eq!(t, [(10.0, 10.0, 10.9)]);
}

#[test]
fn an_edge_moved_across_a_cut_into_the_next_range_trims_none_of_its_words() {
    // Kept 10.0-11.0 and 11.5-20.0; the first end stepped to 11.7 joins them. The word in the
    // second range keeps its head, and the one before follows the edge only to where it starts.
    let d = scratch("editor", "edge-join");
    let mut e = Editor::default();
    e.reset(Some(d.join("edit.junctur")));
    let w = |start, end| Word {
        start,
        end,
        text: "w".into(),
        p: 1.0,
        p_min: None,
    };
    e.words = Some(Arc::new(Words::from_words(vec![
        w(10.2, 10.8),
        w(11.6, 11.9),
    ])));
    let l = list(vec![r(10.0, 11.0), r(11.5, 20.0)]);
    let l1 = e
        .move_edge(&l, (330, 351, true), 330, FPS)
        .unwrap()
        .unwrap()
        .list;
    let t: Vec<_> = l1.times.iter().map(|t| (t.start, t.from, t.to)).collect();
    assert_eq!(t, [(10.2, 10.2, 11.5)]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_word_left_with_under_a_frame_kept_is_cut_whole() {
    // Kept 10.0-11.0; the end moved in to 10.7 leaves "z" under a frame on the kept side: it is
    // cut, its head clipped to the edge.
    let d = scratch("editor", "edge-straddle");
    let mut e = Editor::default();
    e.reset(Some(d.join("edit.junctur")));
    e.words = Some(Arc::new(Words::from_words(vec![Word {
        start: 10.69,
        end: 10.9,
        text: "z".into(),
        p: 1.0,
        p_min: None,
    }])));
    let l = list(vec![r(10.0, 11.0)]);
    let l1 = e
        .move_edge(&l, (321, 330, false), 330, FPS)
        .unwrap()
        .unwrap()
        .list;
    let t: Vec<_> = l1.times.iter().map(|t| (t.start, t.from, t.to)).collect();
    assert_eq!(t, [(10.69, 10.7, 10.9)]);
    let _ = std::fs::remove_dir_all(&d);
}
