//! Analysis run by the app itself (2d): a recording opened without an edit list is analysed in the
//! background while it already plays, and an analysed one can be analysed again when the settings
//! moved. Each run is `pipeline::run` on its own thread with the options `junctur analyze` uses
//! (`config::analysis_options`), and its log lines become [`Progress`] for the project panel.
//!
//! Its own module because none of it is the player's: a run outlives the recording being open (it
//! resumes where it stopped if the app closes, as the CLI's does), and several recordings may be
//! waiting. Runs go one at a time behind `gate`, the pipeline's own rule: Whisper and the ONNX
//! models share one GPU. Rejected: cancelling (the pipeline has no point to stop at between its
//! stages' own steps), and running two at once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use junctur_core::config;
use junctur_core::pipeline::{self, PipelineOptions, Project};
use serde::Serialize;

use crate::lock::lock;
use crate::project::{self, ProjectStatus};

/// The event every change of a run's progress is sent as.
pub const EVENT: &str = "analysis";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Waiting for another recording's run to finish.
    Queued,
    Running,
    Done,
    Failed,
}

impl Phase {
    pub fn busy(self) -> bool {
        matches!(self, Phase::Queued | Phase::Running)
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
}

/// What a request to analyse again did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reanalysed {
    /// The run, when one started.
    pub progress: Option<Progress>,
    /// Nothing started because the edit list holds the user's edits: the page asks first.
    pub hand_edited: bool,
}

/// Told of every change of a run's progress (the app sends it to the page).
pub type Notify = Arc<dyn Fn(&Progress) + Send + Sync>;

#[derive(Clone)]
pub struct Analysis {
    runs: Arc<Mutex<HashMap<PathBuf, Progress>>>,
    gate: Arc<Mutex<()>>,
    /// The cache the projects live in; `None` is the default one (the CLI's).
    cache_dir: Option<PathBuf>,
    /// The user's options a run starts from ([`user_options`]); tests pass their own, so they
    /// never read the user's config or voices files.
    user: fn() -> Result<PipelineOptions, String>,
}

impl Default for Analysis {
    fn default() -> Self {
        Self {
            runs: Default::default(),
            gate: Default::default(),
            cache_dir: None,
            user: user_options,
        }
    }
}

