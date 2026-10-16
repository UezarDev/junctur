use std::path::PathBuf;

use super::*;
use crate::edit::{CutSettings, SpeakerName, Speakers, Turn};
use crate::llm::by_keywords;
use crate::pipeline::{Speech, SpeechTrack};
use crate::test_support::scratch;
use crate::transcribe::Word;
use crate::vad::Region;

fn r(start: f64, end: f64) -> Range {
    Range { start, end }
}

fn list(keep: Vec<Range>) -> EditList {
    EditList {
        source: PathBuf::from("rec.mkv"),
        duration: 60.0,
        settings: CutSettings::default(),
        keep,
        speakers: Speakers {
            names: vec![SpeakerName {
                id: 1,
                name: "Tsunheto".into(),
                track: None,
                color: "#F2B33D".into(),
            }],
            turns: vec![Turn {
                start: 10.0,
                end: 20.0,
                speaker: 1,
            }],
        },
        captions: None,
        fixes: Vec::new(),
        times: Vec::new(),
        windows: Vec::new(),
    }
}

/// A project folder with speech at 10-20 and 40-50 s of a minute.
fn project(name: &str) -> (Project, PathBuf) {
    let d = scratch("ask", name);
    let source = d.join("rec.mkv");
    std::fs::write(&source, b"").unwrap();
    let p = Project::open(&source, Some(d.join("cache")), Some(d.join("proj"))).unwrap();
    let speech = Speech {
        tracks: vec![SpeechTrack {
            track: 0,
            regions: vec![
                Region {
                    start: 10.0,
                    end: 20.0,
                },
                Region {
                    start: 40.0,
                    end: 50.0,
                },
            ],
            share: 0.3,
        }],
        seconds: 60.0,
    };
    std::fs::write(
        p.dir.join("speech.json"),
        serde_json::to_vec(&speech).unwrap(),
    )
    .unwrap();
    (p, d)
}

fn read(request: &str, l: &EditList) -> crate::llm::Read {
    by_keywords(request, &speakers(l), None, None)
}

