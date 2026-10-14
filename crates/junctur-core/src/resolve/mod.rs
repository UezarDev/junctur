//! Captions sent into DaVinci Resolve as Fusion Text+ clips (5g), the way auto-subs does it
//! (tmoroney/auto-subs, MIT, `.planning/research/auto-subs-findings.md`): a script started once in
//! Resolve (Workspace > Scripts > Junctur) stays running and places what Junctur asks, one Text+
//! per speaker per caption on that speaker's own video track, styled by the caption settings, and
//! tagged so sending again restyles them in place. Unlike an SRT this reaches font, size, position
//! and outline, and shows people talking at once where the layout puts them. The SRT export stays.
//!
//! Since 5z a send hands Resolve the whole edit: the script imports Junctur's own FCPXML as a new
//! timeline (the recording, every audio track, cut as the edit list says, [`timeline`] names it and
//! decides what to do when the project already has it) and the captions go on that timeline.
//!
//! A module of its own rather than in `export`: it talks to a running program, and everything it
//! computes (frames, tracks, Text+ inputs) is in [`job`] so the script inside Resolve stays a thin
//! loop that cannot be tested here.

pub mod bridge;
pub mod install;
pub mod job;
pub mod lua;
pub mod reply;
pub mod timeline;

pub use install::{install, Installed};
pub use reply::{Sent, Timeline};
pub use timeline::Answer;

use crate::edit::{Captions, EditList};
use crate::export::srt::Cue;
use crate::export::{self, layout};

/// What a send came to (5z).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Outcome {
    /// The open project already has a timeline called `name`: nothing was changed, the app asks
    /// whether to make a new one or replace it, and sends again with the answer.
    Exists {
        name: String,
    },
    Sent(Sent),
}

/// A 5g script still running in Resolve after Junctur installed a newer one answers `check` so.
const OLD_SCRIPT: &str = "The Junctur script running in Resolve is from an older Junctur: in \
     Resolve choose Workspace > Scripts > Junctur again, then send again";

/// Sends the whole edit (5z): installs (or refreshes) the script, asks Resolve for the open
/// project's timelines, and unless `<recording> (Junctur)` is there and `answer` does not say what
/// to do with it, has the script import the edit as that timeline (Junctur's FCPXML: the recording,
/// every audio track, cut as the edit list says) and place `cues` on it as Text+ clips, laid out
/// for its shape (side-by-side lines are wrapped to its width).
pub fn send(
    cues: &[Cue],
    edit: &EditList,
    c: &Captions,
    answer: Option<Answer>,
) -> Result<Outcome, String> {
    if edit.keep.is_empty() {
        return Err("nothing is kept in this edit, so there is no timeline to send".into());
    }
    install()?;
    let names = bridge::request("check", vec![]).map_err(older)?;
    let wanted = timeline::name(edit);
    let (name, replace) = match timeline::decide(&reply::timeline_names(&names), &wanted, answer) {
        timeline::Decision::Ask => return Ok(Outcome::Exists { name: wanted }),
        timeline::Decision::Build { name, replace } => (name, replace),
    };
    let file = bridge::mailbox()
        .ok_or("no folder for Junctur's settings")?
        .join("timeline.fcpxml");
    export::write_fcpxml(edit, &file).map_err(|e| format!("writing the timeline: {e}"))?;
    let built = bridge::request(
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
    let mut sent = if cues.is_empty() {
        reply::sent(&Default::default(), &timeline.name)
    } else {
        let aspect = timeline.height as f64 / timeline.width.max(1) as f64;
        let placed = layout::place(cues, edit, c, aspect);
        let job = job::build(&placed, edit, c, &timeline);
        let done = bridge::request("place", job.to_lua())?;
        reply::sent(&done, &timeline.name)
    };
    sent.replaced = built.get("replaced").is_some_and(|v| v == "true");
    sent.note = built.get("note").filter(|n| !n.is_empty()).cloned();
    Ok(Outcome::Sent(sent))
}

/// `check` is the first request a 5z script knows: an older one calls it unknown.
fn older(e: String) -> String {
    if e.starts_with("unknown request") {
        OLD_SCRIPT.into()
    } else {
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outcome_reads_as_the_page_expects() {
        let asked = serde_json::to_value(Outcome::Exists {
            name: "rec (Junctur)".into(),
        })
        .unwrap();
        assert_eq!(
            asked,
            serde_json::json!({ "kind": "exists", "name": "rec (Junctur)" })
        );
        let sent = serde_json::to_value(Outcome::Sent(Sent {
            placed: 2,
            timeline: "rec (Junctur) 2".into(),
            replaced: true,
            ..Default::default()
        }))
        .unwrap();
        assert_eq!(sent["kind"], "sent");
        assert_eq!(sent["placed"], 2);
        assert_eq!(sent["replaced"], true);
        assert_eq!(sent["note"], serde_json::Value::Null);
        assert_eq!(sent["firstError"], serde_json::Value::Null);
    }

    #[test]
    fn an_older_script_is_told_to_start_again() {
        assert_eq!(older("unknown request check".into()), OLD_SCRIPT);
        assert_eq!(older("No project is open".into()), "No project is open");
    }
}
