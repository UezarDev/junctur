//! Smart keep in the app (phases 3b and 3d): what the Properties section and the Suggestions lane
//! show of the rules' suggested keeps for a recording, the user's answers turned into edits of the
//! list in play, and the keep run on a thread of its own. A module because the view gathers from
//! four files of the project and the run outlives its command.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use junctur_core::config;
use junctur_core::edit::{frames_ceil, frames_floor};
use junctur_core::keep::{self, rules, Item, ItemState, Summary};
use junctur_core::pipeline::{self, Project, StageName, StageState};
use serde::{Deserialize, Serialize};

use crate::analysis::Editing;
use crate::lock::lock;
use crate::player::Core;

/// The event a keep run's progress is sent as.
pub const EVENT: &str = "keep";

#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub name: &'static str,
    pub prompt: &'static str,
}

/// Where the latest keep run is.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// The recording, as it was opened; empty before any run.
    pub file: String,
    pub running: bool,
    /// The run's last log line.
    pub line: Option<String>,
    pub error: Option<String>,
}

/// What the page shows of a recording's smart keep.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepView {
    /// The recording's own prompt, and the last one set on any (offered when it has none).
    pub prompt: Option<String>,
    pub last: Option<String>,
    /// Smart keep is on in Settings: a recording without a prompt is judged with the last one.
    pub on: bool,
    pub presets: Vec<Preset>,
    /// Smart keep runs once the transcript is done.
    pub transcribed: bool,
    /// The prompt the rules read, and how many spans they keep; `None` before they ran.
    pub judged_prompt: Option<String>,
    pub parts: usize,
    /// How that prompt was read ([`rules::Strictness::describe`]).
    pub reading: Option<String>,
    pub items: Vec<Item>,
    pub summary: Summary,
    /// The keep run, when it is this recording's.
    pub run: Option<Run>,
}

/// One keep run at a time, whichever recording: a run may decode a recording's game track.
#[derive(Default)]
pub struct Judging {
    run: Arc<Mutex<Run>>,
}

impl Judging {
    /// The run, when it is (or was last) `file`'s.
    pub fn of(&self, file: &Path) -> Option<Run> {
        let r = lock(&self.run).clone();
        (Path::new(&r.file) == file).then_some(r)
    }

    /// Runs smart keep on `file` with `prompt` on a thread of its own, `editing` held so no analysis of it
    /// starts meanwhile; `notify` hears every line and the end. Refused while another run is on.
    pub fn start(
        &self,
        file: &Path,
        cache_dir: Option<PathBuf>,
        prompt: String,
        editing: Option<Editing>,
        notify: impl Fn(&Run) + Send + Sync + 'static,
    ) -> Result<Run, String> {
        let first = {
            let mut r = lock(&self.run);
            if r.running {
                return Err(
                    "smart keep is reading another recording: wait for it to finish".into(),
                );
            }
            *r = Run {
                file: file.display().to_string(),
                running: true,
                line: Some("starting".into()),
                error: None,
            };
            r.clone()
        };
        let run = self.run.clone();
        let file = file.to_path_buf();
        let notify = Arc::new(notify);
        std::thread::spawn(move || {
            let _editing = editing;
            let tell = |f: &dyn Fn(&mut Run)| {
                let now = {
                    let mut r = lock(&run);
                    f(&mut r);
                    r.clone()
                };
                notify(&now);
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                judge(&file, cache_dir, prompt, &mut |line: &str| {
                    eprintln!("junctur-app keep: {line}");
                    tell(&|r| r.line = Some(line.to_string()));
                })
            }))
            .unwrap_or_else(|_| Err("smart keep stopped on an internal error".into()));
            tell(&|r| {
                r.running = false;
                r.error = result.as_ref().err().cloned();
            });
        });
        Ok(first)
    }
}

/// The keep stage alone on `file` with `prompt`, the user's other options from the config file.
fn judge(
    file: &Path,
    cache_dir: Option<PathBuf>,
    prompt: String,
    log: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let mut opts = config::analysis_options(None, true, true).map_err(|e| e.to_string())?;
    opts.keep.prompt = Some(prompt);
    match pipeline::judge(&p, &opts, log).map_err(|e| e.to_string())? {
        Some(_) => Ok(()),
        None => Err("smart keep is off for this recording".into()),
    }
}

