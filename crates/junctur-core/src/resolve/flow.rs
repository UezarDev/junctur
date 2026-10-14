//! The order of a send's requests, apart from the mailbox so it is tested with a stand-in: `check`,
//! then unless the user has to be asked `build`, `place` and, for Replace, `finish` (old timeline
//! deleted, new one named). The old timeline goes last, so a send that fails part way never costs
//! it. "Update captions only" is `check`, `open` (makes the timeline current) and `place`, which
//! may ask about the user's clips on Junctur's tracks; the answer is sent again as an update.

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

/// What a send says when the script running in Resolve is older than this Junctur: an old script
/// answers `check` as unknown, and one older than [`reply::SCRIPT`] would ignore fields `place`
/// sends.
pub const OLD_SCRIPT: &str = "The Junctur script running in Resolve is from an older Junctur: in \
     Resolve choose Workspace > Scripts > Junctur again, then send again";

/// An older script calls `check` or `open` an unknown request.
pub fn older(e: String) -> String {
    if e.starts_with("unknown request") {
        OLD_SCRIPT.into()
    } else {
        e
    }
}

/// An update with nothing to place would delete every caption of the recording on the timeline.
pub const NO_CAPTIONS: &str = "This edit has no captions to send (no transcript, or no words \
     kept), so there are none to update: nothing was changed in Resolve";

/// The send after the script is installed: see the module's comment.
pub fn run(
    link: &mut impl Link,
    cues: &[Cue],
    edit: &EditList,
    c: &Captions,
    answer: Option<Answer>,
) -> Result<Outcome, String> {
    if answer == Some(Answer::Update) && cues.is_empty() {
        return Err(NO_CAPTIONS.into());
    }
    let names = link.request("check", vec![]).map_err(older)?;
    // Before anything is written or made: the script's version, as `check` reports it.
    if !reply::script_is_current(&names) {
        return Err(OLD_SCRIPT.into());
    }
    let wanted = timeline::name(edit);
    let (name, replace) = match timeline::decide(&reply::timeline_names(&names), &wanted, answer)? {
        timeline::Decision::Ask => return Ok(Outcome::Exists { name: wanted }),
        timeline::Decision::Update { name } => return update(link, cues, edit, c, &name),
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
            // The animated caption when the highlight or an animation is on.
            ("animated", c.animated().into()),
        ],
    )?;
    let timeline = reply::timeline(&built)?;
    let made = timeline.name.as_str();
    let mut sent = if cues.is_empty() {
        reply::sent(&HashMap::new(), made)
    } else {
        let done = place(link, cues, edit, c, &timeline).map_err(|e| {
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
    if replace && !cues.is_empty() {
        // Asking first placed nothing and waits for the user's answer: the old timeline stays
        // until a send has its captions on the new one.
        if sent.in_the_way > 0 {
            sent.note = Some(format!(
                "Timeline \"{made}\" was made in Resolve beside \"{name}\", which is untouched: \
                 nothing was replaced"
            ));
            return Ok(Outcome::Sent(sent));
        }
        // Every caption refused (a template with no Text+, every append refused): replacing would
        // swap the old timeline for one with no captions at all.
        if sent.placed + sent.restyled == 0 {
            let why = sent
                .first_error
                .as_deref()
                .unwrap_or("Resolve placed none of them");
            return Err(format!(
                "Timeline \"{made}\" was made in Resolve beside \"{name}\", which is untouched, \
                 but none of its captions were placed, so nothing was replaced: {why}"
            ));
        }
    }
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

/// Lays `cues` out for the timeline's shape and has the script place them on it.
fn place(
    link: &mut impl Link,
    cues: &[Cue],
    edit: &EditList,
    c: &Captions,
    timeline: &reply::Timeline,
) -> Result<HashMap<String, String>, String> {
    let aspect = timeline.height as f64 / timeline.width.max(1) as f64;
    let placed = layout::place(cues, edit, c, aspect);
    let job = job::build(&placed, edit, c, timeline);
    link.request("place", job.to_lua())
}

/// "Update captions only" on the one timeline called `name`: made current by `open`, which
/// changes nothing on it, then `place`, which touches only the clips tagged for this recording
/// and the `Junctur …` tracks. Its cut is never rebuilt; when it no longer matches the edit's, the
/// result says so (the captions follow Junctur's cut).
fn update(
    link: &mut impl Link,
    cues: &[Cue],
    edit: &EditList,
    c: &Captions,
    name: &str,
) -> Result<Outcome, String> {
    let opened = link
        .request(
            "open",
            vec![
                ("name", name.into()),
                ("template", job::template(c).into()),
                // The animated caption, looked up as `build` looks it up.
                ("animated", c.animated().into()),
            ],
        )
        .map_err(older)?;
    // `open` may have switched Resolve to the timeline before its answer went wrong.
    let timeline = reply::timeline(&opened).map_err(|e| {
        format!(
            "Resolve's answer about timeline \"{name}\" could not be read ({e}): Resolve may now \
             have it open; nothing else was changed"
        )
    })?;
    let made = timeline.name.as_str();
    let done = place(link, cues, edit, c, &timeline).map_err(|e| placing_failed(made, e))?;
    let mut sent = reply::sent(&done, made);
    sent.updated = true;
    let cut_end = opened
        .get("cutEnd")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|v| v.round() as i64);
    sent.note = timeline::cut_note(edit, &timeline, cut_end);
    Ok(Outcome::Sent(sent))
}

/// How the script's `place` starts its refusal when another timeline is open than the one it was
/// sent for (`junctur.lua`); it refuses before touching anything.
const SWITCHED: &str = "The open timeline in Resolve changed";

/// An update's `place` that failed, said as far as it is known: a timeline switched meanwhile
/// was left alone; anything else (a timeout above all) may have stopped part way through the
/// captions. Either way nothing but Junctur's captions was touched.
fn placing_failed(made: &str, e: String) -> String {
    if e.starts_with(SWITCHED) {
        format!(
            "The open timeline in Resolve changed after Junctur opened \"{made}\", so no captions \
             were placed or changed: open \"{made}\" and send again"
        )
    } else {
        format!(
            "The captions on timeline \"{made}\" in Resolve may be only partly updated: {e}. An \
             update touches only Junctur's captions, so its cuts, its media and your own clips \
             are as they were; send again to finish it"
        )
    }
}

#[cfg(test)]
mod tests;