#[test]
fn silence_over_a_list_that_keeps_everything_is_ghost_cuts_and_accept_and_undo_put_it_back() {
    let (p, d) = project("silence");
    let l = list(vec![r(0.0, 60.0)]);
    let step = propose(
        &p,
        &l,
        "cut the silences",
        None,
        read("cut the silences", &l),
    );
    assert_eq!(step.error, None);
    assert_eq!(step.reading.reader, "keywords");
    // The automatic cut's pads: 0.25 s before speech, 0.35 s after.
    let g: Vec<(f64, f64, bool)> = step
        .ghosts
        .iter()
        .map(|g| (g.start, g.end, g.keep))
        .collect();
    assert_eq!(
        g,
        [
            (0.0, 9.75, false),
            (20.35, 39.75, false),
            (50.35, 60.0, false)
        ]
    );
    let mut ask = Ask::default();
    let id = ask.push(step);
    assert!(ask.waiting());
    assert_eq!(
        ask.preview(&l.keep),
        [r(9.75, 20.35), r(39.75, 50.35)],
        "the preview plays the edit as it would be"
    );
    // Accept the second only, then the rest.
    let e = ask.accept(id, Some(&[1]), &l.keep).unwrap();
    assert_eq!(e, [(20.35, 39.75, false)]);
    let e = ask.accept(id, None, &l.keep).unwrap();
    assert_eq!(e.len(), 2);
    assert!(!ask.waiting());
    assert_eq!(ask.step(id).unwrap().accepted(), 3);
    // The second batch's stretch (0 to 60) holds the first's: it is undone first.
    let first = ask.step(id).unwrap().applied[0].id;
    let second = ask.step(id).unwrap().applied[1].id;
    assert!(ask.undo(id, Some(first)).is_err());
    let undo = ask.undo(id, Some(second)).unwrap();
    assert_eq!(undo, [(0.0, 60.0, false), (0.0, 60.0, true)]);
    let undo = ask.undo(id, Some(first)).unwrap();
    assert_eq!(undo, [(20.35, 39.75, false), (20.35, 39.75, true)]);
    assert!(ask.undo(id, Some(first)).is_err(), "only once");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_ghost_the_list_changed_under_is_not_applied_and_undo_goes_latest_first() {
    let (p, d) = project("stale");
    let l = list(vec![r(0.0, 60.0)]);
    let mut ask = Ask::default();
    let a = ask.push(propose(
        &p,
        &l,
        "cut the silences",
        None,
        read("cut the silences", &l),
    ));
    // A hand cut at 25-30 since: the 20.35-39.75 ghost would bring part of it back... it is stale.
    let now = vec![r(0.0, 25.0), r(30.0, 60.0)];
    assert_eq!(ask.preview(&now), [r(9.75, 25.0), r(30.0, 50.35)]);
    let e = ask.accept(a, None, &now).unwrap();
    assert_eq!(e.len(), 2, "the stale ghost dropped: {e:?}");
    assert_eq!(ask.step(a).unwrap().rejected, 1);
    // Two overlapping batches: the earlier one waits for the later one's undo.
    let b = ask.push(propose(
        &p,
        &l,
        "cut the silences",
        Some((5.0, 9.0)),
        read("cut the silences", &l),
    ));
    let mut ask2 = ask.clone();
    ask2.accept(b, None, &l.keep).unwrap();
    let first = ask2.step(a).unwrap().applied[0].id;
    let second = ask2.step(b).unwrap().applied[0].id;
    assert!(ask2.undo(a, Some(first)).is_err());
    ask2.undo(b, Some(second)).unwrap();
    ask2.undo(a, Some(first)).unwrap();
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_selection_bounds_it_and_what_the_list_already_does_is_no_ghost() {
    let (p, d) = project("selection");
    // 25-30 is cut already.
    let l = list(vec![r(0.0, 25.0), r(30.0, 60.0)]);
    let step = propose(
        &p,
        &l,
        "cut the silences",
        Some((22.0, 35.0)),
        read("cut the silences", &l),
    );
    let g: Vec<(f64, f64)> = step.ghosts.iter().map(|g| (g.start, g.end)).collect();
    assert_eq!(g, [(22.0, 35.0)]);
    // Keeping only what Tsunheto says keeps nothing new there: only cuts.
    let step = propose(
        &p,
        &l,
        "only keep what Tsunheto says",
        None,
        read("only keep what Tsunheto says", &l),
    );
    assert!(step.ghosts.iter().all(|g| !g.keep));
    assert_eq!(step.ghosts.len(), 2);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_tool_that_cannot_answer_and_a_request_that_does_not_read_are_the_steps_error() {
    let (p, d) = project("errors");
    let l = list(vec![r(0.0, 60.0)]);
    let step = propose(
        &p,
        &l,
        "keep a quarter second each time I break an ore",
        None,
        read("keep a quarter second each time I break an ore", &l),
    );
    assert!(
        step.error.as_deref().unwrap().contains("Like this"),
        "{:?}",
        step.error
    );
    assert!(step.recipe.is_some());
    let step = propose(&p, &l, "make it funny", None, read("make it funny", &l));
    assert!(step.recipe.is_none() && step.error.is_some());
    let step = propose(
        &p,
        &l,
        "cut what Bob says",
        None,
        read("cut what \"Bob\" says", &l),
    );
    assert!(step.ghosts.is_empty());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn marks_go_to_moments_once_accepted_and_come_back_on_undo() {
    let mut ask = Ask::default();
    let id = ask.push(Step {
        marks: vec![12.0],
        clips: vec![(30.0, 34.0)],
        ..propose_nothing()
    });
    assert!(ask.markers().is_empty(), "not before Accept");
    assert!(ask.accept(id, None, &[]).unwrap().is_empty());
    let m = ask.markers();
    assert_eq!(m.len(), 2);
    assert_eq!(m[1].span, Some((30.0, 34.0)));
    assert!(ask.accept(id, None, &[]).is_err(), "sent once");
    ask.undo(id, None).unwrap();
    assert!(ask.markers().is_empty());
    ask.reject(id, None).unwrap();
    assert_eq!(ask.step(id).unwrap().rejected, 2);
}

fn propose_nothing() -> Step {
    Step {
        id: 0,
        request: "x".into(),
        range: None,
        recipe: None,
        described: String::new(),
        reading: Reading::by("keywords"),
        error: None,
        found: 0,
        ghosts: Vec::new(),
        marks: Vec::new(),
        clips: Vec::new(),
        sent: false,
        applied: Vec::new(),
        rejected: 0,
        seconds: 0.0,
        notes: Vec::new(),
    }
}

#[test]
fn sayings_and_the_remark_around_them() {
    let w = |start: f64, text: &str| Word {
        start,
        end: start + 0.3,
        text: text.into(),
        p: 0.9,
        p_min: None,
    };
    let words = [
        w(1.0, "hola"),
        w(5.0, "I"),
        w(5.4, "love"),
        w(5.8, "Foods,"),
        w(6.2, "really"),
        w(9.0, "food"),
    ];
    let refs: Vec<&Word> = words.iter().collect();
    assert_eq!(
        signals::sayings(&refs, "food", false),
        [(5.8, 6.1), (9.0, 9.3)]
    );
    assert_eq!(
        signals::sayings(&refs, "food", true),
        [(5.0, 6.5), (9.0, 9.3)]
    );
    assert_eq!(signals::sayings(&refs, "love foods", false), [(5.4, 6.1)]);
    assert!(signals::sayings(&refs, "", false).is_empty());
}

#[test]
fn speakers_by_name() {
    let l = list(vec![]);
    assert_eq!(signals::speaker_ids(&l, "tsunheto").unwrap(), [1]);
    assert_eq!(signals::speaker_ids(&l, "Tsun").unwrap(), [1]);
    assert_eq!(signals::speaker_ids(&l, "anyone").unwrap(), [1]);
    let e = signals::speaker_ids(&l, "Bob").unwrap_err().to_string();
    assert!(e.contains("Tsunheto"), "{e}");
}
