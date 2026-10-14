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
pub mod flow;
pub mod install;
pub mod job;
pub mod lua;
pub mod reply;
pub mod timeline;

pub use install::{install, Installed};
pub use reply::{Sent, Timeline};
pub use timeline::Answer;

use crate::edit::{Captions, EditList};
use crate::export;
use crate::export::srt::Cue;
use std::collections::HashMap;
use std::path::PathBuf;

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

/// Sends the whole edit (5z): installs (or refreshes) the script, then [`flow::run`] through the
/// mailbox: asks Resolve for the open project's timelines and, unless `<recording> (Junctur)` is
/// there and `answer` does not say what to do with it, has the script import the edit as a
/// timeline (Junctur's FCPXML: the recording, every audio track, cut as the edit list says), place
/// `cues` on it as Text+ clips laid out for its shape, and for Replace only then delete the old one.
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
    flow::run(&mut Mailbox, cues, edit, c, answer)
}

/// The real [`flow::Link`]: the request file and `Fusion.prefs` ([`bridge`]), the FCPXML beside them.
struct Mailbox;

impl flow::Link for Mailbox {
    fn request(
        &mut self,
        kind: &str,
        fields: Vec<(&'static str, lua::Value)>,
    ) -> Result<HashMap<String, String>, String> {
        bridge::request(kind, fields)
    }

    fn write_timeline(&mut self, edit: &EditList) -> Result<PathBuf, String> {
        let file = bridge::mailbox()
            .ok_or("no folder for Junctur's settings")?
            .join("timeline.fcpxml");
        export::write_fcpxml(edit, &file).map_err(|e| format!("writing the timeline: {e}"))?;
        Ok(file)
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
}
