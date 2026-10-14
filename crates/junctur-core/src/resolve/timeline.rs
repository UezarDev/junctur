//! The timeline a send makes in Resolve (5z): its name, and what to do when the open project already
//! has a timeline of that name. Decided here, from the names the script lists, so the question the
//! app asks and the name Create new picks are tested without Resolve. Since 5ab the answer can also
//! be to update only the captions on the timeline that is there, and [`cut_note`] says when that
//! timeline's cut no longer matches the edit's.

use serde::Deserialize;

use super::reply::Timeline;
use crate::edit::{frame_ranges, frames_ceil, EditList};

/// The user's answer when the timeline already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Answer {
    /// Make another beside it, numbered.
    New,
    /// Delete it and make it again from this edit.
    Replace,
    /// Leave it and its cut as they are and refresh only Junctur's captions on it (5ab).
    Update,
}

/// What the send does with the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The name is taken and the user has not answered: ask, change nothing.
    Ask,
    /// Make the timeline `name`; `replace` the one of that name already there.
    Build { name: String, replace: bool },
    /// Refresh the captions on the one timeline called `name` (5ab).
    Update { name: String },
}

/// `<file stem> (Junctur)`: the FCPXML export's own project name, so a send and a by-hand import
/// of the FCPXML look alike in Resolve.
pub fn name(edit: &EditList) -> String {
    let stem = edit
        .source
        .file_stem()
        .map_or_else(|| "source".into(), |s| s.to_string_lossy().into_owned());
    format!("{stem} (Junctur)")
}

/// A timeline name as both sides compare it (the script's `norm_name` does the same): ASCII spaces
/// round it dropped and ASCII letters' case ignored, every other character exact. Two names
/// differing only so are too alike to tell apart in Resolve's media pool.
fn norm(name: &str) -> String {
    name.trim_matches([' ', '\t', '\n', '\r', '\x0C'])
        .to_ascii_lowercase()
}

/// How many of `names` are `name`.
fn taken(names: &[String], name: &str) -> usize {
    let name = norm(name);
    names.iter().filter(|n| norm(n) == name).count()
}

/// The first of `name 2`, `name 3`, … that no timeline has.
pub fn free(names: &[String], name: &str) -> String {
    (2..)
        .map(|k| format!("{name} {k}"))
        .find(|n| taken(names, n) == 0)
        .unwrap_or_else(|| name.to_string())
}

/// What to do with `name` given the project's timeline `names` and the user's answer, if any.
/// Replace is refused when several timelines have the name: which one the user meant is not
/// Junctur's to guess.
pub fn decide(names: &[String], name: &str, answer: Option<Answer>) -> Result<Decision, String> {
    let n = taken(names, name);
    // Gone since the pop-up: building one would send the cut too, which Update did not choose.
    if n == 0 && answer == Some(Answer::Update) {
        return Err(format!(
            "Resolve no longer has a timeline called \"{name}\", so there are no captions to \
             update: nothing was changed. Send again to make it"
        ));
    }
    if n == 0 {
        return Ok(Decision::Build {
            name: name.to_string(),
            replace: false,
        });
    }
    Ok(match answer {
        None => Decision::Ask,
        Some(Answer::New) => Decision::Build {
            name: free(names, name),
            replace: false,
        },
        Some(Answer::Replace) if n > 1 => {
            return Err(format!(
                "Resolve has {n} timelines called \"{name}\", so Junctur cannot tell which to \
                 replace: nothing was changed. Rename or delete all but one, or choose Create new \
                 timeline"
            ))
        }
        Some(Answer::Replace) => Decision::Build {
            name: name.to_string(),
            replace: true,
        },
        Some(Answer::Update) if n > 1 => {
            return Err(format!(
                "Resolve has {n} timelines called \"{name}\", so Junctur cannot tell which to \
                 update: nothing was changed. Rename or delete all but one, or choose Create new \
                 timeline"
            ))
        }
        Some(Answer::Update) => Decision::Update {
            name: name.to_string(),
        },
    })
}

