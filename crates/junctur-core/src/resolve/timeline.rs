//! The timeline a send makes in Resolve: its name, and what to do when the open project already
//! has one of that name (ask, make a new one, replace it, or update only its captions). Decided
//! here from the names the script lists, so it is tested without Resolve.

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
    /// Leave it and its cut as they are and refresh only Junctur's captions on it.
    Update,
}

/// What the send does with the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The name is taken and the user has not answered: ask, change nothing.
    Ask,
    /// Make the timeline `name`; `replace` the one of that name already there.
    Build { name: String, replace: bool },
    /// Refresh the captions on the one timeline called `name`.
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
/// frame off the edit's length (Resolve's end frame may count the last frame or not). Only the two
/// lengths are known, not why they differ, so the note says just that; not a refusal, since what
/// was added in Resolve is the user's. No clips on it (`None`) says nothing.
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
mod tests;