/// The options `junctur analyze` starts from: model from the config file, remembered voices.
fn user_options() -> Result<PipelineOptions, String> {
    config::analysis_options(None, false, false).map_err(|e| e.to_string())
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

    /// The options a run of `p` uses: the user's, device auto, and the cut settings of the list on
    /// disk, so a re-analysis keeps settings `junctur cut` set. Never `force`: stages whose results
    /// still answer the options are reused.
    fn options(&self, p: &Project) -> Result<PipelineOptions, String> {
        let mut o = (self.user)()?;
        if let Ok(e) = p.edit() {
            o.cut = e.settings;
        }
        Ok(o)
    }

    /// A run that could not start (its project could not be opened), recorded so the page shows
    /// the error where it shows progress, not as a failed open.
    pub fn failed(&self, file: &Path, error: String) -> Progress {
        let k = key(file);
        let r = Progress {
            file: k.display().to_string(),
            phase: Phase::Failed,
            line: None,
            error: Some(error),
            project: self.status(&k),
        };
        lock(&self.runs).insert(k, r.clone());
        r
    }

    pub fn progress(&self, file: &Path) -> Option<Progress> {
        lock(&self.runs).get(&key(file)).cloned()
    }

    /// A run of `file` is waiting or running.
    pub fn busy(&self, file: &Path) -> bool {
        self.progress(file).is_some_and(|p| p.phase.busy())
    }

    /// Analyses `file` in the background; a run already waiting or running is returned as it is.
    pub fn start(&self, file: &Path, notify: Notify) -> Result<Progress, String> {
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
            };
            runs.insert(k.clone(), r.clone());
            r
        };
        notify(&first);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("analysis".into())
            .spawn(move || me.run(&p, &notify));
        if let Err(e) = spawned {
            let e = format!("could not start the analysis: {e}");
            self.update(&k, &|_| {}, |r| {
                r.phase = Phase::Failed;
                r.error = Some(e.clone());
            });
            return Err(e);
        }
        Ok(first)
    }

    /// Analyses `file` again with the settings now. With the user's edits in the edit list it
    /// starts nothing unless `replace` (the edits are then kept as a backup by the pipeline).
    pub fn reanalyse(
        &self,
        file: &Path,
        replace: bool,
        notify: Notify,
    ) -> Result<Reanalysed, String> {
        if self.busy(file) {
            return Err("this recording is already being analysed".into());
        }
        if !replace {
            let p = Project::open(file, self.cache_dir.clone(), None).map_err(|e| e.to_string())?;
            if pipeline::hand_edited(&p).map_err(|e| e.to_string())? {
                return Ok(Reanalysed {
                    progress: None,
                    hand_edited: true,
                });
            }
        }
        Ok(Reanalysed {
            progress: Some(self.start(file, notify)?),
            hand_edited: false,
        })
    }

    fn run(&self, p: &Project, notify: &Notify) {
        let _turn = lock(&self.gate);
        let k = p.file.clone();
        self.update(&k, notify.as_ref(), |r| r.phase = Phase::Running);
        let name = k.file_name().map(|n| n.to_string_lossy().into_owned());
        let result = self.options(p).and_then(|o| {
            pipeline::run(p, &o, &mut |line| {
                eprintln!(
                    "junctur-app analysis ({}): {line}",
                    name.as_deref().unwrap_or("")
                );
                self.update(&k, notify.as_ref(), |r| r.line = Some(line.to_string()));
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
        });
        if let Err(e) = &result {
            eprintln!("junctur-app analysis failed: {e}");
        }
        self.update(&k, notify.as_ref(), |r| match &result {
            Ok(()) => r.phase = Phase::Done,
            Err(e) => {
                r.phase = Phase::Failed;
                r.error = Some(e.clone());
            }
        });
    }

    /// Changes `k`'s progress, reads its project status again and tells `notify` (outside the
    /// lock: the app's handler takes the player's).
    fn update(&self, k: &Path, notify: &dyn Fn(&Progress), f: impl FnOnce(&mut Progress)) {
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
        notify(&now);
    }

    fn status(&self, file: &Path) -> Option<ProjectStatus> {
        project::status(file, self.cache_dir.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_run_reports_until_it_ends_and_is_not_started_twice() {
        let d = std::env::temp_dir().join(format!("junctur-app-analysis-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Not a video: the tracks stage fails (ffprobe refuses it, or is not there at all).
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let a = Analysis::with_cache(d.join("cache"));
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let notify: Notify = Arc::new(move |p: &Progress| {
            let _ = lock(&tx).send(p.clone());
        });
        // Hold the gate so the run waits, queued, and a second request finds it.
        let held = lock(&a.gate);
        let first = a.start(&file, notify.clone()).unwrap();
        assert_eq!(first.phase, Phase::Queued);
        assert!(a.busy(&file));
        let again = a.start(&file, notify.clone()).unwrap();
        assert_eq!(again.phase, Phase::Queued);
        assert!(a.reanalyse(&file, true, notify).is_err(), "already running");
        drop(held);
        let mut phases = Vec::new();
        loop {
            let p = rx.recv_timeout(Duration::from_secs(60)).unwrap();
            phases.push(p.phase);
            if !p.phase.busy() {
                assert_eq!(p.phase, Phase::Failed);
                assert!(p.error.is_some());
                assert_eq!(p.file, key(&file).display().to_string());
                let tracks = p.project.unwrap().stages[0].clone();
                assert_eq!(tracks.name, "tracks");
                assert_eq!(tracks.state, pipeline::StageState::Failed);
                break;
            }
        }
        assert_eq!(phases.iter().filter(|p| **p == Phase::Queued).count(), 1);
        assert!(phases.contains(&Phase::Running));
        assert!(!a.busy(&file));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_to_lose_without_an_edit_list() {
        let d = std::env::temp_dir().join(format!("junctur-app-reanalyse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let a = Analysis::with_cache(d.join("cache"));
        let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
        // A hand edit: asked first, nothing started.
        std::fs::write(p.edit_path(), "junctur-edit 1\nkeep x\n").unwrap();
        let r = a
            .reanalyse(&file, false, Arc::new(|_: &Progress| {}))
            .unwrap();
        assert!(r.hand_edited && r.progress.is_none());
        assert!(!a.busy(&file));
        // Told to replace it: started.
        let r = a
            .reanalyse(&file, true, Arc::new(|_: &Progress| {}))
            .unwrap();
        assert!(!r.hand_edited && r.progress.is_some());
        // Wait for it to end before removing its folder.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while a.busy(&file) {
            assert!(std::time::Instant::now() < deadline, "the run never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
