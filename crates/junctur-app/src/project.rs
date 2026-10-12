//! A recording's Junctur project as the app shows it: the same folder `junctur status` reads
//! (`pipeline::Project::open` with the default cache), with each stage's state and whether the
//! transcript and edit list exist. Running the stages from the app comes later.

use std::path::{Path, PathBuf};

use junctur_core::pipeline::{Project, StageState, STAGES};
use junctur_core::transcribe;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub dir: String,
    pub stages: Vec<Stage>,
    pub transcript: bool,
    pub edit: Option<EditSummary>,
    /// The edit list exists but does not parse (hand-edited, or a newer format).
    pub edit_error: Option<String>,
    /// Where the edit list is (or will be), for the player to read and watch.
    #[serde(skip)]
    pub edit_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stage {
    pub name: String,
    /// `pending`, `running` (or interrupted), `done` or `failed`, as `jobs.json` spells them.
    pub state: StageState,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSummary {
    pub ranges: usize,
    pub kept: f64,
    pub duration: f64,
}

/// Opens (creating its folder if needed, as the CLI does) the project for `file`. `cache_dir`
/// `None` is the CLI's default cache.
pub fn status(file: &Path, cache_dir: Option<PathBuf>) -> Result<ProjectStatus, String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let jobs = p.jobs();
    let stages = STAGES
        .iter()
        .map(|&name| Stage {
            name: name.to_string(),
            state: jobs.get(name).map_or(StageState::Pending, |r| r.state),
        })
        .collect();
    let (edit, edit_error) = if p.edit_path().is_file() {
        match p.edit() {
            Ok(e) => (
                Some(EditSummary {
                    ranges: e.keep.len(),
                    kept: e.kept(),
                    duration: e.duration,
                }),
                None,
            ),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };
    Ok(ProjectStatus {
        dir: p.dir.display().to_string(),
        stages,
        transcript: transcribe::path(&p.dir).is_file(),
        edit,
        edit_error,
        edit_path: p.edit_path(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use junctur_core::edit::{CutSettings, EditList, Range};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("junctur-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn new_project_has_nothing_yet() {
        let d = scratch("new");
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let s = status(&file, Some(d.join("cache"))).unwrap();
        assert_eq!(s.stages.len(), STAGES.len());
        assert!(s.stages.iter().all(|st| st.state == StageState::Pending));
        assert!(!s.transcript);
        assert_eq!((s.edit, s.edit_error), (None, None));
        assert!(Path::new(&s.dir).is_dir());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reports_edit_list_and_transcript() {
        let d = scratch("edit");
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
        let e = EditList {
            source: p.file.clone(),
            duration: 60.0,
            settings: CutSettings::default(),
            keep: vec![
                Range {
                    start: 0.0,
                    end: 10.0,
                },
                Range {
                    start: 20.0,
                    end: 25.0,
                },
            ],
            speakers: Default::default(),
            windows: vec![],
        };
        std::fs::write(p.edit_path(), e.to_text()).unwrap();
        std::fs::write(transcribe::path(&p.dir), b"{}").unwrap();
        let s = status(&file, Some(d.join("cache"))).unwrap();
        assert!(s.transcript);
        let summary = s.edit.unwrap();
        assert_eq!(summary.ranges, 2);
        assert!((summary.kept - 15.0).abs() < 1e-9);
        assert_eq!(summary.duration, 60.0);

        std::fs::write(p.edit_path(), "garbage").unwrap();
        let s = status(&file, Some(d.join("cache"))).unwrap();
        assert_eq!(s.edit, None);
        assert!(s.edit_error.is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_is_an_error() {
        let d = scratch("missing");
        assert!(status(&d.join("nope.mp4"), Some(d.join("cache"))).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
