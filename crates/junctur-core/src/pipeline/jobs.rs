//! `jobs.json`: the state of each stage for one recording, written before and after every stage
//! so a killed run shows as `running` (interrupted) rather than vanishing.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::Result;

/// The file in a project folder.
pub(crate) const FILE: &str = "jobs.json";

/// The stages, in run order. The edit list comes before the transcript: it needs none of it but
/// the word counts, filled in once the transcript is whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageName {
    Tracks,
    Speech,
    Speakers,
    Edit,
    Transcribe,
}

impl StageName {
    pub const ALL: [StageName; 5] = [
        StageName::Tracks,
        StageName::Speech,
        StageName::Speakers,
        StageName::Edit,
        StageName::Transcribe,
    ];

    /// Its name in `jobs.json`, the log and the page.
    pub const fn name(self) -> &'static str {
        match self {
            StageName::Tracks => "tracks",
            StageName::Speech => "speech",
            StageName::Speakers => "speakers",
            StageName::Edit => "edit",
            StageName::Transcribe => "transcribe",
        }
    }
}

/// Stage names, in run order ([`StageName::ALL`]).
pub const STAGES: [&str; 5] = {
    let mut names = [""; 5];
    let mut i = 0;
    while i < names.len() {
        names[i] = StageName::ALL[i].name();
        i += 1;
    }
    names
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    Pending,
    /// Started and not finished: still running, or the run was killed.
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageRecord {
    /// A [`StageName`]'s name, or one a newer build wrote (kept, last).
    pub name: String,
    pub state: StageState,
    /// Wall time of the last run of this stage.
    pub seconds: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Jobs {
    pub file: PathBuf,
    pub stages: Vec<StageRecord>,
}

impl StageRecord {
    pub fn is(&self, stage: StageName) -> bool {
        self.name == stage.name()
    }
}

impl Jobs {
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Jobs>(&b).ok())
            .map(|mut j| {
                // An older file may list `transcribe` before `edit`: put in run order.
                // Stable, and names this version does not know go last.
                let order = |n: &str| STAGES.iter().position(|&x| x == n).unwrap_or(usize::MAX);
                j.stages.sort_by_key(|s| order(&s.name));
                j
            })
            .unwrap_or_else(|| Jobs {
                file: PathBuf::new(),
                stages: STAGES
                    .iter()
                    .map(|n| StageRecord {
                        name: n.to_string(),
                        state: StageState::Pending,
                        seconds: None,
                        note: None,
                    })
                    .collect(),
            })
    }

    pub fn get(&self, stage: StageName) -> Option<&StageRecord> {
        self.stages.iter().find(|s| s.is(stage))
    }

    /// Records `stage`'s state in `dir`'s `jobs.json`, and takes the file as it now is. Read,
    /// changed and written under one lock, never this copy whole: another holder's stale copy would
    /// put back a stage's old state over a newer one.
    pub fn set(
        &mut self,
        dir: &Path,
        stage: StageName,
        state: StageState,
        seconds: Option<f64>,
        note: Option<String>,
    ) -> Result<()> {
        let file = (!self.file.as_os_str().is_empty()).then(|| self.file.clone());
        *self = record(dir, file, stage.name(), state, seconds, note)?;
        Ok(())
    }

    fn put(&mut self, name: &str, state: StageState, seconds: Option<f64>, note: Option<String>) {
        if !self.stages.iter().any(|s| s.name == name) {
            // A stage added after this project's `jobs.json` was written: put it in run order, or
            // its state would be dropped on every run.
            let order = |n: &str| STAGES.iter().position(|&x| x == n);
            let at = self
                .stages
                .iter()
                .position(|s| order(&s.name) > order(name))
                .unwrap_or(self.stages.len());
            self.stages.insert(
                at,
                StageRecord {
                    name: name.to_string(),
                    state: StageState::Pending,
                    seconds: None,
                    note: None,
                },
            );
        }
        if let Some(s) = self.stages.iter_mut().find(|s| s.name == name) {
            s.state = state;
            if seconds.is_some() {
                s.seconds = seconds;
            }
            s.note = note;
        }
    }

    /// Run one stage, recording it as running, then done (with its time) or failed. The stage
    /// logs through the same `log`. A stage that panics is recorded failed on the way out,
    /// not left `running` as if the run had been killed.
    pub fn stage<T>(
        &mut self,
        dir: &Path,
        stage: StageName,
        log: &mut dyn FnMut(&str),
        f: impl FnOnce(&mut dyn FnMut(&str)) -> Result<T>,
    ) -> Result<T> {
        // Recorded before the line is logged, so a caller reading `jobs.json` on it sees the stage
        // running (the app's progress).
        let name = stage.name();
        self.set(dir, stage, StageState::Running, None, None)?;
        log(&format!("stage {name} ..."));
        let t0 = Instant::now();
        let unwinding = Unwinding { dir, name };
        let result = f(log);
        std::mem::forget(unwinding);
        match result {
            Ok(v) => {
                let secs = t0.elapsed().as_secs_f64();
                self.set(dir, stage, StageState::Done, Some(secs), None)?;
                log(&format!("stage {name} done in {secs:.1} s"));
                Ok(v)
            }
            Err(e) => {
                self.set(dir, stage, StageState::Failed, None, Some(e.to_string()))?;
                Err(e)
            }
        }
    }

    /// A stage whose result was already there.
    pub fn skip(&mut self, dir: &Path, stage: StageName, log: &mut dyn FnMut(&str)) -> Result<()> {
        log(&format!(
            "stage {}: reusing the earlier result",
            stage.name()
        ));
        let state = StageState::Done;
        self.set(dir, stage, state, None, Some("reused".into()))
    }
}

