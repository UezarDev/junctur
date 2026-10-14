//! The timeline a send makes in Resolve (5z): its name, and what to do when the open project already
//! has a timeline of that name. Decided here, from the names the script lists, so the question the
//! app asks and the name Create new picks are tested without Resolve.

use serde::Deserialize;

use crate::edit::EditList;

/// The user's answer when the timeline already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Answer {
    /// Make another beside it, numbered.
    New,
    /// Delete it and make it again from this edit.
    Replace,
}

/// What the send does with the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The name is taken and the user has not answered: ask, change nothing.
    Ask,
    /// Make the timeline `name`; `replace` the one of that name already there.
    Build { name: String, replace: bool },
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
    })
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
        assert!(serde_json::from_str::<Answer>("\"overwrite\"").is_err());
    }
}
