//! Analysis run by the app itself (2d): a recording opened without an edit list is analysed in the
//! background while it already plays, and an analysed one can be analysed again when the settings
//! moved. Each run is `pipeline::run` on its own thread with the options `junctur analyze` uses
//! (`config::analysis_options`), and its log lines become [`Progress`] for the project panel, with
//! how far each track has got through the running stage (2h: the pipeline's [`Step`]s).
//!
//! Its own module because none of it is the player's: a run outlives the recording being open (it
//! resumes where it stopped if the app closes, as the CLI's does), and several recordings may be
//! waiting.
//!
//! A run is the pipeline's two halves on two lanes (2j): the first half (tracks, speech, speakers,
//! the edit list) behind `first`, one recording at a time, then the transcription behind
//! `transcription`, also one at a time. A recording opened while another transcribes gets its edit
//! list at once (Uezar: the fast steps are not held back by "another transcript ... going in the
//! back") and its transcription queues; once its list is written it can be edited while the rest
//! runs, the words growing a 5-minute piece at a time ([`Progress::transcribed`]). Rejected:
//! cancelling (the pipeline has no point to stop at between its stages' own steps; 2k's pause will
//! stop the transcription lane between chunks), and both halves on one gate (2d), which held a new
//! recording's minutes behind another's hours.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use junctur_core::config;
use junctur_core::pipeline::{self, PipelineOptions, Project, Step, Steps};
use serde::Serialize;

use crate::lock::lock;
use crate::project::{self, ProjectStatus};

/// The event every change of a run's progress is sent as.
pub const EVENT: &str = "analysis";

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
    /// Per track, in track order, the latest stage it reported and how far through it it is (2h).
    /// Only the running stage's are current; the page shows those.
    pub steps: Vec<Step>,
    /// The run's edit list is written (2j): it can be edited, and nothing later in the run
    /// replaces it.
    pub listed: bool,
    /// While transcribing: every word before this many seconds of the recording is transcribed, on
    /// every track (a whole number of 5-minute pieces, or where a resumed run starts). `None`
    /// before the transcription starts and once it is whole.
    pub transcribed: Option<f64>,
}

impl Progress {
    /// How far the editor's words go for this run ([`crate::player::Core::transcribed`]): once the
    /// run's list is written and while its transcription is to come, runs or failed part way, the
    /// words so far (none before the first piece); else (not this far, or done) the finished
    /// transcript. Never an older run's `transcript.json` under the new words' indices (2j repair).
    pub fn words_through(&self) -> Option<f64> {
        if self.listed && (self.phase.busy() || self.phase == Phase::Failed) {
            Some(self.transcribed.unwrap_or(0.0))
        } else {
            None
        }
    }
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

/// What moved in a run's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Its phase, log line or stages: the app looks at the project again (the track roles, a
    /// finished run's results).
    Run,
    /// Only a track's step (2h): the page redraws a bar, nothing else needs reading.
    Steps,
    /// The run's edit list was just written (2j): the player takes it, and the new speech envelope.
    Listed,
    /// A piece more of the transcript is whole (2j, [`Progress::transcribed`]): the player reads
    /// the words again.
    Transcribed,
}

/// Told of every change of a run's progress (the app sends it to the page).
pub type Notify = Arc<dyn Fn(&Progress, Change) + Send + Sync>;

#[derive(Clone)]
pub struct Analysis {
    runs: Arc<Mutex<HashMap<PathBuf, Progress>>>,
    /// Held through a run's first half (tracks to edit list).
    first: Arc<Mutex<()>>,
    /// Held through a run's transcription: the one shared background job (2j).
    transcription: Arc<Mutex<()>>,
    /// Held from taking a snapshot of a run until `notify` has it, so snapshots reach the page in
    /// the order they were taken: the speech stage reports from a thread per track, and an older
    /// snapshot sent last would move a bar back (79 % after 100 %). Not `runs` itself: the app's
    /// handler asks the analysis about runs ([`Analysis::busy`]).
    order: Arc<Mutex<()>>,
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
            cache_dir: None,
            user: user_options,
            halves: pipeline_halves,
        }
    }
}

