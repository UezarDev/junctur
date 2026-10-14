//! Background analysis run by the app: the pipeline on its own thread, its first half (tracks to
//! edit list) and its transcription on two one-at-a-time lanes, so a new recording's minutes never
//! wait behind another's hours. Apart from the player because a run outlives the open recording.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use junctur_core::config;
use junctur_core::pipeline::{self, KeepList, PipelineOptions, Project, Step, Steps};
use serde::Serialize;

use crate::lane::Lane;
use crate::lock::lock;
use crate::project::{self, ProjectStatus};

/// The event every change of a run's progress is sent as.
pub const EVENT: &str = "analysis";

/// The event the transcription lane's [`Transcription`] is sent as.
pub const LANE_EVENT: &str = "transcription";

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
type Shape = (bool, bool, Vec<(String, Phase)>);

fn shape(t: &Transcription) -> Shape {
    let jobs = t.jobs.iter().map(|p| (p.file.clone(), p.phase)).collect();
    (t.paused, t.stopped, jobs)
}

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

/// A run's second half (the transcription), as its first half leaves it: told the log, the steps,
/// where the transcript is whole, and called between chunks (where a pause holds it).
type Second<'a> = Box<
    dyn FnOnce(
            &mut dyn FnMut(&str),
            Steps,
            &mut dyn FnMut(f64),
            &mut dyn FnMut(),
        ) -> Result<(), String>
        + 'a,
>;

/// A run's first half (tracks to edit list), returning its second.
type Halves = for<'a> fn(
    &'a Project,
    &'a PipelineOptions,
    &mut dyn FnMut(&str),
    Steps,
) -> Result<Second<'a>, String>;

/// `pipeline::prepare`, then `pipeline::finish` as the second half.
fn pipeline_halves<'a>(
    p: &'a Project,
    o: &'a PipelineOptions,
    log: &mut dyn FnMut(&str),
    steps: Steps,
) -> Result<Second<'a>, String> {
    let prepared = pipeline::prepare(p, o, log, steps).map_err(|e| e.to_string())?;
    Ok(Box::new(move |log, steps, transcribed, between| {
        pipeline::finish(p, o, prepared, log, steps, transcribed, between)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }))
}

/// The options `junctur analyze` starts from: model from the config file, remembered voices.
fn user_options() -> Result<PipelineOptions, String> {
    config::analysis_options(None, false, false).map_err(|e| e.to_string())
}

/// How many edits of each recording are in progress, and a way to wait for them to end.
#[derive(Default)]
struct Edits {
    count: Mutex<HashMap<PathBuf, usize>>,
    ended: Condvar,
}

/// An edit of a recording in progress ([`Analysis::edit`]): until it drops, no run of that
/// recording starts its first half.
pub struct Editing {
    edits: Arc<Edits>,
    k: PathBuf,
}

impl Drop for Editing {
    fn drop(&mut self) {
        let mut count = lock(&self.edits.count);
        if let Some(n) = count.get_mut(&self.k) {
            *n -= 1;
            if *n == 0 {
                count.remove(&self.k);
            }
        }
        self.edits.ended.notify_all();
    }
}

/// A recording whose run is `r` may be edited: no run, or its run has written the list, which
/// nothing later in the run replaces.
fn may_edit(r: Option<&Progress>) -> bool {
    r.is_none_or(|p| !p.phase.busy() || p.listed)
}

