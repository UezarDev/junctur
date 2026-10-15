//! What the page is told of a run: its phase, progress and the transcription lane.

use junctur_core::pipeline::Step;
use serde::Serialize;

use crate::project::ProjectStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Waiting for another recording's first half (tracks to edit list) to finish.
    Queued,
    Running,
    /// The edit list is written (editable); the transcription waits for another recording's.
    Waiting,
    Done,
    Failed,
}

impl Phase {
    pub fn busy(self) -> bool {
        matches!(self, Phase::Queued | Phase::Running | Phase::Waiting)
    }
}

/// Where a recording's run is.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// The recording, absolute (the project's own path): the key the page matches events by.
    pub file: String,
    pub phase: Phase,
    /// The pipeline's last log line (a stage starting, Whisper's percentage, ...).
    pub line: Option<String>,
    pub error: Option<String>,
    /// The project as `jobs.json` and the files say now: each stage's state.
    pub project: Option<ProjectStatus>,
    /// Per track, in track order, the latest stage it reported and how far through it it is.
    /// Only the running stage's are current.
    pub steps: Vec<Step>,
    /// The run's edit list is written: it can be edited, and nothing later in the run replaces it.
    pub listed: bool,
    /// While transcribing: every word before this many seconds is transcribed on every track.
    /// `None` before the transcription starts and once it is whole.
    pub transcribed: Option<f64>,
}

impl Progress {
    /// How far the editor's words go ([`crate::player::Core::transcribed`]): once the list is
    /// written and the transcription is pending, running or failed part way, the words so far;
    /// else `None` (the finished transcript), so an older `transcript.json` never sits under new
    /// indices.
    pub fn words_through(&self) -> Option<f64> {
        if self.listed && (self.phase.busy() || self.phase == Phase::Failed) {
            Some(self.transcribed.unwrap_or(0.0))
        } else {
            None
        }
    }
}

/// The transcription lane as the page shows it, whichever recording is open.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcription {
    /// Pause was asked for: the transcribing recording stops after its chunk, the rest wait.
    pub paused: bool,
    /// The transcribing recording has stopped at a chunk boundary (paused, and no longer
    /// finishing a chunk).
    pub stopped: bool,
    /// The recordings on the lane, the transcribing one first, then those waiting in the order
    /// they will run.
    pub jobs: Vec<Progress>,
}

/// What of the lane the page needs a new [`Transcription`] for: its members, in turn, with their
/// phases, and the pause.
pub(super) type Shape = (bool, bool, Vec<(String, Phase)>);

pub(super) fn shape(t: &Transcription) -> Shape {
    let jobs = t.jobs.iter().map(|p| (p.file.clone(), p.phase)).collect();
    (t.paused, t.stopped, jobs)
}
