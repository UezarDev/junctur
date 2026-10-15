use super::*;
use crate::edit::Range;
use crate::export::srt::Part;

/// A request's answer: its fields, or the error it gives.
type Answered = Result<Vec<(&'static str, String)>, String>;

/// A stand-in Resolve: answers per request kind, or the error given for it, and keeps what was
/// asked.
#[derive(Default)]
struct Fake {
    answers: HashMap<&'static str, Answered>,
    asked: Vec<(String, HashMap<&'static str, String>)>,
    written: usize,
}

impl Fake {
    fn new(timelines: &[&str]) -> Self {
        let mut check = vec![
            ("timelines", timelines.len().to_string()),
            ("script", reply::SCRIPT.to_string()),
        ];
        let keys = ["timeline1", "timeline2", "timeline3"];
        for (k, t) in timelines.iter().enumerate() {
            check.push((keys[k], t.to_string()));
        }
        let mut f = Fake::default();
        f.answers.insert("check", Ok(check));
        f.answers.insert("place", Ok(vec![("placed", "1".into())]));
        f
    }
    fn built(self, name: &str) -> Self {
        self.answered("build", name, None)
    }
    /// The timeline already there, as `open` finds it, its cut ending at `cut_end`.
    fn opened(self, name: &str, cut_end: &str) -> Self {
        self.answered("open", name, Some(cut_end))
    }
    fn answered(mut self, kind: &'static str, name: &str, cut_end: Option<&str>) -> Self {
        let mut a = vec![
            ("name", name),
            ("id", "T-9"),
            ("fps", "24"),
            ("start", "0"),
            ("end", "2400"),
            ("width", "1920"),
            ("height", "1080"),
            ("template", "Junctur Caption"),
            ("templateFps", "24"),
        ];
        if let Some(e) = cut_end {
            a.push(("cutEnd", e));
        }
        self.answers.insert(
            kind,
            Ok(a.iter().map(|(k, v)| (*k, v.to_string())).collect()),
        );
        self
    }
    fn kinds(&self) -> Vec<&str> {
        self.asked.iter().map(|(k, _)| k.as_str()).collect()
    }
    fn field(&self, kind: &str, key: &str) -> Option<&str> {
        let (_, f) = self.asked.iter().find(|(k, _)| k == kind)?;
        f.get(key).map(String::as_str)
    }
}

impl Link for Fake {
    fn request(
        &mut self,
        kind: &str,
        fields: Vec<(&'static str, Value)>,
    ) -> Result<HashMap<String, String>, String> {
        let shown = fields
            .into_iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::Str(s) => s,
                    Value::Bool(b) => b.to_string(),
                    _ => "…".into(),
                };
                (k, v)
            })
            .collect();
        self.asked.push((kind.to_string(), shown));
        let a = self
            .answers
            .get(kind)
            .cloned()
            .unwrap_or_else(|| Err(format!("unknown request {kind}")))?;
        Ok(a.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    fn write_timeline(&mut self, _: &EditList) -> Result<PathBuf, String> {
        self.written += 1;
        Ok(PathBuf::from(r"C:\J\timeline.fcpxml"))
    }
}

fn edit() -> EditList {
    EditList {
        source: r"C:\Videos\rec.mp4".into(),
        duration: 10.0,
        settings: Default::default(),
        keep: vec![Range {
            start: 0.0,
            end: 5.0,
        }],
        speakers: Default::default(),
        captions: None,
        fixes: Vec::new(),
        times: Vec::new(),
        windows: Vec::new(),
    }
}

fn cues() -> Vec<Cue> {
    vec![Cue {
        start: 0.0,
        end: 1.0,
        parts: vec![Part {
            speaker: 1,
            lines: vec!["hola".into()],
            words: Vec::new(),
        }],
    }]
}

fn sent(o: Outcome) -> super::super::Sent {
    match o {
        Outcome::Sent(s) => s,
        o => panic!("not sent: {o:?}"),
    }
}

#[test]
fn a_taken_name_asks_and_writes_or_changes_nothing() {
    let mut f = Fake::new(&["Main", "rec (Junctur)"]).built("x");
    let o = run(&mut f, &cues(), &edit(), &Captions::default(), None).unwrap();
    assert_eq!(
        o,
        Outcome::Exists {
            name: "rec (Junctur)".into()
        }
    );
    assert_eq!(f.kinds(), ["check"]);
    assert_eq!(f.written, 0);
}

#[test]
fn create_new_builds_beside_it_and_places_its_captions() {
    let mut f = Fake::new(&["rec (Junctur)"]).built("rec (Junctur) 2");
    let s = sent(
        run(
            &mut f,
            &cues(),
            &edit(),
            &Captions::default(),
            Some(Answer::New),
        )
        .unwrap(),
    );
    assert_eq!(f.kinds(), ["check", "build", "place"]);
    assert_eq!(f.field("build", "name"), Some("rec (Junctur) 2"));
    assert_eq!(f.field("build", "replace"), Some("false"));
    assert_eq!(f.field("build", "file"), Some(r"C:\J\timeline.fcpxml"));
    // The animated caption is asked for only with the highlight or an animation on.
    assert_eq!(f.field("build", "animated"), Some("false"));
    assert_eq!(f.field("place", "timeline"), Some("T-9"));
    assert_eq!(
        (s.placed, s.replaced, s.timeline.as_str()),
        (1, false, "rec (Junctur) 2")
    );
}

#[test]
fn a_built_timeline_never_asks_about_clips_in_the_way() {
    // Whatever the recording's choice, a timeline `build` just made gets a new track for anything
    // in the way: an ask there would be answered by a send that builds yet another timeline.
    for answer in [None, Some(Answer::New), Some(Answer::Replace)] {
        for existing in [
            ExistingClips::Ask,
            ExistingClips::Replace,
            ExistingClips::Skip,
        ] {
            let names: &[&str] = if answer.is_some() {
                &["rec (Junctur)"]
            } else {
                &[]
            };
            let mut f = Fake::new(names).built("rec (Junctur) 2");
            f.answers
                .insert("finish", Ok(vec![("replaced", "true".into())]));
            let c = Captions {
                existing_clips: existing,
                ..Default::default()
            };
            run(&mut f, &cues(), &edit(), &c, answer).unwrap();
            assert_eq!(
                f.field("place", "existing"),
                Some("newTrack"),
                "{answer:?} {existing:?}"
            );
        }
    }
}

#[test]
fn replace_deletes_the_old_only_after_the_captions_are_placed() {
    let mut f = Fake::new(&["rec (Junctur)"]).built("rec (Junctur) (new)");
    f.answers.insert(
        "finish",
        Ok(vec![
            ("replaced", "true".into()),
            ("name", "rec (Junctur)".into()),
            ("note", "".into()),
        ]),
    );
    let s = sent(
        run(
            &mut f,
            &cues(),
            &edit(),
            &Captions::default(),
            Some(Answer::Replace),
        )
        .unwrap(),
    );
    assert_eq!(f.kinds(), ["check", "build", "place", "finish"]);
    assert_eq!(f.field("build", "replace"), Some("true"));
    assert_eq!(f.field("finish", "name"), Some("rec (Junctur)"));
    assert_eq!(f.field("finish", "made"), Some("T-9"));
    assert_eq!(
        (s.replaced, s.timeline.as_str(), s.note),
        (true, "rec (Junctur)", None)
    );
}

#[test]
fn a_finish_resolve_refused_reaches_the_result() {
    let mut f = Fake::new(&["rec (Junctur)"]).built("rec (Junctur) (new)");
    let note = "Resolve did not delete the old \"rec (Junctur)\"";
    f.answers.insert(
        "finish",
        Ok(vec![
            ("replaced", "false".into()),
            ("name", "rec (Junctur) (new)".into()),
            ("note", note.into()),
        ]),
    );
    let s = sent(
        run(
            &mut f,
            &cues(),
            &edit(),
            &Captions::default(),
            Some(Answer::Replace),
        )
        .unwrap(),
    );
    assert_eq!(
        (s.replaced, s.timeline.as_str()),
        (false, "rec (Junctur) (new)")
    );
    assert_eq!(s.note.as_deref(), Some(note));
}

#[test]
fn captions_not_placed_say_what_was_made_and_never_finish() {
    let mut f = Fake::new(&["rec (Junctur)"]).built("rec (Junctur) (new)");
    f.answers
        .insert("place", Err("Resolve did not add a video track".into()));
    let e = run(
        &mut f,
        &cues(),
        &edit(),
        &Captions::default(),
        Some(Answer::Replace),
    )
    .unwrap_err();
    assert_eq!(f.kinds(), ["check", "build", "place"]);
    assert!(
        e.contains("\"rec (Junctur) (new)\" was made")
            && e.contains("\"rec (Junctur)\", which is untouched")
            && e.contains("video track"),
        "{e}"
    );
    let mut f = Fake::new(&[]).built("rec (Junctur)");
    f.answers.insert("place", Err("took too long".into()));
    let e = run(&mut f, &cues(), &edit(), &Captions::default(), None).unwrap_err();
    assert!(
        e.starts_with("Timeline \"rec (Junctur)\" was made in Resolve, but its captions"),
        "{e}"
    );
    // A finish that does not answer: both timelines are said to be there.
    let mut f = Fake::new(&["rec (Junctur)"]).built("rec (Junctur) (new)");
    let e = run(
        &mut f,
        &cues(),
        &edit(),
        &Captions::default(),
        Some(Answer::Replace),
    )
    .unwrap_err();
    assert!(e.contains("is still there"), "{e}");
}

#[test]
fn replace_keeps_the_old_timeline_when_no_caption_landed() {
    // Every caption refused: the old one is kept, the made one named, finish never asked.
    let mut f = Fake::new(&["rec (Junctur)"]).built("rec (Junctur) (new)");
    f.answers.insert(
        "place",
        Ok(vec![
            ("placed", "0".into()),
            ("failed", "1".into()),
            ("firstError", "the template has no Text+".into()),
        ]),
    );
    let e = run(
        &mut f,
        &cues(),
        &edit(),
        &Captions::default(),
        Some(Answer::Replace),
    )
    .unwrap_err();
    assert_eq!(f.kinds(), ["check", "build", "place"]);
    assert!(
        e.contains("\"rec (Junctur) (new)\" was made")
            && e.contains("\"rec (Junctur)\", which is untouched")
            && e.contains("nothing was replaced")
            && e.contains("no Text+"),
        "{e}"
    );
    // Some captions placed and some failed: Replace finishes.
    let mut f = Fake::new(&["rec (Junctur)"]).built("rec (Junctur) (new)");
    f.answers.insert(
        "place",
        Ok(vec![("placed", "1".into()), ("failed", "1".into())]),
    );
    f.answers.insert(
        "finish",
        Ok(vec![
            ("replaced", "true".into()),
            ("name", "rec (Junctur)".into()),
        ]),
    );
    let s = sent(
        run(
            &mut f,
            &cues(),
            &edit(),
            &Captions::default(),
            Some(Answer::Replace),
        )
        .unwrap(),
    );
    assert_eq!(f.kinds(), ["check", "build", "place", "finish"]);
    assert!(s.replaced);
}

#[test]
fn no_captions_build_the_timeline_alone() {
    let mut f = Fake::new(&[]).built("rec (Junctur)");
    let s = sent(run(&mut f, &[], &edit(), &Captions::default(), None).unwrap());
    assert_eq!(f.kinds(), ["check", "build"]);
    assert_eq!(f.field("build", "captions"), Some("false"));
    assert_eq!((s.placed, s.timeline.as_str()), (0, "rec (Junctur)"));
}

#[test]
fn an_older_script_and_a_double_name_change_nothing() {
    let mut f = Fake::default();
    let e = run(&mut f, &cues(), &edit(), &Captions::default(), None).unwrap_err();
    assert_eq!(e, OLD_SCRIPT);
    // A script that knows `check` but is older than this Junctur, or says no version: refused
    // before anything is written or made.
    for old in [None, Some("5n")] {
        let mut f = Fake::new(&[]).built("rec (Junctur)");
        if let Some(Ok(check)) = f.answers.get_mut("check") {
            check.retain(|(k, _)| *k != "script");
            if let Some(v) = old {
                check.push(("script", v.to_string()));
            }
        }
        let e = run(&mut f, &cues(), &edit(), &Captions::default(), None).unwrap_err();
        assert_eq!(e, OLD_SCRIPT);
        assert_eq!((f.kinds(), f.written), (vec!["check"], 0));
    }
    let mut f = Fake::new(&["rec (Junctur)", "REC (Junctur)"]).built("x");
    let e = run(
        &mut f,
        &cues(),
        &edit(),
        &Captions::default(),
        Some(Answer::Replace),
    )
    .unwrap_err();
    assert!(e.contains("2 timelines"), "{e}");
    assert_eq!((f.kinds(), f.written), (vec!["check"], 0));
}

fn update(f: &mut Fake, cues: &[Cue]) -> Result<Outcome, String> {
    run(f, cues, &edit(), &Captions::default(), Some(Answer::Update))
}

#[test]
fn an_update_meeting_clips_in_the_way_asks_and_the_answer_is_an_update_again() {
    // On an update the timeline there has the user's clips on Junctur's tracks, so `place`
    // (asking) places nothing and counts them; nothing is built or finished.
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "120");
    f.answers.insert(
        "place",
        Ok(vec![("inTheWay", "2".into()), ("blocked", "3".into())]),
    );
    let s = sent(update(&mut f, &cues()).unwrap());
    assert_eq!(f.kinds(), ["check", "open", "place"]);
    assert_eq!(f.field("place", "existing"), Some("ask"));
    assert_eq!(f.field("open", "animated"), Some("false"));
    assert_eq!(
        (s.in_the_way, s.blocked, s.updated, f.written),
        (2, 3, true, 0)
    );
    // The user's answer goes with the same Update: on the same timeline, never a new one.
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "120");
    let c = Captions {
        existing_clips: crate::edit::ExistingClips::Replace,
        ..Default::default()
    };
    let s = sent(run(&mut f, &cues(), &edit(), &c, Some(Answer::Update)).unwrap());
    assert_eq!(f.kinds(), ["check", "open", "place"]);
    assert_eq!(f.field("place", "existing"), Some("replace"));
    assert!(s.updated && s.placed == 1);
}

