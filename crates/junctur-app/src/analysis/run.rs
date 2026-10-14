//! A run on its thread: the first half under its lock, the transcription on the lane, and every
//! log line and step turned into progress for the page.

use std::path::Path;

use junctur_core::config;
use junctur_core::pipeline::{self, PipelineOptions, Project, Step, Steps};

use super::progress::{Phase, Progress};
use super::{Analysis, Change, How, Notify};
use crate::lock::lock;
use crate::project::{self, ProjectStatus};

/// A run's second half (the transcription), as its first half leaves it: told the log, the steps,
/// where the transcript is whole, and called between chunks (where a pause holds it).
pub(super) type Second<'a> = Box<
    dyn FnOnce(
            &mut dyn FnMut(&str),
            Steps,
            &mut dyn FnMut(f64),
            &mut dyn FnMut(),
        ) -> Result<(), String>
        + 'a,
>;

/// A run's first half (tracks to edit list), returning its second.
pub(super) type Halves = for<'a> fn(
    &'a Project,
    &'a PipelineOptions,
    &mut dyn FnMut(&str),
    Steps,
) -> Result<Second<'a>, String>;

/// `pipeline::prepare`, then `pipeline::finish` as the second half.
pub(super) fn pipeline_halves<'a>(
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
pub(super) fn user_options() -> Result<PipelineOptions, String> {
    config::analysis_options(None, false, false).map_err(|e| e.to_string())
}

/// The message a panic carried, for the run's error.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".into())
}

impl Analysis {
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

    pub(super) fn run(&self, p: &Project, notify: &Notify, how: How) {
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
    pub(super) fn update(
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
    pub(super) fn step(&self, k: &Path, s: Step, notify: &dyn Fn(&Progress, Change)) {
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

    pub(super) fn status(&self, file: &Path) -> Option<ProjectStatus> {
        project::status(file, self.cache_dir.clone()).ok()
    }
}
