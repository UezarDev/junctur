//! Labels for the parts where nobody talks (phase 3a): each silent piece of a recording is looked at
//! every 2 s by SigLIP 2, scored against a list of labels for the kind of video, and given a busy
//! score from the game's audio. The result, `labels.json` in the project folder, is what the editor's
//! Labels lane shows and what Laya (3b) reads. A module of its own because it spans the video
//! frames, a model and the audio; the pipeline only calls [`run`] as its `labels` stage.

pub mod busy;
mod lists;
pub mod model;
pub mod pieces;
mod run;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use busy::BusyTrack;
pub use lists::{Added, Kind, Label, Settings};
pub use pieces::Piece;
pub use run::{reused, run, Job};

use crate::Result;

/// The file in a project folder.
pub const FILE: &str = "labels.json";
/// Labels kept per piece, the likeliest first.
pub const TOP: usize = 3;

/// A recording's labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Labels {
    /// The model that looked ([`model::NAME`]).
    pub model: String,
    pub kind: Kind,
    /// The labels scored, as shown.
    pub labels: Vec<String>,
    /// What the model read for each, in the same order.
    pub prompts: Vec<String>,
    /// The track busy is read from; `None` when the recording has neither a game nor a mix track.
    pub busy_track: Option<BusyTrack>,
    pub pieces: Vec<Labelled>,
}

/// One silent piece, labelled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Labelled {
    pub start: f64,
    pub end: f64,
    /// Frames looked at; 0 when none could be decoded (the scores are then empty).
    pub frames: usize,
    /// The [`TOP`] likeliest labels with their share (the mean over its frames of the softmax over
    /// the list: all labels' shares sum to 1).
    pub top: Vec<Share>,
    /// The share of the recording's seconds quieter than this piece on the busy track, 0 to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy: Option<f64>,
    /// The piece's mean peak level on the busy track, in dBFS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_db: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Share {
    pub label: String,
    pub score: f64,
}

impl Labelled {
    pub fn piece(&self) -> Piece {
        Piece {
            start: self.start,
            end: self.end,
        }
    }
}

/// The labels in project folder `dir`; `None` when there are none or they do not read.
pub fn load(dir: &Path) -> Option<Labels> {
    serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
}

/// Writes `l` as [`FILE`] in `dir`, whole or not at all.
pub fn save(dir: &Path, l: &Labels) -> Result<()> {
    crate::output::write_replacing(&dir.join(FILE), serde_json::to_vec_pretty(l)?)?;
    Ok(())
}

/// One line for the log: how much was labelled and the top labels met most often.
pub fn summary(l: &Labels) -> String {
    let mut count: Vec<(&str, usize)> = Vec::new();
    for p in &l.pieces {
        if let Some(top) = p.top.first() {
            match count.iter_mut().find(|(t, _)| *t == top.label) {
                Some((_, n)) => *n += 1,
                None => count.push((&top.label, 1)),
            }
        }
    }
    count.sort_by_key(|c| std::cmp::Reverse(c.1));
    let seconds: f64 = l.pieces.iter().map(|p| p.end - p.start).sum();
    let frames: usize = l.pieces.iter().map(|p| p.frames).sum();
    let most: Vec<String> = count.iter().map(|(t, n)| format!("{t} {n}")).collect();
    format!(
        "labels: {} silent pieces ({seconds:.0} s), {frames} frames; on top: {}",
        l.pieces.len(),
        if most.is_empty() {
            "none".into()
        } else {
            most.join(", ")
        }
    )
}

/// Whether `l` answers `labels` over `pieces` with this model: the same prompts in the same order
/// and the same pieces. Then the stage reuses it.
pub fn answers(l: &Labels, labels: &[Label], pieces: &[Piece]) -> bool {
    l.model == model::NAME
        && l.prompts.len() == labels.len()
        && l.prompts.iter().zip(labels).all(|(p, x)| *p == x.prompt)
        && l.labels.iter().zip(labels).all(|(t, x)| *t == x.text)
        && l.pieces.len() == pieces.len()
        && l.pieces.iter().zip(pieces).all(|(a, b)| a.piece() == *b)
}

#[cfg(test)]
mod tests;
