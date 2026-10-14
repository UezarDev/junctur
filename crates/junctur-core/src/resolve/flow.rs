//! The order of a send's requests (5z), apart from the mailbox so it is tested with a stand-in:
//! `check` (changes nothing), then, unless the user has to be asked, `build` (the new timeline),
//! `place` (its captions) and, for Replace, `finish` (the old timeline deleted, the new one given
//! its name). The old timeline goes last, once the new one has its captions, so a send that fails
//! part way never costs it; and once a timeline has been made, every error says what is in Resolve.

use std::collections::HashMap;
use std::path::PathBuf;

use super::lua::Value;
use super::{job, reply, timeline, Answer, Outcome};
use crate::edit::{Captions, EditList};
use crate::export::layout;
use crate::export::srt::Cue;

/// The way to Resolve: the mailbox in a real send.
pub trait Link {
    /// Sends request `kind` and waits for its answer; the script's own error is an `Err`.
    fn request(
        &mut self,
        kind: &str,
        fields: Vec<(&'static str, Value)>,
    ) -> Result<HashMap<String, String>, String>;
    /// Writes the edit as the FCPXML the script imports, and says where.
    fn write_timeline(&mut self, edit: &EditList) -> Result<PathBuf, String>;
}

/// A 5g script still running in Resolve after Junctur installed a newer one answers `check` so.
pub const OLD_SCRIPT: &str = "The Junctur script running in Resolve is from an older Junctur: in \
     Resolve choose Workspace > Scripts > Junctur again, then send again";

/// `check` is the first request a 5z script knows: an older one calls it unknown.
pub fn older(e: String) -> String {
    if e.starts_with("unknown request") {
        OLD_SCRIPT.into()
    } else {
        e
    }
}

/// The send after the script is installed: see the module's comment.
pub fn run(
    link: &mut impl Link,
    cues: &[Cue],
    edit: &EditList,
    c: &Captions,
    answer: Option<Answer>,
) -> Result<Outcome, String> {
    let names = link.request("check", vec![]).map_err(older)?;
    let wanted = timeline::name(edit);
    let (name, replace) = match timeline::decide(&reply::timeline_names(&names), &wanted, answer)? {
        timeline::Decision::Ask => return Ok(Outcome::Exists { name: wanted }),
        timeline::Decision::Build { name, replace } => (name, replace),
    };
    let file = link.write_timeline(edit)?;
    let built = link.request(
        "build",
        vec![
            ("file", file.to_string_lossy().into_owned().into()),
            ("name", name.as_str().into()),
            ("replace", replace.into()),
            ("source", edit.source.to_string_lossy().into_owned().into()),
            ("captions", (!cues.is_empty()).into()),
            ("template", job::template(c).into()),
        ],
    )?;
    let timeline = reply::timeline(&built)?;
    let made = timeline.name.as_str();
    let mut sent = if cues.is_empty() {
        reply::sent(&HashMap::new(), made)
    } else {
        let aspect = timeline.height as f64 / timeline.width.max(1) as f64;
        let placed = layout::place(cues, edit, c, aspect);
        let job = job::build(&placed, edit, c, &timeline);
        let done = link.request("place", job.to_lua()).map_err(|e| {
            if replace {
                format!(
                    "Timeline \"{made}\" was made in Resolve beside \"{name}\", which is untouched, \
                     but its captions were not placed, so nothing was replaced: {e}"
                )
            } else {
                format!("Timeline \"{made}\" was made in Resolve, but its captions were not placed: {e}")
            }
        })?;
        reply::sent(&done, made)
    };
    if replace {
        let done = link
            .request(
                "finish",
                vec![
                    ("name", name.as_str().into()),
                    ("made", timeline.id.as_str().into()),
                ],
            )
            .map_err(|e| {
                format!(
                    "Timeline \"{made}\" was made in Resolve with its captions, but \"{name}\" was \
                     not replaced and is still there: {e}"
                )
            })?;
        sent.replaced = done.get("replaced").is_some_and(|v| v == "true");
        if let Some(n) = done.get("name").filter(|n| !n.is_empty()) {
            sent.timeline = n.clone();
        }
        sent.note = done.get("note").filter(|n| !n.is_empty()).cloned();
    }
    Ok(Outcome::Sent(sent))
}

#[cfg(test)]
mod tests {
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
            let mut check = vec![("timelines", timelines.len().to_string())];
            let keys = ["timeline1", "timeline2", "timeline3"];
            for (k, t) in timelines.iter().enumerate() {
                check.push((keys[k], t.to_string()));
            }
            let mut f = Fake::default();
            f.answers.insert("check", Ok(check));
            f.answers.insert("place", Ok(vec![("placed", "1".into())]));
            f
        }
        fn built(mut self, name: &str) -> Self {
            let a = [
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
            self.answers.insert(
                "build",
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
        assert_eq!(f.field("place", "timeline"), Some("T-9"));
        assert_eq!(
            (s.placed, s.replaced, s.timeline.as_str()),
            (1, false, "rec (Junctur) 2")
        );
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
}