/// The message a panic carried, for the run's error.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".into())
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

    /// The options a run of `p` uses: the user's, and the cut settings of the list on disk, so a
    /// re-analysis keeps settings `junctur cut` set. Never `force`: still-valid stages are reused.
    fn options(&self, p: &Project, how: How) -> Result<PipelineOptions, String> {
        let mut o = (self.user)()?;
        if let Ok(e) = p.edit() {
            o.cut = e.settings;
        }
        o.keep_list = how.keep_list;
        o.rehear = how.correct;
        o.retranscribe = how.correct;
        Ok(o)
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

    /// `file`'s edit list may be edited ([`may_edit`]); the app asks through [`Self::edit`], which
    /// also holds a run off while the edit works.
    #[cfg(test)]
    fn editable(&self, file: &Path) -> bool {
        may_edit(self.progress(file).as_ref())
    }

    /// Marks an edit of `file` in progress when it may be edited ([`may_edit`]), else `None`; a run
    /// asked for meanwhile waits before its first half. Check and count are one step under `runs`,
    /// so an edit (an enrolled pin) and a run's speakers stage never overlap.
    pub fn edit(&self, file: &Path) -> Option<Editing> {
        let k = key(file);
        let runs = lock(&self.runs);
        if !may_edit(runs.get(&k)) {
            return None;
        }
        *lock(&self.editing.count).entry(k.clone()).or_default() += 1;
        drop(runs);
        Some(Editing {
            edits: self.editing.clone(),
            k,
        })
    }

    /// Blocks until no edit of `k` is in progress ([`Self::edit`]).
    fn wait_for_edits(&self, k: &Path) {
        let mut count = lock(&self.editing.count);
        while count.contains_key(k) {
            count = self
                .editing
                .ended
                .wait(count)
                .unwrap_or_else(|e| e.into_inner());
        }
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

    fn run(&self, p: &Project, notify: &Notify, how: How) {
        let k = p.file.clone();
        // A panic must end the run `Failed`, or it stays busy for good; shared state survives the
        // unwind (poison-tolerant locks, the lane's place a `Turn`).
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.halves(p, notify, how)))
                .unwrap_or_else(|e| {
                    Err(format!(
                        "the analysis stopped on an internal error: {}",
                        panic_message(e.as_ref())
                    ))
                });
        if let Err(e) = &result {
            eprintln!("junctur-app analysis failed: {e}");
        }
        self.update(&k, notify.as_ref(), Change::Run, |r| match &result {
            Ok(()) => {
                r.phase = Phase::Done;
                r.transcribed = None;
            }
            Err(e) => {
                r.phase = Phase::Failed;
                r.error = Some(e.clone());
                // Failed part way: the finished pieces stay; none yet is 0, not an older
                // run's transcript.
                if r.listed {
                    r.transcribed = r.transcribed.or(Some(0.0));
                }
            }
        });
    }

    /// The pipeline's first half on the `first` lane, then its transcription on the
    /// `transcription` lane, telling the page when the list is written and as pieces land.
    fn halves(&self, p: &Project, notify: &Notify, how: How) -> Result<(), String> {
        let k = p.file.clone();
        let notify = notify.as_ref();
        let name = k.file_name().map(|n| n.to_string_lossy().into_owned());
        let steps = |s: Step| self.step(&k, s, notify);
        let mut log = |line: &str| {
            eprintln!(
                "junctur-app analysis ({}): {line}",
                name.as_deref().unwrap_or("")
            );
            self.update(&k, notify, Change::Run, |r| r.line = Some(line.to_string()));
        };
        // Before anything of the recording is read: an edit begun before this run was queued
        // ends first.
        self.wait_for_edits(&k);
        let o = self.options(p, how)?;
        let second = {
            let _turn = lock(&self.first);
            self.update(&k, notify, Change::Run, |r| r.phase = Phase::Running);
            (self.halves)(p, &o, &mut log, &steps)?
        };
        // Waiting is said only when there is someone ahead, so a lone recording never shows as
        // waiting.
        let mut waited = false;
        let _turn = self.transcription.enter(&k, || {
            waited = true;
            self.update(&k, notify, Change::Listed, |r| {
                r.listed = true;
                r.phase = Phase::Waiting;
            });
        });
        let change = if waited { Change::Run } else { Change::Listed };
        self.update(&k, notify, change, |r| {
            r.listed = true;
            r.phase = Phase::Running;
        });
        let mut transcribed = |t: f64| {
            self.update(&k, notify, Change::Transcribed, |r| r.transcribed = Some(t));
        };
        // A pause holds the run here; the page hears when it has stopped (`Change::Run` sends the
        // lane again).
        let mut between = || {
            self.transcription
                .hold(|| self.update(&k, notify, Change::Run, |_| {}))
        };
        second(&mut log, &steps, &mut transcribed, &mut between)
    }

    /// Changes `k`'s progress, reads its project status again and tells `notify` what moved
    /// (outside the `runs` lock: the app's handler takes the player's; in order, under
    /// [`Self::order`]).
    fn update(
        &self,
        k: &Path,
        notify: &dyn Fn(&Progress, Change),
        change: Change,
        f: impl FnOnce(&mut Progress),
    ) {
        let _order = lock(&self.order);
        let status = self.status(k);
        let now = {
            let mut runs = lock(&self.runs);
            let Some(r) = runs.get_mut(k) else {
                return;
            };
            f(r);
            r.project = status.or(r.project.take());
            r.clone()
        };
        notify(&now, change);
    }

    /// Takes a track's step and tells `notify` only when its stage or whole percentage moved, so
    /// at most a hundred events per track and stage. The project status is not read again.
    fn step(&self, k: &Path, s: Step, notify: &dyn Fn(&Progress, Change)) {
        let _order = lock(&self.order);
        let now = {
            let mut runs = lock(&self.runs);
            let Some(r) = runs.get_mut(k) else {
                return;
            };
            match r.steps.iter_mut().find(|x| x.track == s.track) {
                Some(x) => {
                    let moved = x.stage != s.stage || x.percent() != s.percent();
                    *x = s;
                    if !moved {
                        return;
                    }
                }
                None => {
                    r.steps.push(s);
                    r.steps.sort_by_key(|x| x.track);
                }
            }
            r.clone()
        };
        notify(&now, Change::Steps);
    }

    fn status(&self, file: &Path) -> Option<ProjectStatus> {
        project::status(file, self.cache_dir.clone()).ok()
    }
}

#[cfg(test)]
mod tests;