/// One change to `jobs.json` at a time in this process (see [`Jobs::set`]).
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Loads `dir`'s `jobs.json`, records one stage's state in it and writes it back whole, under
/// [`WRITING`]; returns what was written. `file` names the recording when the caller knows it.
fn record(
    dir: &Path,
    file: Option<PathBuf>,
    name: &str,
    state: StageState,
    seconds: Option<f64>,
    note: Option<String>,
) -> Result<Jobs> {
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut now = Jobs::load(dir);
    if let Some(f) = file {
        now.file = f;
    }
    now.put(name, state, seconds, note);
    // Whole or not at all: a half-written file would read back as every stage pending.
    crate::output::write_replacing(&dir.join(FILE), serde_json::to_vec_pretty(&now)?)?;
    Ok(now)
}

/// Armed while a stage runs; dropped only when the stage unwinds (a panic), when it records the
/// stage failed ([`Jobs::stage`]).
struct Unwinding<'a> {
    dir: &'a Path,
    name: &'a str,
}

impl Drop for Unwinding<'_> {
    fn drop(&mut self) {
        let note = Some("stopped on an internal error".to_string());
        let _ = record(self.dir, None, self.name, StageState::Failed, None, note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_running_done_and_failed() {
        let dir = std::env::temp_dir().join(format!("junctur-jobs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut j = Jobs::load(&dir);
        assert!(j.stages.iter().all(|s| s.state == StageState::Pending));
        let mut log = |_: &str| {};
        let v = j
            .stage(&dir, StageName::Tracks, &mut log, |_| {
                // Mid-stage, the file on disk says running.
                assert_eq!(
                    Jobs::load(&dir).get(StageName::Tracks).unwrap().state,
                    StageState::Running
                );
                Ok(7)
            })
            .unwrap();
        assert_eq!(v, 7);
        let r: Result<()> = j.stage(&dir, StageName::Speech, &mut log, |_| {
            Err(crate::Error::Edit("boom".into()))
        });
        assert!(r.is_err());
        let back = Jobs::load(&dir);
        assert_eq!(back.get(StageName::Tracks).unwrap().state, StageState::Done);
        assert_eq!(
            back.get(StageName::Speech).unwrap().state,
            StageState::Failed
        );
        assert!(back
            .get(StageName::Speech)
            .unwrap()
            .note
            .as_deref()
            .unwrap()
            .contains("boom"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_copies_never_write_over_each_others_stages() {
        let dir = std::env::temp_dir().join(format!("junctur-jobs-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A run's second half and the speakers heard again, each loaded while transcribing.
        let mut run = Jobs::load(&dir);
        run.set(&dir, StageName::Transcribe, StageState::Running, None, None)
            .unwrap();
        let mut rehear = Jobs::load(&dir);
        run.set(
            &dir,
            StageName::Transcribe,
            StageState::Done,
            Some(1.0),
            None,
        )
        .unwrap();
        rehear
            .set(&dir, StageName::Speakers, StageState::Done, Some(2.0), None)
            .unwrap();
        let back = Jobs::load(&dir);
        assert_eq!(
            back.get(StageName::Transcribe).unwrap().state,
            StageState::Done
        );
        assert_eq!(
            back.get(StageName::Speakers).unwrap().state,
            StageState::Done
        );
        assert_eq!(rehear, back, "a copy takes the file as written");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stage_that_panics_is_recorded_failed() {
        let dir = std::env::temp_dir().join(format!("junctur-jobs-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let caught = std::panic::catch_unwind(|| {
            let mut j = Jobs::load(&dir);
            let _: Result<()> = j.stage(&dir, StageName::Speech, &mut |_| {}, |_| panic!("boom"));
        });
        assert!(caught.is_err());
        assert_eq!(
            Jobs::load(&dir).get(StageName::Speech).unwrap().state,
            StageState::Failed
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stage_an_older_file_does_not_list_is_added_in_run_order() {
        let dir = std::env::temp_dir().join(format!("junctur-jobs-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = r#"{"file":"x.mp4","stages":[
            {"name":"tracks","state":"done","seconds":1.0,"note":null},
            {"name":"speech","state":"done","seconds":1.0,"note":null},
            {"name":"transcribe","state":"done","seconds":1.0,"note":null},
            {"name":"edit","state":"done","seconds":1.0,"note":null}]}"#;
        std::fs::write(dir.join(FILE), old).unwrap();
        let mut j = Jobs::load(&dir);
        j.set(&dir, StageName::Speakers, StageState::Done, Some(2.0), None)
            .unwrap();
        let back = Jobs::load(&dir);
        // Also put in run order: the file had `transcribe` before `edit`.
        let names: Vec<&str> = back.stages.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, STAGES);
        assert_eq!(
            back.get(StageName::Speakers).unwrap().state,
            StageState::Done
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The names are `jobs.json`'s and, serialised, the page's progress steps'.
    #[test]
    fn stage_names_are_the_files_and_the_pages() {
        assert_eq!(
            STAGES,
            ["tracks", "speech", "speakers", "edit", "transcribe"]
        );
        for s in StageName::ALL {
            assert_eq!(
                serde_json::to_string(&s).unwrap(),
                format!("\"{}\"", s.name())
            );
        }
    }
}
