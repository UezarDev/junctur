//! Playing only what the edit list keeps: the kept ranges on the frame grid ([`timeline`]), the
//! mpv EDL that plays them gaplessly ([`edl`]), and the watch that reloads the edit list when it
//! changes on disk ([`watch`]). The player decides which form to load; this module holds the parts
//! that need no mpv, so they are tested without it.

pub mod edl;
pub mod timeline;
pub mod watch;

use std::path::Path;
use std::sync::Arc;

use junctur_core::edit::{self, EditList};
use junctur_core::ffmpeg;
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

/// One `loadfile`, kept until mpv's `start-file` names its playlist entry, so positions are always
/// mapped with the timeline the playing file was built from.
#[derive(Debug, Clone)]
pub struct Load {
    /// mpv's playlist entry id for this load.
    pub entry: i64,
    pub file: String,
    pub form: Form,
}

/// The edit list as last read from disk.
#[derive(Debug, Clone, Default)]
pub struct Edit {
    /// The list as parsed: what the editor edits.
    pub list: Option<Arc<EditList>>,
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

/// Reads the edit list of `file` from `edit_path`, probing the frame rate when `fps` is not known
/// yet (a process start, so callers do this outside their locks). `None` when there is no edit
/// list; one that cannot be read, does not parse or keeps nothing has no timeline and says why.
pub fn fetch(file: &str, edit_path: &Path, fps: Option<Fps>) -> (Option<Fps>, Option<Edit>) {
    if !edit_path.is_file() {
        return (fps, None);
    }
    let fps = match fps.map_or_else(|| probe_fps(file.as_ref()), Ok) {
        Ok(f) => f,
        Err(e) => return (None, Some(failed(format!("frame rate: {e}")))),
    };
    (Some(fps), read(edit_path, fps))
}

/// Reads `edit_path`; `None` when it does not exist.
pub fn read(edit_path: &Path, fps: Fps) -> Option<Edit> {
    let text = match std::fs::read_to_string(edit_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(failed(format!("{}: {e}", edit_path.display()))),
    };
    Some(parse(&text, edit_path, fps))
}

fn parse(text: &str, edit_path: &Path, fps: Fps) -> Edit {
    match edit::parse(text) {
        Ok(l) => from_list(l, fps),
        Err(e) => failed(format!("{}: {e}", edit_path.display())),
    }
}

/// An edit list that parsed (read from disk, or made by the editor) on the frame grid.
pub fn from_list(list: EditList, fps: Fps) -> Edit {
    let summary = Some(EditSummary {
        ranges: list.keep.len(),
        kept: list.kept(),
        duration: list.duration,
    });
    let t = Timeline::new(&list.keep, list.duration, fps);
    let list = Some(Arc::new(list));
    if t.is_empty() {
        return Edit {
            list,
            timeline: None,
            summary,
            error: Some("the edit list keeps nothing".into()),
        };
    }
    Edit {
        list,
        timeline: Some(Arc::new(t)),
        summary,
        error: None,
    }
}

/// What a re-read of the edit list leaves playing, given what played before. Ranges in play are
/// kept through a file that is missing (deleted and about to be rewritten) or does not parse yet
/// (a hand edit half-typed): only a list that parses replaces them. A list with the same frames
/// keeps the same timeline, so nothing reloads for a change of scores or comments.
pub fn merge(old: &Edit, new: Option<Edit>) -> Edit {
    let Some(old_t) = &old.timeline else {
        return new.unwrap_or_default();
    };
    match new {
        None => Edit {
            error: Some("the edit list is missing; still playing its last ranges".into()),
            ..old.clone()
        },
        Some(n) if n.summary.is_none() => Edit {
            error: n.error,
            ..old.clone()
        },
        Some(n) if n.timeline.as_deref() == Some(old_t) => Edit {
            timeline: Some(old_t.clone()),
            ..n
        },
        Some(n) => n,
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
    /// The frame rate the ranges are counted in, once known (also for a list keeping nothing).
    pub fps: Option<Fps>,
    /// Kept source frames, flattened `[start0, end0, start1, end1, ...]` (compact for thousands).
    pub ranges: Vec<u64>,
    pub total_frames: u64,
    pub output_frames: u64,
    pub summary: Option<EditSummary>,
    pub error: Option<String>,
    /// There is a parsed list the editor can change.
    pub editable: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// What the last edit or outside change did that the user should know (a backup made, an
    /// unreadable file replaced, cuts placed without the envelope).
    pub notice: Option<String>,
    /// The speakers the list names, by id.
    pub speakers: Vec<SpeakerView>,
    /// Changes when the list's turns (or speaker tracks) do, so the page asks for the words'
    /// labels again after an outside change; a rename leaves it as it was.
    pub speakers_key: String,
    /// Changes when the list's fixed or retimed words do, so the page asks for the words' text and
    /// times again, after an edit of its own or an outside change.
    pub fixes_key: String,
    /// The caption settings: the list's own, else the user's defaults (also with no list).
    pub captions: edit::Captions,
    /// The list has caption settings of its own; without, it follows the defaults.
    pub captions_own: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerView {
    pub id: u32,
    pub name: String,
    pub color: String,
    /// Their audio track (0-based) when speakers were told apart by track: the lanes colour it
    /// and the avatars take its level.
    pub track: Option<usize>,
}

/// The editor's side of the edit state: what undo and redo can do, and its last notice.
#[derive(Debug, Clone, Default)]
pub struct EditorStatus {
    pub can_undo: bool,
    pub can_redo: bool,
    pub notice: Option<String>,
}

impl EditState {
    /// `defaults`: the user's caption defaults, for a list without settings of its own.
    pub fn new(
        edit: &Edit,
        skip: bool,
        fps: Option<Fps>,
        status: &EditorStatus,
        defaults: &edit::Captions,
    ) -> Self {
        let own = edit.list.as_ref().and_then(|l| l.captions.clone());
        let t = edit.timeline.as_deref();
        Self {
            skip,
            fps: t.map(|t| t.fps).or(fps),
            ranges: t.map_or_else(Vec::new, |t| {
                t.ranges.iter().flat_map(|&(a, b)| [a, b]).collect()
            }),
            total_frames: t.map_or(0, |t| t.total),
            output_frames: t.map_or(0, |t| t.output_frames),
            summary: edit.summary.clone(),
            error: edit.error.clone(),
            editable: edit.list.is_some() && fps.is_some(),
            can_undo: status.can_undo,
            can_redo: status.can_redo,
            notice: status.notice.clone(),
            speakers: edit.list.as_ref().map_or_else(Vec::new, |l| {
                l.speakers
                    .names
                    .iter()
                    .map(|n| SpeakerView {
                        id: n.id,
                        name: n.name.clone(),
                        color: n.color.clone(),
                        track: n.track,
                    })
                    .collect()
            }),
            speakers_key: edit
                .list
                .as_ref()
                .map_or_else(String::new, |l| speakers_key(&l.speakers)),
            fixes_key: edit
                .list
                .as_ref()
                .map_or_else(String::new, |l| fixes_key(&l.fixes, &l.times)),
            captions: own.clone().unwrap_or_else(|| defaults.clone()),
            captions_own: own.is_some(),
        }
    }
}

/// FNV-1a over 64-bit values, as a string, since a 64-bit number does not survive JSON into
/// JavaScript exactly.
fn key(values: impl IntoIterator<Item = u64>) -> String {
    let mut h = junctur_core::hash::Fnv::new();
    for x in values {
        h.u64(x);
    }
    h.hex()
}

/// Over the turns and speaker tracks (not the names): what the words' labels depend on.
fn speakers_key(s: &edit::Speakers) -> String {
    let names = s
        .names
        .iter()
        .flat_map(|n| [n.id as u64, n.track.map_or(u64::MAX, |t| t as u64)]);
    let turns = s
        .turns
        .iter()
        .flat_map(|t| [t.start.to_bits(), t.end.to_bits(), t.speaker as u64]);
    key(names.chain(turns))
}

/// Over the fixed words, spans and retimed words: what the words' text and times
/// depend on beside the transcript.
fn fixes_key(fixes: &[edit::Fix], times: &[edit::Retime]) -> String {
    let times = times.iter().flat_map(|t| {
        [
            t.track as u64,
            t.start.to_bits(),
            t.nth as u64,
            t.from.to_bits(),
            t.to.to_bits(),
            u64::MAX - 1,
        ]
    });
    key(fixes
        .iter()
        .flat_map(|f| {
            let through = f
                .through
                .map_or([u64::MAX, u64::MAX], |t| [t.start.to_bits(), t.nth as u64]);
            [f.track as u64, f.start.to_bits(), f.nth as u64]
                .into_iter()
                .chain(through)
                .chain(f.text.bytes().map(u64::from))
                .chain([u64::MAX])
        })
        .chain(times))
}

#[cfg(test)]
mod tests;