#[test]
fn update_places_on_the_timeline_there_and_builds_nothing() {
    // The edit keeps 5 s: 120 frames at 24, the cut the timeline has.
    let mut f = Fake::new(&["Main", "REC (Junctur)"]).opened("REC (Junctur)", "120");
    let s = sent(update(&mut f, &cues()).unwrap());
    assert_eq!(f.kinds(), ["check", "open", "place"]);
    assert_eq!(f.written, 0);
    assert_eq!(f.field("open", "name"), Some("rec (Junctur)"));
    assert_eq!(f.field("place", "timeline"), Some("T-9"));
    assert_eq!(
        (s.placed, s.updated, s.replaced, s.timeline.as_str()),
        (1, true, false, "REC (Junctur)")
    );
    assert_eq!(s.note, None);
    // No clips on it to measure: nothing said about the cut.
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "");
    assert_eq!(sent(update(&mut f, &cues()).unwrap()).note, None);
}

#[test]
fn update_says_when_the_timelines_cut_is_not_the_edits() {
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "96");
    let s = sent(update(&mut f, &cues()).unwrap());
    let note = s.note.unwrap();
    assert!(
        note.contains("5.00 s") && note.contains("4.00 s") && note.contains("follow Junctur's cut"),
        "{note}"
    );
    assert_eq!((s.placed, f.kinds().len()), (1, 3));
}

