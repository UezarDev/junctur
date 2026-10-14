//! A recording's Junctur project as the app shows it: the same folder `junctur status` reads
//! (`pipeline::Project::open` with the default cache), with each stage's state and whether the
//! transcript and edit list exist. The app runs the stages itself in `analysis` (2d).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use junctur_core::ffmpeg;
use junctur_core::pipeline::{self, Project, StageState, STAGES};
use junctur_core::transcribe;
use serde::Serialize;

use crate::lock::lock;

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
    Ok(status_of(&p))
}

fn status_of(p: &Project) -> ProjectStatus {
    let jobs = p.jobs();
    let stages = STAGES
        .iter()
        .map(|&name| Stage {
            name: name.to_string(),
            state: jobs.get(name).map_or(StageState::Pending, |r| r.state),
        })
        .collect();
    let (edit, edit_error) = if p.edit_path().is_file() {
        match summary(p) {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(e)),
        }
    } else {
        (None, None)
    };
    ProjectStatus {
        dir: p.dir.display().to_string(),
        stages,
        transcript: transcribe::path(&p.dir).is_file(),
        edit,
        edit_error,
        edit_path: p.edit_path(),
    }
}

/// Each edit list's text as last parsed here and what it said: the status is read on every line a
/// run logs, and parsing a long recording's list each time is the cost, not reading it.
type Parsed = HashMap<PathBuf, (String, Result<EditSummary, String>)>;
static PARSED: Mutex<Option<Parsed>> = Mutex::new(None);
/// More lists than a session opens: past it the cache starts over rather than grow.
const PARSED_MAX: usize = 64;

/// `p`'s edit list summed up, or why it does not read, as `p.edit()` says it; parsed again only when
/// its text is not the text parsed last.
fn summary(p: &Project) -> Result<EditSummary, String> {
    let path = p.edit_path();
    let text =
        std::fs::read_to_string(&path).map_err(|e| junctur_core::Error::from(e).to_string())?;
    if let Some((t, s)) = lock(&PARSED).as_ref().and_then(|m| m.get(&path)) {
        if *t == text {
            return s.clone();
        }
    }
    let s = p
        .parse_edit(&text)
        .map(|e| EditSummary {
            ranges: e.keep.len(),
            kept: e.kept(),
            duration: e.duration,
        })
        .map_err(|e| e.to_string());
    let mut held = lock(&PARSED);
    let m = held.get_or_insert_with(HashMap::new);
    if m.len() >= PARSED_MAX && !m.contains_key(&path) {
        m.clear();
    }
    m.insert(path, (text, s.clone()));
    s
}

/// A recent recording's line on the start window (2g's S): how long it is and where its analysis
/// stands. Asked per row after the list is drawn, so the list never waits on its projects.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingInfo {
    pub duration: Option<f64>,
    /// The edit list's kept ranges, when it has one that parses.
    pub ranges: Option<usize>,
    /// Stages done, of [`STAGES`].
    pub stages_done: usize,
    pub stages: usize,
}

/// [`RecordingInfo`] of `file`: its length from the edit list, else the track analysis's cache,
/// else ffprobe (only for a recording never analysed this far).
pub fn info(file: &Path, cache_dir: Option<PathBuf>) -> Result<RecordingInfo, String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let s = status_of(&p);
    let done = s
        .stages
        .iter()
        .filter(|st| st.state == StageState::Done)
        .count();
    let duration = match &s.edit {
        Some(e) => Some(e.duration),
        None => pipeline::cached_tracks(&p)
            .map(|r| r.duration)
            .or_else(|| ffmpeg::probe(file).ok().map(|p| p.duration)),
    };
    Ok(RecordingInfo {
        duration: duration.filter(|d| *d > 0.0),
        ranges: s.edit.as_ref().map(|e| e.ranges),
        stages_done: done,
        stages: s.stages.len(),
    })
}