/// Saves `prompt` as `file`'s own; a blank one clears it.
pub fn set_prompt(file: &Path, cache_dir: Option<PathBuf>, prompt: &str) -> Result<(), String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let mut c = keep::load_choices(&p.dir).map_err(|e| e.to_string())?;
    let prompt = prompt.trim();
    c.prompt = (!prompt.is_empty()).then(|| prompt.to_string());
    keep::save_choices(&p.dir, &c).map_err(|e| e.to_string())?;
    if !prompt.is_empty() {
        crate::settings::remember_prompt(prompt)?;
    }
    Ok(())
}

/// What `file`'s project says now: read again after every edit, so a hand edit stops a suggestion
/// at once.
pub fn view(file: &Path, cache_dir: Option<PathBuf>, run: Option<Run>) -> Result<KeepView, String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let choices = keep::load_choices(&p.dir).map_err(|e| e.to_string())?;
    let settings = config::load().map(|c| c.keep).unwrap_or_default();
    let transcribed = p
        .jobs()
        .get(StageName::Transcribe)
        .is_some_and(|r| r.state == StageState::Done);
    let mut v = KeepView {
        prompt: choices.prompt.clone(),
        last: settings.last.clone(),
        on: settings.on,
        presets: keep::PRESETS
            .iter()
            .map(|&(name, prompt)| Preset { name, prompt })
            .collect(),
        transcribed,
        judged_prompt: None,
        parts: 0,
        reading: None,
        items: Vec::new(),
        summary: Summary::default(),
        run,
    };
    let (Some(judged), Ok(list)) = (keep::load(&p.dir), p.edit()) else {
        return Ok(v);
    };
    let s = pipeline::standing(&p, &judged, &list, &choices);
    v.reading = Some(rules::Strictness::read(&judged.prompt).describe());
    v.judged_prompt = Some(judged.prompt);
    v.parts = judged.parts.len();
    v.items = s.items;
    v.summary = s.summary;
    Ok(v)
}

/// What the user does with smart keep's suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Every waiting suggestion, as one undo step.
    AcceptAll,
    /// The waiting suggestions within the span given (Review's, the lane's).
    Accept,
    /// A waiting suggestion turned down: no edit.
    Dismiss,
    /// An accepted part put back as it was.
    Undo,
}

/// Applies `action` to the open recording `file` (as `core` plays it): the edits first, as one
/// undo step, then the answers, so a refused edit records nothing. `at` names the part (its start
/// and end) for all but [`Action::AcceptAll`].
pub fn answer(
    core: &Core,
    file: Option<&str>,
    cache_dir: Option<PathBuf>,
    action: Action,
    at: Option<(f64, f64)>,
) -> Result<(), String> {
    let path = file.ok_or("no recording is open")?;
    let p = Project::open(Path::new(path), cache_dir, None).map_err(|e| e.to_string())?;
    let mut choices = keep::load_choices(&p.dir).map_err(|e| e.to_string())?;
    let judged = keep::load(&p.dir).ok_or("smart keep has not run on this recording")?;
    let list = p.edit().map_err(|e| e.to_string())?;
    // As the lane was shown them: Accept all takes exactly these.
    let pipeline::Standing { auto, items, .. } = pipeline::standing(&p, &judged, &list, &choices);
    let now = list.keep.as_slice();
    let near =
        |i: &&Item| at.is_some_and(|(a, b)| (i.start - a).abs() < 1e-3 && (i.end - b).abs() < 1e-3);
    let edits: Vec<(f64, f64, bool)> = match action {
        Action::AcceptAll => keep::accept(&items, &judged, (&auto, now), &mut choices, true),
        Action::Accept => {
            let picked: Vec<Item> = items.iter().filter(near).copied().collect();
            keep::accept(&picked, &judged, (&auto, now), &mut choices, false)
        }
        Action::Dismiss => {
            let item = items
                .iter()
                .find(|i| near(i) && i.state == ItemState::Suggested)
                .ok_or("that part is no longer a suggestion")?;
            keep::dismiss(item, &mut choices);
            Vec::new()
        }
        Action::Undo => {
            let item = items
                .iter()
                .find(|i| near(i) && i.state == ItemState::Accepted)
                .ok_or("that part is no longer an accepted suggestion")?;
            keep::undo(item, &judged, (&auto, now), &mut choices)
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
    keep::save_choices(&p.dir, &choices).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
