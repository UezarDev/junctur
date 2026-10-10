//! `jobs.json`: the state of each stage for one recording, written before and after every stage
//! so a killed run shows as `running` (interrupted) rather than vanishing.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Stage names, in run order.
pub const STAGES: [&str; 4] = ["tracks", "speech", "transcribe", "edit"];

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

    /// Run one stage, recording it as running, then done (with its time) or failed.
    pub fn stage<T>(
        &mut self,
        dir: &Path,
        name: &str,
        log: &mut dyn FnMut(&str),
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        log(&format!("stage {name} ..."));
        self.set(dir, name, StageState::Running, None, None)?;
        let t0 = Instant::now();
        match f() {
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
            .stage(&dir, "tracks", &mut log, || {
                // Mid-stage, the file on disk says running.
                assert_eq!(
                    Jobs::load(&dir).get("tracks").unwrap().state,
                    StageState::Running
                );
                Ok(7)
            })
            .unwrap();
        assert_eq!(v, 7);
        let r: Result<()> = j.stage(&dir, "speech", &mut log, || {
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
}
