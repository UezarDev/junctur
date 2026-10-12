//! `jobs.json`: the state of each stage for one recording, written before and after every stage
//! so a killed run shows as `running` (interrupted) rather than vanishing.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Stage names, in run order.
pub const STAGES: [&str; 5] = ["tracks", "speech", "speakers", "transcribe", "edit"];

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

impl Jobs {
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join("jobs.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
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

    pub fn get(&self, name: &str) -> Option<&StageRecord> {
        self.stages.iter().find(|s| s.name == name)
    }

    pub fn set(
        &mut self,
        dir: &Path,
        name: &str,
        state: StageState,
        seconds: Option<f64>,
        note: Option<String>,
    ) -> Result<()> {
        if !self.stages.iter().any(|s| s.name == name) {
            // A stage added after this project's `jobs.json` was written (5a's `speakers`): put it
            // in run order, or its state would be dropped on every run.
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
        std::fs::write(dir.join("jobs.json"), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Run one stage, recording it as running, then done (with its time) or failed. The stage
    /// logs through the same `log`.
    pub fn stage<T>(
        &mut self,
        dir: &Path,
        name: &str,
        log: &mut dyn FnMut(&str),
        f: impl FnOnce(&mut dyn FnMut(&str)) -> Result<T>,
    ) -> Result<T> {
        log(&format!("stage {name} ..."));
        self.set(dir, name, StageState::Running, None, None)?;
        let t0 = Instant::now();
        match f(log) {
            Ok(v) => {
                let secs = t0.elapsed().as_secs_f64();
                self.set(dir, name, StageState::Done, Some(secs), None)?;
                log(&format!("stage {name} done in {secs:.1} s"));
                Ok(v)
            }
            Err(e) => {
                self.set(dir, name, StageState::Failed, None, Some(e.to_string()))?;
                Err(e)
            }
        }
    }

    /// A stage whose result was already there.
    pub fn skip(&mut self, dir: &Path, name: &str, log: &mut dyn FnMut(&str)) -> Result<()> {
        log(&format!("stage {name}: reusing the earlier result"));
        let state = StageState::Done;
        self.set(dir, name, state, None, Some("reused".into()))
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
            .stage(&dir, "tracks", &mut log, |_| {
                // Mid-stage, the file on disk says running.
                assert_eq!(
                    Jobs::load(&dir).get("tracks").unwrap().state,
                    StageState::Running
                );
                Ok(7)
            })
            .unwrap();
        assert_eq!(v, 7);
        let r: Result<()> = j.stage(&dir, "speech", &mut log, |_| {
            Err(crate::Error::Edit("boom".into()))
        });
        assert!(r.is_err());
        let back = Jobs::load(&dir);
        assert_eq!(back.get("tracks").unwrap().state, StageState::Done);
        assert_eq!(back.get("speech").unwrap().state, StageState::Failed);
        assert!(back
            .get("speech")
            .unwrap()
            .note
            .as_deref()
            .unwrap()
            .contains("boom"));
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
        std::fs::write(dir.join("jobs.json"), old).unwrap();
        let mut j = Jobs::load(&dir);
        j.set(&dir, "speakers", StageState::Done, Some(2.0), None)
            .unwrap();
        let back = Jobs::load(&dir);
        let names: Vec<&str> = back.stages.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, STAGES);
        assert_eq!(back.get("speakers").unwrap().state, StageState::Done);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
