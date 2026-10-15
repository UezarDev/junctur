//! The commands the frontend invokes, each forwarding to the player or the project. They are
//! `async` because an mpv command blocks the main thread; whole-recording work goes to
//! [`blocking`].

mod captions;
mod edit;
mod playback;
mod prefs;
mod recording;
mod speakers;
mod words;

// One module per part of the page; every command is still `commands::<name>` for the handler list.
pub use captions::*;
pub use edit::*;
pub use playback::*;
pub use prefs::*;
pub use recording::*;
pub use speakers::*;
pub use words::*;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::analysis::{self, Analysis, Change, Notify, Progress};
use crate::lock::lock;
use crate::player::{Core, Player};
use crate::translation::Translation;
use crate::waves::{self};
use junctur_core::pipeline::{StageName, StageState};

/// The player, or why it could not start (libmpv missing, no GL): the window still opens and
/// says so instead of the app failing to launch.
pub struct App {
    pub player: Mutex<Result<Player, String>>,
    pub analysis: Analysis,
    /// The open recording's waveforms.
    pub waves: waves::Store,
    /// The caption translation running in the background.
    pub translation: Translation,
}

impl App {
    fn with<T>(&self, f: impl FnOnce(&Player) -> Result<T, String>) -> Result<T, String> {
        let guard = lock(&self.player);
        match &*guard {
            Ok(p) => f(p),
            Err(e) => Err(e.clone()),
        }
    }

    /// The player's shared part, for a command that should not hold the app's player lock while
    /// it works (edits save a file; the transcript is parsed on first use).
    fn core(&self) -> Result<Arc<Core>, String> {
        self.with(|p| Ok(p.core()))
    }

    /// [`App::core`] for a command that changes the edit list: refused until the open recording's
    /// run has written its list, which would replace the edit. Held while the command works, so a
    /// run asked for meanwhile waits for it ([`Analysis::edit`]).
    pub(crate) fn editable(&self) -> Result<Editable, String> {
        let core = self.core()?;
        let file = core.file();
        let editing = match &file {
            Some(f) => Some(self.analysis.edit(Path::new(f)).ok_or(
                "analysing this recording: edits wait until its edit list is made".to_string(),
            )?),
            None => None,
        };
        Ok(Editable {
            core,
            file,
            _editing: editing,
        })
    }
}

/// The player's core for one edit of the open recording, and the mark that holds a run of that
/// recording off until the edit is done ([`App::editable`]).
pub(crate) struct Editable {
    core: Arc<Core>,
    file: Option<String>,
    _editing: Option<analysis::Editing>,
}

impl Editable {
    /// The recording this edit is for, as it was opened; the core's edits take it and refuse
    /// when another is open by the time they write.
    pub(crate) fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }
}

impl std::ops::Deref for Editable {
    type Target = Core;
    fn deref(&self) -> &Core {
        &self.core
    }
}

/// Sends a run's progress to the page, the player first taking what the run wrote, since the page
/// refetches on these events. Every change but a step also sends the transcription lane.
pub fn notifier(handle: AppHandle) -> Notify {
    Arc::new(move |p: &Progress, change| {
        // A step only moves a bar: reading the track roles on each would read the disk every time.
        if change == Change::Steps {
            let _ = handle.emit(analysis::EVENT, p);
            return;
        }
        if let Some(lane) = handle
            .try_state::<App>()
            .and_then(|a| a.analysis.lane_moved())
        {
            let _ = handle.emit(analysis::LANE_EVENT, lane);
        }
        let core = handle.try_state::<App>().and_then(|a| a.core().ok());
        if change == Change::Transcribed {
            if let Some(core) = core {
                core.transcribed(Path::new(&p.file), p.transcribed);
            }
            let _ = handle.emit(analysis::EVENT, p);
            return;
        }
        if let Some(core) = core {
            // The mutes follow the track roles as soon as the first stage is done; read on each
            // line after, which is cheap and catches a re-analysis whose roles moved.
            let tracks_done = p.project.as_ref().is_some_and(|s| {
                s.stages
                    .iter()
                    .any(|st| st.name == StageName::Tracks.name() && st.state == StageState::Done)
            });
            if tracks_done {
                core.tracks_known(Path::new(&p.file));
            }
            if !p.phase.busy() || change == Change::Listed {
                core.analysed(Path::new(&p.file), p.words_through());
            }
        }
        let _ = handle.emit(analysis::EVENT, p);
    })
}

/// Runs `f` on the async runtime's blocking threads: its result as it is, and a panic in it as the
/// join error's text.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// Whether the page has had the command line's file.
static STARTUP_TAKEN: AtomicBool = AtomicBool::new(false);

/// A file named on the command line (`junctur-app <file>`, or "Open with"), for the page to open
/// once it is ready. Only the first page start gets it, so a reloaded page does not open it again.
#[tauri::command]
pub fn startup_file() -> Option<String> {
    once(std::env::args().nth(1))
}

/// `arg` the first time, nothing after ([`STARTUP_TAKEN`]).
fn once(arg: Option<String>) -> Option<String> {
    if STARTUP_TAKEN.swap(true, Ordering::SeqCst) {
        None
    } else {
        arg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The only test of the process-wide `STARTUP_TAKEN`, which `startup_file` hands out through.
    #[test]
    fn startup_file_is_given_once() {
        assert_eq!(once(Some("a.mp4".into())).as_deref(), Some("a.mp4"));
        // A reloaded page asks again: the file is not opened a second time.
        assert_eq!(once(Some("a.mp4".into())), None);
        assert_eq!(once(None), None);
    }
}
