//! Playing only what the edit list keeps: the kept ranges on the frame grid ([`timeline`]), the
//! mpv EDL that plays them gaplessly ([`edl`]), and the watch that reloads the edit list when it
//! changes on disk ([`watch`]). The player decides which form to load; this module holds the parts
//! that need no mpv, so they are tested without it.

pub mod edl;
pub mod timeline;
pub mod watch;

use std::path::Path;
use std::sync::Arc;

use junctur_core::{edit, ffmpeg};
use serde::Serialize;

use crate::project::EditSummary;
pub use timeline::{Fps, Timeline};

/// What a `loadfile` plays: the whole source, or only the kept ranges of a timeline (an EDL, whose
/// positions are output time).
#[derive(Debug, Clone, Default)]
pub enum Form {
    #[default]
    Source,
    Cuts(Arc<Timeline>),
}

/// One `loadfile`, queued until mpv's `start-file` says it began, so positions are always mapped
/// with the timeline the playing file was built from.
#[derive(Debug, Clone)]
pub struct Load {
    pub file: String,
    pub form: Form,
}

/// The edit list as last read from disk.
#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub timeline: Option<Arc<Timeline>>,
    pub summary: Option<EditSummary>,
    /// Why there is no timeline (or why the last change was not taken): unreadable, keeps nothing.
    pub error: Option<String>,
}

/// The frame rate the edit list is rounded to: the one FCPXML export probes.
pub fn probe_fps(file: &Path) -> Result<Fps, String> {
    Ok(ffmpeg::probe_video(file)
        .map_err(|e| e.to_string())?
        .map_or(Fps::AUDIO_ONLY, |v| Fps {
            num: v.fps_num,
            den: v.fps_den,
        }))
}

/// Reads `edit_path`. No file yet is not an error (the pipeline has not run); one that does not
/// parse, or keeps nothing, has no timeline and says why.
pub fn read(edit_path: &Path, fps: Fps) -> Edit {
    let text = match std::fs::read_to_string(edit_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Edit::default(),
        Err(e) => return failed(format!("{}: {e}", edit_path.display())),
    };
    let list = match edit::parse(&text) {
        Ok(l) => l,
        Err(e) => return failed(format!("{}: {e}", edit_path.display())),
    };
    let summary = Some(EditSummary {
        ranges: list.keep.len(),
        kept: list.kept(),
        duration: list.duration,
    });
    let t = Timeline::new(&list.keep, list.duration, fps);
    if t.is_empty() {
        return Edit {
            timeline: None,
            summary,
            error: Some("the edit list keeps nothing".into()),
        };
    }
    Edit {
        timeline: Some(Arc::new(t)),
        summary,
        error: None,
    }
}

fn failed(error: String) -> Edit {
    Edit {
        error: Some(error),
        ..Edit::default()
    }
}

/// What the frontend draws the timeline from (`edit-state`): sent when a file opens, the edit list
/// changes or skipping is toggled, not with every position.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditState {
    /// The user's choice; it takes effect only while there is a timeline.
    pub skip: bool,
    pub fps: Option<Fps>,
    /// Kept source frames, flattened `[start0, end0, start1, end1, ...]` (compact for thousands).
    pub ranges: Vec<u64>,
    pub total_frames: u64,
    pub output_frames: u64,
    pub summary: Option<EditSummary>,
    pub error: Option<String>,
}

impl EditState {
    pub fn new(edit: &Edit, skip: bool) -> Self {
        let t = edit.timeline.as_deref();
        Self {
            skip,
            fps: t.map(|t| t.fps),
            ranges: t.map_or_else(Vec::new, |t| {
                t.ranges.iter().flat_map(|&(a, b)| [a, b]).collect()
            }),
            total_frames: t.map_or(0, |t| t.total),
            output_frames: t.map_or(0, |t| t.fps.frame_at(t.output_duration())),
            summary: edit.summary.clone(),
            error: edit.error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use junctur_core::edit::{CutSettings, EditList, Range};

    const FPS: Fps = Fps { num: 30, den: 1 };

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("junctur-cuts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn list(keep: Vec<Range>) -> String {
        EditList {
            source: "rec.mp4".into(),
            duration: 10.0,
            settings: CutSettings::default(),
            keep,
            windows: vec![],
        }
        .to_text()
    }

    #[test]
    fn reads_an_edit_list() {
        let d = scratch("read");
        let p = d.join("edit.junctur");
        assert!(read(&p, FPS).timeline.is_none(), "no file yet");
        assert!(read(&p, FPS).error.is_none());

        std::fs::write(
            &p,
            list(vec![Range {
                start: 1.0,
                end: 2.0,
            }]),
        )
        .unwrap();
        let e = read(&p, FPS);
        assert_eq!(e.timeline.as_ref().unwrap().ranges, vec![(30, 60)]);
        let s = EditState::new(&e, true);
        assert_eq!(s.ranges, vec![30, 60]);
        assert_eq!((s.total_frames, s.output_frames), (300, 30));
        assert_eq!(s.summary.unwrap().ranges, 1);

        std::fs::write(&p, list(vec![])).unwrap();
        let e = read(&p, FPS);
        assert!(e.timeline.is_none());
        assert_eq!(e.error.as_deref(), Some("the edit list keeps nothing"));
        assert_eq!(e.summary.unwrap().ranges, 0);

        std::fs::write(&p, "garbage").unwrap();
        let e = read(&p, FPS);
        assert!(e.timeline.is_none() && e.error.is_some());
        let _ = std::fs::remove_dir_all(&d);
    }
}