/// A run's second half (the transcription), as its first half leaves it: told the log, the steps
/// and where the transcript is whole.
type Second<'a> =
    Box<dyn FnOnce(&mut dyn FnMut(&str), Steps, &mut dyn FnMut(f64)) -> Result<(), String> + 'a>;

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
    Ok(Box::new(move |log, steps, transcribed| {
        pipeline::finish(p, o, prepared, log, steps, transcribed)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }))
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
    /// still answer the options are reused. `keep_list`: a list with the user's edits is kept
    /// (every run but an explicit re-analysis, 2j).
    fn options(&self, p: &Project, keep_list: bool) -> Result<PipelineOptions, String> {
        let mut o = (self.user)()?;
        if let Ok(e) = p.edit() {
            o.cut = e.settings;
        }
        o.keep_list = keep_list;
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
            steps: Vec::new(),
            listed: false,
            transcribed: None,
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

    /// `file`'s edit list may be edited: no run, or its run has written the list (2j), which
    /// nothing later in the run replaces.
    pub fn editable(&self, file: &Path) -> bool {
        self.progress(file)
            .is_none_or(|p| !p.phase.busy() || p.listed)
    }

    /// Analyses `file` in the background, keeping an edit list with the user's edits (a run resumed
    /// after they edited while it transcribed, 2j); a run already waiting or running is returned as
    /// it is.
    pub fn start(&self, file: &Path, notify: Notify) -> Result<Progress, String> {
        self.start_with(file, notify, true)
    }

    fn start_with(&self, file: &Path, notify: Notify, keep_list: bool) -> Result<Progress, String> {
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
            .spawn(move || me.run(&p, &notify, keep_list));
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
            // Asked (or nothing of the user's to lose): the list is re-cut.
            progress: Some(self.start_with(file, notify, false)?),
            hand_edited: false,
        })
    }

    /// Runs `f` behind the lane runs' first halves take (2j: tracks to edit list, the speakers
    /// stage among them): after the first half in progress, if any, and with none starting until
    /// it returns. For work that loads the speaker models outside a run (5c: a recording's
    /// speakers heard again). Not behind the transcription lane: like a first half, it is minutes,
    /// and Uezar's rule is that those never wait on another recording's transcription.
    pub fn exclusive<T>(&self, f: impl FnOnce() -> T) -> T {
        let _turn = lock(&self.first);
        f()
    }

    fn run(&self, p: &Project, notify: &Notify, keep_list: bool) {
        let k = p.file.clone();
        let result = self.halves(p, notify, keep_list);
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
                // A transcription that failed part way keeps the pieces it finished in the
                // editor (2j repair); one that failed before any is "none yet", not an older
                // run's transcript.
                if r.listed {
                    r.transcribed = r.transcribed.or(Some(0.0));
                }
            }
        });
    }

    /// The pipeline's first half on the `first` lane, then its transcription on the
    /// `transcription` lane, telling the page when the list is written and as pieces land.
    fn halves(&self, p: &Project, notify: &Notify, keep_list: bool) -> Result<(), String> {
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
        let o = self.options(p, keep_list)?;
        let second = {
            let _turn = lock(&self.first);
            self.update(&k, notify, Change::Run, |r| r.phase = Phase::Running);
            (self.halves)(p, &o, &mut log, &steps)?
        };
        // Taken at once when free, so a lone recording never shows as waiting.
        let (_turn, waited) = match self.transcription.try_lock() {
            Ok(turn) => (turn, false),
            Err(_) => {
                self.update(&k, notify, Change::Listed, |r| {
                    r.listed = true;
                    r.phase = Phase::Waiting;
                });
                (lock(&self.transcription), true)
            }
        };
        let change = if waited { Change::Run } else { Change::Listed };
        self.update(&k, notify, change, |r| {
            r.listed = true;
            r.phase = Phase::Running;
        });
        let mut transcribed = |t: f64| {
            self.update(&k, notify, Change::Transcribed, |r| r.transcribed = Some(t));
        };
        second(&mut log, &steps, &mut transcribed)
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

    /// Takes a track's step and tells `notify` only when its stage or whole percentage moved: at
    /// most a hundred events per track and stage however long the recording, where the speech
    /// stage reports every minute of audio. The project status is not read again (stage changes
    /// come through the log lines, [`Self::update`]).
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
        let notify: Notify = Arc::new(move |p: &Progress, _| {
            let _ = lock(&tx).send(p.clone());
        });
        // Hold the first lane so the run waits, queued, and a second request finds it.
        let held = lock(&a.first);
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

    /// Lets the fake transcriptions below end.
    static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// A pipeline whose first half is instant and whose transcription lands its first piece, then
    /// runs until [`RELEASE`]; recordings named `fail*` then fail.
    fn fake_halves<'a>(
        p: &'a Project,
        _: &'a PipelineOptions,
        _: &mut dyn FnMut(&str),
        _: Steps,
    ) -> Result<Second<'a>, String> {
        Ok(Box::new(move |_, _, transcribed| {
            transcribed(300.0);
            while !RELEASE.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let name = p.file.file_name().unwrap().to_string_lossy();
            if name.starts_with("fail") {
                Err("Whisper failed".into())
            } else {
                Ok(())
            }
        }))
    }

    fn wait_for(what: &str, ok: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !ok() {
            assert!(std::time::Instant::now() < deadline, "never: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_first_half_never_waits_behind_a_transcription() {
        let d = std::env::temp_dir().join(format!("junctur-app-lanes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (one, two) = (d.join("one.mp4"), d.join("fail-two.mp4"));
        for f in [&one, &two] {
            std::fs::write(f, b"not a video").unwrap();
        }
        let mut a = Analysis::with_cache(d.join("cache"));
        a.halves = fake_halves;
        let seen = Arc::new(Mutex::new(Vec::<Phase>::new()));
        let noted = seen.clone();
        let file = key(&two).display().to_string();
        let notify: Notify = Arc::new(move |p: &Progress, _| {
            if p.file == file {
                lock(&noted).push(p.phase);
            }
        });
        a.start(&one, notify.clone()).unwrap();
        wait_for("one transcribing", || {
            a.progress(&one)
                .is_some_and(|p| p.transcribed == Some(300.0))
        });
        assert!(a.editable(&one), "edited while it transcribes");
        // The second recording's first half runs at once; its transcription waits, editable.
        a.start(&two, notify).unwrap();
        wait_for("two waiting", || {
            a.progress(&two).is_some_and(|p| p.phase == Phase::Waiting)
        });
        let p = a.progress(&two).unwrap();
        assert!(p.listed && a.editable(&two));
        assert_eq!(a.progress(&one).unwrap().phase, Phase::Running);
        RELEASE.store(true, std::sync::atomic::Ordering::Relaxed);
        wait_for("both ended", || !a.busy(&one) && !a.busy(&two));
        let one = a.progress(&one).unwrap();
        assert_eq!((one.phase, one.transcribed), (Phase::Done, None));
        // Failed part way: the piece it finished stays the editor's.
        let two = a.progress(&two).unwrap();
        assert_eq!((two.phase, two.transcribed), (Phase::Failed, Some(300.0)));
        let phases = lock(&seen).clone();
        let at = |x| phases.iter().position(|p| *p == x).unwrap();
        assert!(at(Phase::Queued) < at(Phase::Waiting));
        assert!(phases[at(Phase::Waiting)..].contains(&Phase::Running));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn edits_wait_only_until_the_runs_list_is_written() {
        let a = Analysis::with_cache(std::env::temp_dir().join("junctur-app-editable-unused"));
        let k = PathBuf::from("rec.mp4");
        assert!(a.editable(&k), "no run");
        let run = |phase, listed| Progress {
            file: "rec.mp4".into(),
            phase,
            line: None,
            error: None,
            project: None,
            steps: Vec::new(),
            listed,
            transcribed: None,
        };
        for (phase, listed, editable) in [
            (Phase::Queued, false, false),
            (Phase::Running, false, false),
            (Phase::Waiting, true, true),
            (Phase::Running, true, true),
            (Phase::Done, true, true),
            (Phase::Failed, false, true),
        ] {
            lock(&a.runs).insert(key(&k), run(phase, listed));
            assert_eq!(a.editable(&k), editable, "{phase:?}, listed {listed}");
        }
    }

    #[test]
    fn steps_are_sent_when_a_track_moves_a_whole_percent_or_a_stage() {
        let a = Analysis::with_cache(std::env::temp_dir().join("junctur-app-steps-unused"));
        let k = PathBuf::from("rec.mp4");
        lock(&a.runs).insert(
            k.clone(),
            Progress {
                file: "rec.mp4".into(),
                phase: Phase::Running,
                line: None,
                error: None,
                project: None,
                steps: Vec::new(),
                listed: false,
                transcribed: None,
            },
        );
        let sent = Mutex::new(Vec::<Vec<Step>>::new());
        let notify = |p: &Progress, c| {
            assert_eq!(c, Change::Steps);
            lock(&sent).push(p.steps.clone());
        };
        let step = |stage, track, at| Step {
            stage,
            track,
            at,
            of: 1000.0,
        };
        a.step(&k, step("speech", 1, 0.0), &notify);
        a.step(&k, step("speech", 0, 0.0), &notify);
        // Under a whole percent: kept, not sent.
        a.step(&k, step("speech", 0, 9.0), &notify);
        a.step(&k, step("speech", 0, 10.0), &notify);
        // A new stage at the same percentage is sent.
        a.step(&k, step("transcribe", 0, 10.0), &notify);
        let sent = lock(&sent).clone();
        assert_eq!(sent.len(), 4);
        let last = sent.last().unwrap();
        assert_eq!(last.iter().map(|s| s.track).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(last[0], step("transcribe", 0, 10.0));
        // 1 % (10 s) is the third event.
        assert_eq!(sent[2][0].at, 10.0);
        // A file with no run takes nothing.
        a.step(
            &PathBuf::from("other.mp4"),
            step("speech", 0, 500.0),
            &|_, _| panic!("no run"),
        );
    }

    #[test]
    fn steps_from_two_threads_reach_notify_in_the_order_taken() {
        let a = Analysis::with_cache(std::env::temp_dir().join("junctur-app-order-unused"));
        let k = PathBuf::from("rec.mp4");
        lock(&a.runs).insert(
            k.clone(),
            Progress {
                file: "rec.mp4".into(),
                phase: Phase::Running,
                line: None,
                error: None,
                project: None,
                steps: Vec::new(),
                listed: false,
                transcribed: None,
            },
        );
        let sent = Mutex::new(Vec::<Vec<Step>>::new());
        let notify = |p: &Progress, _| {
            // Widen the gap between snapshot and send, where an unordered send would overtake.
            std::thread::yield_now();
            lock(&sent).push(p.steps.clone());
        };
        std::thread::scope(|scope| {
            for track in 0..2 {
                let (a, k, notify) = (&a, &k, &notify);
                scope.spawn(move || {
                    for at in 0..=1000 {
                        let s = Step {
                            stage: "speech",
                            track,
                            at: at as f64,
                            of: 1000.0,
                        };
                        a.step(k, s, notify);
                    }
                });
            }
        });
        let sent = lock(&sent).clone();
        for track in 0..2 {
            let ats: Vec<f64> = sent
                .iter()
                .filter_map(|v| v.iter().find(|s| s.track == track).map(|s| s.at))
                .collect();
            assert!(
                ats.windows(2).all(|w| w[0] <= w[1]),
                "track {track} went back"
            );
            assert_eq!(ats.last(), Some(&1000.0));
        }
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
            .reanalyse(&file, false, Arc::new(|_: &Progress, _| {}))
            .unwrap();
        assert!(r.hand_edited && r.progress.is_none());
        assert!(!a.busy(&file));
        // Told to replace it: started.
        let r = a
            .reanalyse(&file, true, Arc::new(|_: &Progress, _| {}))
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