/// The project folder's still of `file` (`thumb.jpg`, 320 px wide): made once by ffmpeg from 10% in
/// (at most a minute, so a 30 h recording does not seek half an hour), then read from there.
pub fn thumbnail(file: &Path, cache_dir: Option<PathBuf>) -> Result<Vec<u8>, String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let path = p.dir.join(THUMBNAIL);
    if let Ok(b) = std::fs::read(&path) {
        return Ok(b);
    }
    let at = ffmpeg::probe(file).map_or(0.0, |x| (x.duration * 0.1).min(60.0));
    // Written beside it and renamed, so a half-written still is never taken for the real one; a
    // name of its own per call, as two asks for one recording may run at once.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = p.dir.join(format!("thumb.{}.{n}.jpg", std::process::id()));
    let args: Vec<String> = [
        "-v",
        "error",
        "-nostdin",
        "-y",
        "-ss",
        &format!("{at:.3}"),
        "-i",
        &file.to_string_lossy(),
        "-frames:v",
        "1",
        "-vf",
        "scale=320:-2",
        "-q:v",
        "5",
        &tmp.to_string_lossy(),
    ]
    .map(String::from)
    .to_vec();
    let made = ffmpeg::ffmpeg(&args)
        .map_err(|e| e.to_string())
        .and_then(|()| std::fs::rename(&tmp, &path).map_err(|e| e.to_string()));
    if let Err(e) = made {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::read(&path).map_err(|e| e.to_string())
}

const THUMBNAIL: &str = "thumb.jpg";

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
            captions: Default::default(),
            fixes: Vec::new(),
            times: Vec::new(),
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
    fn a_kept_parse_says_what_parsing_the_list_again_says() {
        let d = scratch("parsed");
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
        let list = |ends: &[f64]| EditList {
            source: p.file.clone(),
            duration: 90.0,
            settings: CutSettings::default(),
            keep: ends
                .iter()
                .map(|&end| Range {
                    start: end - 5.0,
                    end,
                })
                .collect(),
            speakers: Default::default(),
            captions: Default::default(),
            fixes: Vec::new(),
            times: Vec::new(),
            windows: vec![],
        };
        // What the status said before the parse was kept: the list read and parsed afresh.
        let afresh = || match p.edit() {
            Ok(e) => (
                Some(EditSummary {
                    ranges: e.keep.len(),
                    kept: e.kept(),
                    duration: e.duration,
                }),
                None,
            ),
            Err(e) => (None, Some(e.to_string())),
        };
        let texts = [
            list(&[10.0, 30.0]).to_text(),
            list(&[10.0, 30.0]).to_text(),
            // One digit apart: the same length, another summary.
            list(&[10.0, 40.0]).to_text(),
            "garbage".to_string(),
            "junctur-edit 999\n".to_string(),
            list(&[10.0, 30.0, 50.0]).to_text(),
            list(&[10.0, 30.0]).to_text(),
        ];
        for text in &texts {
            std::fs::write(p.edit_path(), text).unwrap();
            for _ in 0..2 {
                let s = status(&file, Some(d.join("cache"))).unwrap();
                assert_eq!((s.edit, s.edit_error), afresh(), "{text}");
            }
        }
        // Not UTF-8: refused as reading it refuses it.
        std::fs::write(p.edit_path(), [0xff, 0xfe, 0x00]).unwrap();
        let s = status(&file, Some(d.join("cache"))).unwrap();
        assert_eq!((s.edit, s.edit_error.clone()), afresh());
        assert!(s.edit_error.is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_recordings_info_and_still() {
        let d = scratch("info");
        let file = d.join("rec.mkv");
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=d=3:s=640x360:r=25",
            ])
            .args(["-f", "lavfi", "-i", "sine=d=3", "-shortest"])
            .arg(&file)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let cache = Some(d.join("cache"));
        let i = info(&file, cache.clone()).unwrap();
        assert_eq!((i.ranges, i.stages_done, i.stages), (None, 0, STAGES.len()));
        assert!(
            (i.duration.unwrap() - 3.0).abs() < 0.1,
            "from ffprobe: {i:?}"
        );
        let jpg = thumbnail(&file, cache.clone()).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8], "a JPEG");
        let dir = PathBuf::from(status(&file, cache.clone()).unwrap().dir);
        assert!(dir.join(THUMBNAIL).is_file());
        // Made once: what is there is what comes back.
        std::fs::write(dir.join(THUMBNAIL), b"kept").unwrap();
        assert_eq!(thumbnail(&file, cache.clone()).unwrap(), b"kept");
        assert!(thumbnail(&d.join("nope.mkv"), cache).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_is_an_error() {
        let d = scratch("missing");
        assert!(status(&d.join("nope.mp4"), Some(d.join("cache"))).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