/// The edit's cut in frames of `fps`, as the FCPXML lays its kept ranges end to end: the
/// imported timeline runs at the source's rate, which `fps` (Resolve's, made exact for NTSC by
/// [`super::reply::rate`]) is taken back to as a fraction.
fn cut_frames(edit: &EditList, fps: f64) -> i64 {
    let (num, den) = if (fps - fps.round()).abs() < 1e-6 {
        (fps.round() as u64, 1)
    } else if ((fps * 1001.0) - (fps * 1001.0).round()).abs() < 1e-3 {
        ((fps * 1001.0).round() as u64, 1001)
    } else {
        ((fps * 1000.0).round() as u64, 1000)
    };
    let total = frames_ceil(edit.duration, num, den);
    frame_ranges(&edit.keep, num, den, total)
        .iter()
        .map(|(a, b)| (b - a) as i64)
        .sum()
}

/// What an update says when the timeline's clips (captions aside) end at `cut_end`, more than a
/// frame off the edit's length (Resolve's end frame may count the last frame or not) (5ab). Only
/// the two lengths are known, not why they differ (a cut changed on either side, or a music bed
/// or an outro added past it), so the note says just that. Not a refusal: what was added in
/// Resolve is the user's. No clips on it (`None`) says nothing: there is nothing to compare.
pub fn cut_note(edit: &EditList, t: &Timeline, cut_end: Option<i64>) -> Option<String> {
    let theirs = cut_end? - t.start;
    let ours = cut_frames(edit, t.fps);
    if (theirs - ours).abs() <= 1 {
        return None;
    }
    let secs = |f: i64| f as f64 / t.fps;
    Some(format!(
        "Timeline \"{}\"'s clips end {:.2} s in and Junctur's cut is {:.2} s long. The \
         captions follow Junctur's cut, so they may not line up with this timeline's clips; \
         Create new timeline or Replace also sends the cut",
        t.name,
        secs(theirs),
        secs(ours)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn named_after_the_recording_as_the_fcpxml_names_it() {
        let edit = EditList {
            source: r"C:\Videos\2026-06-15 00-09-04.mp4".into(),
            duration: 1.0,
            settings: Default::default(),
            keep: Vec::new(),
            speakers: Default::default(),
            captions: None,
            fixes: Vec::new(),
            times: Vec::new(),
            windows: Vec::new(),
        };
        assert_eq!(name(&edit), "2026-06-15 00-09-04 (Junctur)");
    }

    #[test]
    fn a_free_name_is_built_without_asking() {
        let d = decide(&names(&["Timeline 1", "rec"]), "rec (Junctur)", None).unwrap();
        assert_eq!(
            d,
            Decision::Build {
                name: "rec (Junctur)".into(),
                replace: false
            }
        );
        // An answer given for a name that is free by now changes nothing.
        let d = decide(&[], "rec (Junctur)", Some(Answer::Replace)).unwrap();
        assert!(matches!(d, Decision::Build { replace: false, .. }));
    }

    #[test]
    fn a_taken_name_asks_until_answered() {
        let n = names(&["REC (Junctur)", "rec (Junctur) 2", "rec (Junctur) 4"]);
        assert_eq!(decide(&n, "rec (Junctur)", None).unwrap(), Decision::Ask);
        assert_eq!(
            decide(&n, "rec (Junctur)", Some(Answer::New)).unwrap(),
            Decision::Build {
                name: "rec (Junctur) 3".into(),
                replace: false
            }
        );
        assert_eq!(
            decide(&n, "rec (Junctur)", Some(Answer::Replace)).unwrap(),
            Decision::Build {
                name: "rec (Junctur)".into(),
                replace: true
            }
        );
    }

    #[test]
    fn replace_is_refused_when_several_have_the_name_and_names_compare_as_the_script_does() {
        let n = names(&["rec (Junctur)", " REC (JUNCTUR)\t"]);
        let e = decide(&n, "rec (Junctur)", Some(Answer::Replace)).unwrap_err();
        assert!(
            e.contains("2 timelines") && e.contains("nothing was changed"),
            "{e}"
        );
        // Create new still works, and asking still asks.
        assert_eq!(
            decide(&n, "rec (Junctur)", Some(Answer::New)).unwrap(),
            Decision::Build {
                name: "rec (Junctur) 2".into(),
                replace: false
            }
        );
        assert_eq!(decide(&n, "rec (Junctur)", None).unwrap(), Decision::Ask);
        // Only ASCII case is folded, as Lua's byte-wise compare does: "É" and "é" differ.
        assert_eq!(
            decide(&names(&["Égal (Junctur)"]), "égal (Junctur)", None).unwrap(),
            Decision::Build {
                name: "égal (Junctur)".into(),
                replace: false
            }
        );
    }

    #[test]
    fn answers_read_as_the_app_sends_them() {
        let a: Answer = serde_json::from_str("\"new\"").unwrap();
        assert_eq!(a, Answer::New);
        let a: Answer = serde_json::from_str("\"replace\"").unwrap();
        assert_eq!(a, Answer::Replace);
        let a: Answer = serde_json::from_str("\"update\"").unwrap();
        assert_eq!(a, Answer::Update);
        assert!(serde_json::from_str::<Answer>("\"overwrite\"").is_err());
    }

    #[test]
    fn update_needs_exactly_one_timeline_of_the_name() {
        let n = names(&["Main", " REC (Junctur)"]);
        assert_eq!(
            decide(&n, "rec (Junctur)", Some(Answer::Update)).unwrap(),
            Decision::Update {
                name: "rec (Junctur)".into()
            }
        );
        // Gone since the pop-up: refused, never built.
        let e = decide(&names(&["Main"]), "rec (Junctur)", Some(Answer::Update)).unwrap_err();
        assert!(
            e.contains("no longer has") && e.contains("nothing was changed"),
            "{e}"
        );
        let n = names(&["rec (Junctur)", "REC (JUNCTUR)"]);
        let e = decide(&n, "rec (Junctur)", Some(Answer::Update)).unwrap_err();
        assert!(e.contains("2 timelines") && e.contains("update"), "{e}");
    }

    fn timeline(fps: f64, start: i64) -> Timeline {
        Timeline {
            name: "rec (Junctur)".into(),
            id: "T-1".into(),
            fps,
            start,
            end: start + 1,
            width: 1920,
            height: 1080,
            template: String::new(),
            template_fps: fps,
        }
    }

    #[test]
    fn a_cut_that_changed_is_said_and_the_same_cut_is_not() {
        let mut edit = EditList {
            source: "rec.mp4".into(),
            duration: 60.0,
            settings: Default::default(),
            keep: vec![
                crate::edit::Range {
                    start: 0.0,
                    end: 10.0,
                },
                crate::edit::Range {
                    start: 20.0,
                    end: 25.0,
                },
            ],
            speakers: Default::default(),
            captions: None,
            fixes: Vec::new(),
            windows: Vec::new(),
        };
        // 15 s at 24 fps is 360 frames; Resolve's end one frame either way is the same cut.
        let t = timeline(24.0, 86400);
        assert_eq!(cut_note(&edit, &t, Some(86400 + 360)), None);
        assert_eq!(cut_note(&edit, &t, Some(86400 + 361)), None);
        assert_eq!(cut_note(&edit, &t, None), None);
        let note = cut_note(&edit, &t, Some(86400 + 240)).unwrap();
        assert!(
            note.contains("\"rec (Junctur)\"'s clips end 10.00 s in")
                && note.contains("Junctur's cut is 15.00 s long")
                && note.contains("Replace")
                && !note.contains("changed"),
            "{note}"
        );
        // NTSC: the timeline's 29.97 is taken back to 30000/1001, the FCPXML's grid. 1 s to 2 s
        // is frames 29 (29.97 floored) to 60 (59.94 ceiled) there: 31, where 30 fps gives 30.
        edit.keep = vec![crate::edit::Range {
            start: 1.0,
            end: 2.0,
        }];
        let t = timeline(30000.0 / 1001.0, 0);
        assert_eq!(cut_frames(&edit, t.fps), 31);
        assert_eq!(cut_frames(&edit, 30.0), 30);
        assert_eq!(cut_note(&edit, &t, Some(31)), None);
        assert!(cut_note(&edit, &t, Some(36)).is_some());
    }
}
