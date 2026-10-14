//! Background analysis run by the app: the pipeline on its own thread, its first half (tracks to
//! edit list) and its transcription on two one-at-a-time lanes, so a new recording's minutes never
//! wait behind another's hours. Apart from the player because a run outlives the open recording.

mod edits;
mod progress;
mod run;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use junctur_core::config;
use junctur_core::pipeline::{self, KeepList, PipelineOptions, Project};
use serde::Serialize;

use crate::lane::Lane;
use crate::lock::lock;

pub use edits::Editing;
pub use progress::{Phase, Progress, Transcription};

use edits::Edits;
use progress::{shape, Shape};
use run::{pipeline_halves, user_options, Halves};

/// The event every change of a run's progress is sent as.
pub const EVENT: &str = "analysis";

/// The event the transcription lane's [`Transcription`] is sent as.
pub const LANE_EVENT: &str = "transcription";

/// What a run is asked to do with what is there already.
#[derive(Debug, Clone, Copy)]
struct How {
    /// What becomes of an edit list holding the user's edits.
    keep_list: KeepList,
    /// Hear the speakers and the words again (the Correct button).
    correct: bool,
}

/// What a request to analyse again did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reanalysed {
    /// The run, when one started.
    pub progress: Option<Progress>,
    /// A fresh cut was asked for and nothing started, because the edit list holds the user's
    /// edits: the page asks first.
    pub hand_edited: bool,
}

/// What moved in a run's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Its phase, log line or stages: the app looks at the project again (the track roles, a
    /// finished run's results).
    Run,
    /// Only a track's step: the page redraws a bar, nothing else needs reading.
    Steps,
    /// The run's edit list was just written: the player takes it, and the new speech envelope.
    Listed,
    /// A piece more of the transcript is whole ([`Progress::transcribed`]): the player reads the
    /// words again.
    Transcribed,
}

/// Told of every change of a run's progress (the app sends it to the page).
pub type Notify = Arc<dyn Fn(&Progress, Change) + Send + Sync>;

#[derive(Clone)]
pub struct Analysis {
    runs: Arc<Mutex<HashMap<PathBuf, Progress>>>,
    /// Held through a run's first half (tracks to edit list).
    first: Arc<Mutex<()>>,
    /// Held through a run's transcription: one at a time, in turn and pausable.
    transcription: Arc<Lane>,
    /// Held from taking a snapshot until `notify` has it, so the speech stage's per-track threads
    /// never send an older snapshot last. Not `runs`: the app's handler asks about runs.
    order: Arc<Mutex<()>>,
    /// The lane's shape as last sent to the page ([`Analysis::lane_moved`]).
    lane_sent: Arc<Mutex<Option<Shape>>>,
    /// Edits in progress per recording ([`Analysis::edit`]); a run of one waits for its edits to
    /// end before its first half.
    editing: Arc<Edits>,
    /// The cache the projects live in; `None` is the default one (the CLI's).
    cache_dir: Option<PathBuf>,
    /// The user's options a run starts from ([`user_options`]); tests pass their own, so they
    /// never read the user's config or voices files.
    user: fn() -> Result<PipelineOptions, String>,
    /// The pipeline a run goes through ([`pipeline_halves`]); tests pass their own, to drive the
    /// lanes without media or models.
    halves: Halves,
}

impl Default for Analysis {
    fn default() -> Self {
        Self {
            runs: Default::default(),
            first: Default::default(),
            transcription: Default::default(),
            order: Default::default(),
            lane_sent: Default::default(),
            editing: Default::default(),
            cache_dir: None,
            user: user_options,
            halves: pipeline_halves,
        }
    }
}

/// The key a recording's runs are kept under: its absolute path, as the project names it.
fn key(file: &Path) -> PathBuf {
    std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf())
}

/// Why `file`'s analysis is out of date with the settings now (see `pipeline::outdated`); a config
/// file that cannot be read says so instead. Only the transcript's settings count, so the voices
/// file is not read: a broken one is no reason to analyse again.
pub fn outdated(file: &Path) -> Vec<String> {
    let p = match Project::open(file, None, None) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    match config::analysis_options(None, false, true) {
        Ok(o) => pipeline::outdated(&p, &o),
        Err(e) => vec![format!("The settings could not be read: {e}")],
    }
}

