//! A recording's Junctur project as the app shows it: the same folder `junctur status` reads
//! (`pipeline::Project::open` with the default cache), with each stage's state and whether the
//! transcript and edit list exist. The app runs the stages itself in `analysis`.

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
/// The lists the status is read for at once (the open recording, runs, the start window's rows),
/// and the text they may hold together: past either the cache starts over, and a list bigger than
/// that is parsed each time rather than held.
const PARSED_MAX: usize = 8;
const PARSED_BYTES: usize = 16 << 20;

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
    if text.len() > PARSED_BYTES {
        return s;
    }
    let mut held = lock(&PARSED);
    let m = held.get_or_insert_with(HashMap::new);
    m.remove(&path);
    let bytes: usize = m.values().map(|(t, _)| t.len()).sum();
    if m.len() >= PARSED_MAX || bytes + text.len() > PARSED_BYTES {
        m.clear();
    }
    m.insert(path, (text, s.clone()));
    s
}

/// A recent recording's line on the start window: how long it is and where its analysis
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
mod tests;