#[test]
fn update_failures_say_what_is_in_resolve() {
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "120");
    f.answers.insert("place", Err("took too long".into()));
    let e = update(&mut f, &cues()).unwrap_err();
    assert!(
        e.contains("\"rec (Junctur)\" in Resolve may be only partly updated")
            && e.contains("its cuts, its media and your own clips are as they were")
            && e.contains("took too long")
            && e.contains("send again to finish"),
        "{e}"
    );
    // Another timeline opened between open and place: the script refused, nothing placed.
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "120");
    f.answers.insert(
        "place",
        Err("The open timeline in Resolve changed while sending: send again".into()),
    );
    let e = update(&mut f, &cues()).unwrap_err();
    assert!(
        e.contains("changed after Junctur opened \"rec (Junctur)\"")
            && e.contains("no captions were placed or changed")
            && !e.contains("partly"),
        "{e}"
    );
    // An answer to open that cannot be read: it may be open now, nothing else changed.
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "120");
    f.answers.insert(
        "open",
        Ok(vec![
            ("name", "rec (Junctur)".into()),
            ("fps", "fast".into()),
        ]),
    );
    let e = update(&mut f, &cues()).unwrap_err();
    assert!(
        e.contains("may now have it open") && e.contains("nothing else was changed"),
        "{e}"
    );
    assert_eq!(f.kinds(), ["check", "open"]);
    // Gone since the pop-up, or doubled: refused before anything but check.
    let mut f = Fake::new(&["Main"]).opened("x", "0");
    let e = update(&mut f, &cues()).unwrap_err();
    assert!(e.contains("no longer has"), "{e}");
    let mut f = Fake::new(&["rec (Junctur)", "rec (junctur)"]).opened("x", "0");
    let e = update(&mut f, &cues()).unwrap_err();
    assert!(e.contains("which to update"), "{e}");
    assert_eq!((f.kinds(), f.written), (vec!["check"], 0));
    // An older script, which knows check but not open.
    let mut f = Fake::new(&["rec (Junctur)"]);
    assert_eq!(update(&mut f, &cues()).unwrap_err(), OLD_SCRIPT);
    assert_eq!(f.kinds(), ["check", "open"]);
}

#[test]
fn update_with_no_captions_asks_resolve_nothing() {
    let mut f = Fake::new(&["rec (Junctur)"]).opened("rec (Junctur)", "120");
    assert_eq!(update(&mut f, &[]).unwrap_err(), NO_CAPTIONS);
    assert!(f.kinds().is_empty());
}
