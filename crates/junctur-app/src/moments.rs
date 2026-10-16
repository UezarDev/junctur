//! Moments in the app (phase 7a, a prototype): what the Moments panel and lane show of a recording's
//! voice and hotkey markers ([`junctur_core::moments`]), and the user's Accept, Reject and Undo turned
//! into edits of the list in play. A module because the view gathers from four files (the transcript,
//! the markers file, the edit list and the answers) and the answers edit through the player.

use std::path::{Path, PathBuf};

use junctur_core::edit::{frames_ceil, frames_floor};
use junctur_core::moments::{self, How, Moment};
use junctur_core::pipeline::Project;
use junctur_core::transcribe::{self, Segment};
use serde::{Deserialize, Serialize};

use crate::player::Core;

/// Where a moment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Suggested,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MomentView {
    #[serde(flatten)]
    pub moment: Moment,
    pub state: State,
}

/// What the page shows of a recording's moments.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MomentsView {
    pub moments: Vec<MomentView>,
    /// There are words to look for phrases in (a transcript, or its first pieces).
    pub heard: bool,
    /// The timestamp file read, and how many of its lines did not read; `markersError` when it is
    /// there and could not be read at all.
    pub markers_file: Option<String>,
    pub markers_bad: usize,
    pub markers_error: Option<String>,
    /// Where a timestamp file would be read from, for the panel to say.
    pub markers_hint: String,
    pub phrases: Vec<&'static str>,
}

/// Every word of the project's transcript (or of the run's chunks so far) with its track, in time
/// order.
fn segments(dir: &Path) -> Option<Vec<Segment>> {
    transcribe::load(dir)
        .map(|t| t.segments)
        .or_else(|| transcribe::so_far(dir))
}

/// The moments of `file` and the answers they were matched against, with the view's other fields.
fn gather(p: &Project) -> Result<(Vec<Moment>, moments::Answers, MomentsView), String> {
    let mut v = MomentsView {
        phrases: moments::PHRASES.to_vec(),
        markers_hint: format!(
            "{} beside the recording",
            p.file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
                + ".markers.txt"
        ),
        ..Default::default()
    };
    let duration = p.edit().map(|l| l.duration).unwrap_or(f64::INFINITY);
    let mut markers = Vec::new();
    if let Some(segs) = segments(&p.dir) {
        let mut words: Vec<(usize, &transcribe::Word)> = segs
            .iter()
            .flat_map(|s| s.words.iter().map(move |w| (s.track, w)))
            .collect();
        words.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
        v.heard = !words.is_empty();
        markers.extend(moments::voice_markers(&words));
    }
    if let Some(path) = moments::markers_file(&p.file, &p.dir) {
        v.markers_file = Some(path.display().to_string());
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let (m, bad) = moments::hotkey_markers(&text);
                markers.extend(m);
                v.markers_bad = bad;
            }
            Err(e) => v.markers_error = Some(e.to_string()),
        }
    }
    let answers = moments::load_answers(&p.dir).map_err(|e| e.to_string())?;
    Ok((moments::moments(markers, duration), answers, v))
}

/// What `file`'s moments are now: read again after every edit and answer.
pub fn view(file: &Path, cache_dir: Option<PathBuf>) -> Result<MomentsView, String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let (all, answers, mut v) = gather(&p)?;
    v.moments = all
        .into_iter()
        .map(|moment| {
            let state = match answers.of(&moment).map(|a| a.how) {
                Some(How::Accepted) => State::Accepted,
                Some(How::Rejected) => State::Rejected,
                None => State::Suggested,
            };
            MomentView { moment, state }
        })
        .collect();
    Ok(v)
}

/// What the user does with a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Keep its clip and cut its phrases, as one undo step.
    Accept,
    /// Turn it down: no edit.
    Reject,
    /// Put an accepted one's span back as the list had it, or a rejected one waiting again.
    Undo,
    /// Every waiting moment accepted, as one undo step.
    AcceptAll,
}

