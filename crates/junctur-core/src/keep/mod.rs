//! Smart keep: the user says what kind of video they are making, and the parts where nobody talks that
//! are busier than the recording usually is are suggested as keeps ([`rules`], phase 3d); talk stays
//! as the automatic cut leaves it. The suggestions are what the user accepts, dismisses or undoes
//! ([`suggest`]). Phase 3b had Laya, a decision model, judge every part instead, and learn a cutoff
//! and temperature from the user's answers ([`laya`], [`run`], [`state`], [`fit`], [`learn`],
//! [`agreement`]): that code stays here, but smart keep no longer calls it, since base Laya judged no
//! better than chance (ideas.md). A module of its own because it spans the labels, the audio and the
//! edit list; the pipeline only runs the rules as its `keep` stage.

mod agreement;
pub mod fit;
pub mod laya;
mod learn;
pub mod parts;
pub mod rules;
mod run;
pub mod state;
mod suggest;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use agreement::{agreement, Agreement, Score};
pub use fit::Fit;
pub use learn::{collect, fitted, load_pairs, pairs_path, store_pairs, Pair};
pub use parts::{Kind, Part};
pub use run::{run, score_windows, Job};
pub use suggest::{accept, dismiss, suggestions, undo, Item, ItemState, Summary};

use crate::Result;

/// Smart keep's suggested keeps for a recording, in the project folder: rebuilt by every keep run.
pub const FILE: &str = "keep.json";
/// The user's prompt and answers for a recording, in the project folder: theirs, never rebuilt.
pub const CHOICES: &str = "keep_choices.json";

/// Ready prompts the app offers.
pub const PRESETS: [(&str, &str); 3] = [
    (
        "League highlights",
        "League of Legends highlights with friends: keep the fights, the plays and the funny moments, cut the waiting, walking and menus",
    ),
    (
        "Full VOD minus dead air",
        "The whole stream as it happened, only the dead air cut: keep anything where something happens or someone talks",
    ),
    (
        "Podcast-style talk",
        "A podcast-style video of the conversation: keep the talk that goes somewhere, cut the game noise, silences and small talk",
    ),
];

/// Smart keep in the config file: off by default; `last` is the prompt last set on any recording,
/// which a recording without its own is judged with when this is on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default)]
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

/// What smart keep made of a recording: the rules' busy silent stretches, or (3b) Laya's every part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judged {
    /// Who judged: [`rules::NAME`], or [`laya::NAME`] in a file 3b wrote.
    pub model: String,
    pub prompt: String,
    /// The judge's version ([`rules::VERSION`], [`state::VERSION`]).
    pub version: u32,
    pub parts: Vec<Judgement>,
}

/// One part, judged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judgement {
    pub start: f64,
    pub end: f64,
    pub kind: Kind,
    /// Laya's keep logit minus its cut logit, or how sure the rules are of a span (0 at the line to 4
    /// at the top): P(keep) is sigmoid(margin / T) ([`Fit`]).
    pub margin: f64,
    /// What Laya read (kept for its pairs), or why the rules keep it.
    pub state: String,
    /// Laya's: of the model, the question and the state, a part whose hash is the same is not judged
    /// again. Empty from the rules.
    pub hash: String,
}

impl Judgement {
    pub fn part(&self) -> Part {
        Part {
            start: self.start,
            end: self.end,
            kind: self.kind,
        }
    }
}

/// The user's answers for a recording, and its prompt.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Choices {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default)]
    pub answers: Vec<Answer>,
}

/// A suggestion the user answered: the part, and whether they keep it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub start: f64,
    pub end: f64,
    pub keep: bool,
    pub how: How,
    /// Accepted with every other suggestion at once (Accept all): Laya's own choice, not a decision
    /// about this part, so it is no training pair ([`collect`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub bulk: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// Laya's suggestion applied.
    Accepted,
    /// Laya's suggestion turned down, or an accepted one undone.
    Dismissed,
}

impl Choices {
    pub fn answer(&self, p: &Part) -> Option<&Answer> {
        self.answers.iter().find(|a| same(a.start, a.end, p))
    }

    /// `p`'s answer while the list (`now`, sorted) still does what it says. One the list no longer
    /// does (an accept taken back with the editor's Undo, a part edited by hand since) no longer
    /// stands: the part is as if never answered, and an edit of it by hand is a hand edit.
    pub fn standing(&self, p: &Part, now: &[crate::edit::Range]) -> Option<&Answer> {
        self.answer(p).filter(|a| p.kept_by(now) == a.keep)
    }

    /// Records `a`, replacing any answer for the same part.
    pub fn set(&mut self, a: Answer) {
        // Matched as `answer` matches, so a part read back a hair off is replaced, not doubled.
        let p = Part {
            start: a.start,
            end: a.end,
            kind: Kind::Talk,
        };
        self.answers.retain(|x| !same(x.start, x.end, &p));
        self.answers.push(a);
        self.answers.sort_by(|x, y| x.start.total_cmp(&y.start));
    }
}

fn same(start: f64, end: f64, p: &Part) -> bool {
    (start - p.start).abs() < 1e-6 && (end - p.end).abs() < 1e-6
}

/// The prompt a recording is judged with: its own, else the last one when smart keep is on.
pub fn prompt_for(choices: &Choices, settings: &Settings) -> Option<String> {
    let own = choices
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let last = settings
        .last
        .as_deref()
        .map(str::trim)
        .filter(|p| settings.on && !p.is_empty());
    own.or(last).map(str::to_string)
}

/// Smart keep's suggested keeps in project folder `dir`, as the rules wrote them; `None` when there
/// are none, they do not read, or Laya wrote them (3b): its cuts of talk are never shown again, and the
/// next keep run writes the rules'.
pub fn load(dir: &Path) -> Option<Judged> {
    read(dir).filter(|j| j.model == rules::NAME && j.version == rules::VERSION)
}

/// `keep.json` in `dir` whoever wrote it.
fn read(dir: &Path) -> Option<Judged> {
    serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
}

pub fn save(dir: &Path, j: &Judged) -> Result<()> {
    crate::output::write_replacing(&dir.join(FILE), serde_json::to_vec_pretty(j)?)?;
    Ok(())
}

fn choices_path(dir: &Path) -> PathBuf {
    dir.join(CHOICES)
}

/// The user's answers in `dir`: none when there is no file; an error when it is there and does not
/// read, since it is theirs and must not be written over by an empty one.
pub fn load_choices(dir: &Path) -> Result<Choices> {
    match std::fs::read(choices_path(dir)) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Choices::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_choices(dir: &Path, c: &Choices) -> Result<()> {
    crate::output::write_replacing(&choices_path(dir), serde_json::to_vec_pretty(c)?)?;
    Ok(())
}

/// One line for Laya's log (3b).
pub fn summary(j: &Judged, fit: &Fit) -> String {
    let keep = j.parts.iter().filter(|p| fit.keeps(p.margin)).count();
    let unsure = j.parts.iter().filter(|p| fit.uncertain(p.margin)).count();
    format!(
        "keep: Laya judged {} parts, {keep} to keep and {} to cut, {unsure} uncertain (turning at Laya's {:.2}, temperature {:.2}, from {} of your decisions)",
        j.parts.len(),
        j.parts.len() - keep,
        fit.cutoff,
        fit.temperature,
        fit.pairs
    )
}

#[cfg(test)]
mod tests;
