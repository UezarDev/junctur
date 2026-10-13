//! Captions sent into DaVinci Resolve as Fusion Text+ clips (5g), the way auto-subs does it
//! (tmoroney/auto-subs, MIT, `.planning/research/auto-subs-findings.md`): a script started once in
//! Resolve (Workspace > Scripts > Junctur) stays running and places what Junctur asks, one Text+
//! per speaker per caption on that speaker's own video track, styled by the caption settings, and
//! tagged so sending again restyles them in place. Unlike an SRT this reaches font, size, position
//! and outline, and shows people talking at once where the layout puts them. The SRT export stays.
//!
//! A module of its own rather than in `export`: it talks to a running program, and everything it
//! computes (frames, tracks, Text+ inputs) is in [`job`] so the script inside Resolve stays a thin
//! loop that cannot be tested here.

pub mod bridge;
pub mod install;
pub mod job;
pub mod lua;
pub mod reply;

pub use install::{install, Installed};
pub use reply::{Sent, Timeline};

use crate::edit::{Captions, EditList};
use crate::export::layout;
use crate::export::srt::Cue;

/// Installs (or refreshes) the script, asks Resolve for the open timeline and the template, then
/// places `cues` on it, laid out for that timeline's shape (side-by-side lines are wrapped to its
/// width).
pub fn send(cues: &[Cue], edit: &EditList, c: &Captions) -> Result<Sent, String> {
    if cues.is_empty() {
        return Err("no captions to send: the transcript has no kept words yet".into());
    }
    install()?;
    let info = bridge::request("info", vec![("template", job::template(c).into())])?;
    let timeline = reply::timeline(&info)?;
    let aspect = timeline.height as f64 / timeline.width.max(1) as f64;
    let placed = layout::place(cues, edit, c, aspect);
    let job = job::build(&placed, edit, c, &timeline);
    let done = bridge::request("place", job.to_lua())?;
    Ok(reply::sent(&done, &timeline.name))
}