/// Applies `action` to the moment named by `at` (its [`Moment::at`]; none for
/// [`Action::AcceptAll`]) on the open recording `file`, as `core` plays it: the edits first, as one
/// undo step, then the answers, so a refused edit records nothing.
pub fn answer(
    core: &Core,
    file: Option<&str>,
    cache_dir: Option<PathBuf>,
    action: Action,
    at: Option<f64>,
) -> Result<(), String> {
    let path = file.ok_or("no recording is open")?;
    let p = Project::open(Path::new(path), cache_dir, None).map_err(|e| e.to_string())?;
    let (all, mut answers, _) = gather(&p)?;
    let list = p.edit().map_err(|e| e.to_string())?;
    let now = list.keep.as_slice();
    let named = || {
        all.iter()
            .find(|m| at.is_some_and(|t| (m.at - t).abs() < 1e-3))
            .ok_or("that moment is no longer there")
    };
    let edits = match action {
        Action::Accept => {
            let m = named()?;
            if answers.of(m).is_some() {
                return Err("that moment was answered already".into());
            }
            moments::accept(&[m], &all, now, &mut answers)
        }
        Action::Reject => {
            let m = named()?;
            moments::reject(m, &mut answers);
            Vec::new()
        }
        Action::Undo => {
            let m = named()?;
            match answers.of(m).map(|a| a.how) {
                Some(How::Accepted) => moments::undo(m, &mut answers),
                Some(How::Rejected) => {
                    answers.forget(m);
                    Vec::new()
                }
                None => return Err("that moment has no answer to undo".into()),
            }
        }
        Action::AcceptAll => {
            let waiting: Vec<&Moment> = all.iter().filter(|m| answers.of(m).is_none()).collect();
            moments::accept(&waiting, &all, now, &mut answers)
        }
    };
    if !edits.is_empty() {
        let state = core.edit_state();
        let fps = state.fps.ok_or("the frame rate is not known")?;
        let total = state.total_frames;
        let frames: Vec<(u64, u64, bool)> = edits
            .iter()
            .map(|&(a, b, keep)| {
                let a = frames_floor(a, fps.num, fps.den).min(total);
                let b = frames_ceil(b, fps.num, fps.den).min(total);
                (a, b, keep)
            })
            .filter(|(a, b, _)| b > a)
            .collect();
        core.edit_frames_all(file, &frames)?;
    }
    moments::save_answers(&p.dir, &answers).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_reads_phrases_a_markers_file_and_answers() {
        let dir = std::env::temp_dir().join(format!("junctur-app-moments-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("rec.mkv");
        std::fs::write(&source, b"").unwrap();
        let cache = dir.join("cache");
        let p = Project::open(&source, Some(cache.clone()), None).unwrap();
        // A finished transcript with "clip that" at 100 s.
        let word = |start: f64, text: &str| transcribe::Word {
            start,
            end: start + 0.3,
            text: text.into(),
            p: 0.9,
            p_min: None,
        };
        let seg = Segment {
            track: 0,
            start: 99.0,
            end: 101.0,
            lang: "en".into(),
            model: "m".into(),
            words: vec![word(99.0, "nice"), word(100.0, "clip"), word(100.4, "that")],
        };
        let t = transcribe::Transcript {
            choice: transcribe::Choice {
                model: "m".into(),
                device: junctur_core::runtime::Device::Cpu,
                note: None,
            },
            languages: vec!["en".into()],
            segments: vec![seg],
            audio_s: 2.0,
            elapsed_s: 1.0,
            resumed_chunks: 0,
        };
        std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&t).unwrap()).unwrap();
        std::fs::write(dir.join("rec.markers.txt"), "5:00 baron\nbad line\n").unwrap();
        let mut answers = moments::Answers::default();
        let v = view(&source, Some(cache.clone())).unwrap();
        assert!(v.heard);
        assert_eq!(v.markers_bad, 1);
        assert_eq!(v.moments.len(), 2);
        assert_eq!(
            (v.moments[0].moment.start, v.moments[0].moment.end),
            (70.0, 100.0)
        );
        assert_eq!(v.moments[1].moment.label, "baron");
        assert!(v.moments.iter().all(|m| m.state == State::Suggested));
        moments::reject(&v.moments[1].moment, &mut answers);
        moments::save_answers(&p.dir, &answers).unwrap();
        let v = view(&source, Some(cache)).unwrap();
        assert_eq!(v.moments[1].state, State::Rejected);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