impl Analysis {
    #[cfg(test)]
    fn with_cache(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir: Some(cache_dir),
            user: || Ok(PipelineOptions::default()),
            ..Self::default()
        }
    }

    /// A run that could not start (its project could not be opened), recorded so the page shows
    /// the error where it shows progress. A busy run is returned as it is: marking it failed while
    /// its thread goes on would let the recording take a second place on the lane.
    pub fn failed(&self, file: &Path, error: String) -> Progress {
        let k = key(file);
        let mut runs = lock(&self.runs);
        if let Some(r) = runs.get(&k).filter(|r| r.phase.busy()) {
            return r.clone();
        }
        let r = Progress {
            file: k.display().to_string(),
            phase: Phase::Failed,
            line: None,
            error: Some(error),
            project: self.status(&k),
            steps: Vec::new(),
            listed: false,
            transcribed: None,
        };
        runs.insert(k, r.clone());
        r
    }

    pub fn progress(&self, file: &Path) -> Option<Progress> {
        lock(&self.runs).get(&key(file)).cloned()
    }

    /// A run of `file` is waiting or running.
    pub fn busy(&self, file: &Path) -> bool {
        self.progress(file).is_some_and(|p| p.phase.busy())
    }

    /// Analyses `file` in the background, keeping an edit list with the user's edits; a run already
    /// waiting or running is returned as it is.
    pub fn start(&self, file: &Path, notify: Notify) -> Result<Progress, String> {
        self.start_with(
            file,
            notify,
            How {
                keep_list: KeepList::Edits,
                correct: false,
            },
        )
    }

    fn start_with(&self, file: &Path, notify: Notify, how: How) -> Result<Progress, String> {
        // Before opening the project: a run going on is the answer even when the folder cannot be
        // opened just now (the check under the lock below closes the race).
        if let Some(r) = self.progress(file).filter(|r| r.phase.busy()) {
            return Ok(r);
        }
        let p = Project::open(file, self.cache_dir.clone(), None).map_err(|e| e.to_string())?;
        let k = p.file.clone();
        let first = {
            let mut runs = lock(&self.runs);
            if let Some(r) = runs.get(&k).filter(|r| r.phase.busy()) {
                return Ok(r.clone());
            }
            let r = Progress {
                file: k.display().to_string(),
                phase: Phase::Queued,
                line: None,
                error: None,
                project: self.status(&k),
                steps: Vec::new(),
                listed: false,
                transcribed: None,
            };
            runs.insert(k.clone(), r.clone());
            r
        };
        notify(&first, Change::Run);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("analysis".into())
            .spawn(move || me.run(&p, &notify, how));
        if let Err(e) = spawned {
            let e = format!("could not start the analysis: {e}");
            self.update(&k, &|_, _| {}, Change::Run, |r| {
                r.phase = Phase::Failed;
                r.error = Some(e.clone());
            });
            return Err(e);
        }
        Ok(first)
    }

    /// Analyses `file` again with the settings now. Not `fresh`: the user's edit list is kept and
    /// only this run's speakers go into it. `fresh`: the list is re-cut, so with the user's edits
    /// in it nothing starts unless `replace`, and the page asks first.
    pub fn reanalyse(
        &self,
        file: &Path,
        fresh: bool,
        replace: bool,
        notify: Notify,
    ) -> Result<Reanalysed, String> {
        if self.busy(file) {
            return Err("this recording is already being analysed".into());
        }
        if fresh && !replace {
            let p = Project::open(file, self.cache_dir.clone(), None).map_err(|e| e.to_string())?;
            if pipeline::hand_edited(&p).map_err(|e| e.to_string())? {
                return Ok(Reanalysed {
                    progress: None,
                    hand_edited: true,
                });
            }
        }
        Ok(Reanalysed {
            progress: Some(self.start_with(
                file,
                notify,
                How {
                    keep_list: if fresh { KeepList::No } else { KeepList::Whole },
                    correct: false,
                },
            )?),
            hand_edited: false,
        })
    }

    /// The Correct button's run: Re-analyse keeping the user's list, with the speakers heard and
    /// the words transcribed again, fixes carried onto the new words. Refused while being analysed.
    pub fn correct(&self, file: &Path, notify: Notify) -> Result<Progress, String> {
        if self.busy(file) {
            return Err("this recording is already being analysed".into());
        }
        self.start_with(
            file,
            notify,
            How {
                keep_list: KeepList::Whole,
                correct: true,
            },
        )
    }

    /// Pauses or resumes the transcription lane after the current chunk; whether anything changed.
    /// `send` runs under `order`, so a pause's snapshot never lands after a newer one.
    pub fn pause(&self, paused: bool, send: &dyn Fn(&Transcription)) -> bool {
        let _order = lock(&self.order);
        let changed = self.transcription.pause(paused);
        let now = self.transcription();
        *lock(&self.lane_sent) = Some(shape(&now));
        send(&now);
        changed
    }

    /// The lane, when its shape (members, phases, pause) moved since last sent; called under
    /// `order`. Each job's progress goes through its own `analysis` events.
    pub fn lane_moved(&self) -> Option<Transcription> {
        let now = self.transcription();
        let mut last = lock(&self.lane_sent);
        let s = shape(&now);
        if last.as_ref() == Some(&s) {
            return None;
        }
        *last = Some(s);
        Some(now)
    }

    /// Who is on the transcription lane, in turn, with each one's progress.
    pub fn transcription(&self) -> Transcription {
        let v = self.transcription.view();
        let runs = lock(&self.runs);
        Transcription {
            paused: v.paused,
            stopped: v.stopped,
            jobs: v
                .queue
                .iter()
                .filter_map(|k| runs.get(k).cloned())
                .collect(),
        }
    }

    /// Runs `f` on the first-half lane, for work that loads the speaker models outside a run. Not
    /// on the transcription lane: minutes of work never wait on another recording's hours.
    pub fn exclusive<T>(&self, f: impl FnOnce() -> T) -> T {
        let _turn = lock(&self.first);
        f()
    }
}

#[cfg(test)]
mod tests;
