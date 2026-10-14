//! The player: one mpv instance, its embedded video, and an event thread turning mpv's events into
//! [`PlayerState`]; it plays the whole file or only the kept ranges (an EDL, see `cuts`).
//! Edits reach playback through [`Core::take`] directly, never waiting for the app's own save to
//! come back through the edit-list watch.
//! Locks are taken in the order editor, ctl, state, mixer (state and mixer never together).

mod captions;
mod edits;
mod events;
mod meters;
mod playback;
mod speakers;
mod words;

pub use speakers::TurnsView;
pub use words::FixDone;

use events::{event_loop, Shared};
use playback::Loads;

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use junctur_core::edit::{Captions, EditList};
use junctur_core::transcribe;
use junctur_core::translate::Translations;
use tauri::{AppHandle, Emitter};

use crate::cuts::{self, watch, Edit, EditState, EditorStatus, Form, Fps};
use crate::editor::{Editor, Words};
use crate::lock::lock;
use crate::mixer::Mixer;
use crate::mpv::{self, Mpv};
use crate::state::{Clock, PlayerState, OBSERVED};
use crate::trace::Trace;

/// The list in play, its caption settings, its words if transcribed, and the frame rate.
type EditSource = (Arc<EditList>, Captions, Option<Arc<Words>>, Fps);

pub const STATE_EVENT: &str = "player-state";
pub const EDIT_EVENT: &str = "edit-state";
pub const MIXER_EVENT: &str = "mixer-state";
pub const LEVELS_EVENT: &str = "levels";

/// How long shutdown waits for mpv's event thread before letting the app exit anyway.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// mpv options, set before it starts. `config=no` keeps a user's own mpv.conf from changing how
/// the app plays; `hwdec` tries NVDEC (CUDA-GL interop on the render API) and else whatever is
/// safe; `keep-open` pauses on the last frame instead of unloading the file.
const OPTIONS: &[(&str, &str)] = &[
    ("config", "no"),
    ("terminal", "no"),
    ("vo", "libmpv"),
    ("hwdec", "nvdec,auto-safe"),
    ("keep-open", "yes"),
    ("idle", "yes"),
    ("input-default-bindings", "no"),
    ("osc", "no"),
    // What mpv draws with no video (idle, between files): the window's own dark background.
    ("background-color", "#101114"),
];

/// The recording and its edit list, as the commands left them.
struct Ctl {
    file: Option<String>,
    edit_path: Option<PathBuf>,
    /// Probed once per file, when an edit list first needs it.
    fps: Option<Fps>,
    edit: Edit,
    skipping: Skipping,
    /// The form of the last load asked for: what plays once in-flight loads land.
    requested: Form,
    watch: Option<watch::Watch>,
    /// The editor's undo/redo state and last notice, copied here so the edit state can be sent
    /// under this lock.
    status: EditorStatus,
}

/// Whether only the kept ranges play: the user's toggle, or a review holding it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Skipping {
    /// The user's toggle; on by default.
    Toggle(bool),
    /// In review: off, with the toggle as it was before.
    Review { toggle: bool },
}

impl Skipping {
    fn on(self) -> bool {
        self == Skipping::Toggle(true)
    }

    fn reviewing(self) -> bool {
        matches!(self, Skipping::Review { .. })
    }
}

impl Ctl {
    fn wanted(&self) -> Form {
        match (&self.edit.timeline, self.skipping.on()) {
            (Some(t), true) => Form::Cuts(t.clone()),
            _ => Form::Source,
        }
    }
}

fn same_form(a: &Form, b: &Form) -> bool {
    match (a, b) {
        (Form::Source, Form::Source) => true,
        (Form::Cuts(x), Form::Cuts(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

pub struct Player {
    core: Arc<Core>,
    events: Option<JoinHandle<()>>,
    #[cfg(windows)]
    video: Option<crate::video::VideoOutput>,
}

/// The player's commands (`seek`, `set_skip`, ...) are [`Core`]'s.
impl Deref for Player {
    type Target = Core;
    fn deref(&self) -> &Core {
        &self.core
    }
}

pub struct Core {
    mpv: Arc<Mpv>,
    app: AppHandle,
    state: Arc<Mutex<Clock>>,
    loads: Arc<Mutex<Loads>>,
    /// Also serialises edits: one is applied, saved and handed to playback before the next.
    editor: Mutex<Editor>,
    ctl: Mutex<Ctl>,
    /// Shared with the event thread, which resends the mutes when playback restarts.
    mixer: Arc<Mutex<Mixer>>,
    /// Held while `mutes.json` is written, so writes land in the order of the clicks.
    saving: Mutex<()>,
    /// The user's caption defaults, for an edit list without settings of its own.
    caption_defaults: Mutex<Captions>,
    /// The open recording's translations as last read, and the file's size and time then: read
    /// again only when the background run has written it since.
    translations: Mutex<Option<(PathBuf, Stamp, Arc<Translations>)>>,
    /// The partial transcript as read so far while the recording transcribes: each landed piece
    /// parses only the chunks added since.
    so_far: Mutex<transcribe::SoFar>,
}

/// A file's size and modification time, to tell it was written since.
type Stamp = Option<(u64, std::time::SystemTime)>;

impl Player {
    /// Starts mpv with `extra` options after the defaults and embeds its video in `window`. Call
    /// on the UI thread. `trace` records skips for test mode.
    pub fn start(
        app: AppHandle,
        window: &tauri::WebviewWindow,
        extra: &[(&str, &str)],
        trace: Option<Arc<Trace>>,
    ) -> Result<Self, String> {
        let options: Vec<(&str, &str)> = OPTIONS.iter().chain(extra).copied().collect();
        let mpv = Arc::new(Mpv::new(mpv::load()?, &options)?);
        for (id, name, format) in OBSERVED {
            mpv.observe(id, name, format)?;
        }
        mpv.request_log("warn")?;
        #[cfg(windows)]
        let video = {
            let size = window.inner_size().map_err(|e| e.to_string())?;
            let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
            Some(crate::video::VideoOutput::attach(
                hwnd,
                size.width,
                size.height,
                mpv.clone(),
            )?)
        };
        #[cfg(not(windows))]
        {
            let _ = window;
            return Err("embedded video is only implemented on Windows so far".into());
        }
        let state = Arc::new(Mutex::new(Clock::default()));
        let loads = Arc::new(Mutex::new(Loads::default()));
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let events = {
            let mut reader = mpv.events().ok_or("mpv's event reader is already taken")?;
            let shared = Shared {
                state: state.clone(),
                loads: loads.clone(),
                mixer: mixer.clone(),
                mpv: mpv.clone(),
                app: app.clone(),
                trace,
            };
            std::thread::Builder::new()
                .name("mpv-events".into())
                .spawn(move || event_loop(&mut reader, &shared))
                .map_err(|e| e.to_string())?
        };
        let core = Arc::new(Core {
            mpv,
            app,
            state,
            loads,
            editor: Mutex::new(Editor::default()),
            mixer,
            saving: Mutex::new(()),
            caption_defaults: Mutex::new(Captions::default()),
            translations: Mutex::new(None),
            so_far: Mutex::new(transcribe::SoFar::default()),
            ctl: Mutex::new(Ctl {
                file: None,
                edit_path: None,
                fps: None,
                edit: Edit::default(),
                skipping: Skipping::Toggle(true),
                requested: Form::Source,
                watch: None,
                status: EditorStatus::default(),
            }),
        });
        Ok(Self {
            core,
            events: Some(events),
            #[cfg(windows)]
            video,
        })
    }

    /// The part commands and the watch share, to use without holding the app's player lock.
    pub fn core(&self) -> Arc<Core> {
        self.core.clone()
    }

    /// New mpv frames drawn onto the surface so far (redraws of the same frame not counted).
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub fn frames(&self) -> u64 {
        #[cfg(windows)]
        if let Some(v) = &self.video {
            return v.frames();
        }
        0
    }

    /// Plays `path` from the start (only its kept ranges when skipping is on and `edit_path`, the
    /// project's edit list, keeps something) and watches the edit list. `edit_path` is `None`
    /// when the project could not be opened.
    pub fn open(&self, path: &str, edit_path: Option<PathBuf>) -> Result<(), String> {
        // Stamp, then read (both outside the lock: the read may start ffprobe). The mixer's track
        // count may take a second ffprobe, run alongside rather than after.
        let baseline = edit_path.as_deref().map(watch::stamp);
        let dir = edit_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let ((fps, read), mixer) = std::thread::scope(|s| {
            let m = s.spawn(|| Mixer::open(path, dir));
            let read = match &edit_path {
                Some(p) => cuts::fetch(path, p, None),
                None => (None, None),
            };
            (read, m.join().unwrap_or_default())
        });
        let mut ed = lock(&self.editor);
        ed.reset(edit_path.clone());
        *lock(&self.so_far) = transcribe::SoFar::default();
        let mut c = lock(&self.ctl);
        // A review of the recording before ends with it, its skip toggle back for this one; the
        // load below clears the loop again anyway, so a failure here does not stop the open.
        let _ = self.leave_review(&mut c);
        c.file = Some(path.to_string());
        c.fps = fps;
        c.edit = read.unwrap_or_default();
        c.edit_path = edit_path.clone();
        c.status = EditorStatus::default();
        c.watch = edit_path.zip(baseline).map(|(p, base)| {
            let core = Arc::downgrade(&self.core);
            watch::Watch::start(p, base, move || {
                if let Some(core) = Weak::upgrade(&core) {
                    core.reload_edit();
                }
            })
        });
        lock(&self.state).set_reference(c.edit.timeline.clone());
        *lock(&self.mixer) = mixer;
        let form = c.wanted();
        let loaded = self.load(&mut c, form, None, false);
        self.emit_edit(&c);
        self.emit_mixer();
        loaded
    }

    pub fn resize(&self, w: u32, h: u32) {
        #[cfg(windows)]
        if let Some(v) = &self.video {
            v.resize(w, h);
        }
        #[cfg(not(windows))]
        let _ = (w, h);
    }

    /// Stops rendering, then mpv. Order matters: mpv's render context must be freed before the
    /// mpv handle is destroyed.
    pub fn shutdown(&mut self) {
        lock(&self.ctl).watch = None;
        #[cfg(windows)]
        if let Some(mut v) = self.video.take() {
            v.stop();
        }
        // `quit` makes mpv send its shutdown event, which ends the event thread. The wait is
        // bounded so a wedged mpv cannot hang the app's exit; the thread is then left behind
        // (it holds the last `Arc<Mpv>`, and the process is ending).
        let Some(t) = self.events.take() else {
            return;
        };
        if let Err(e) = self.mpv.command(&["quit"]) {
            eprintln!("junctur-app: mpv quit: {e}");
            return;
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !t.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if t.is_finished() {
            let _ = t.join();
        } else {
            eprintln!("junctur-app: mpv did not shut down in time; exiting without it");
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// An edit refused because another recording was opened while it was asked for.
pub const OPENED_MEANWHILE: &str = "another recording was opened meanwhile: nothing was changed";

impl Core {
    pub fn state(&self) -> PlayerState {
        lock(&self.state).shown()
    }

    /// The open recording's path, as it was opened.
    pub fn file(&self) -> Option<String> {
        lock(&self.ctl).file.clone()
    }

    /// An analysis of `file` (absolute) ended or wrote its edit list: if it is open, the words and
    /// envelope are dropped and the list on disk taken now. Words are read again up to `through`
    /// ([`Self::transcribed`]), never an older `transcript.json` under the new word indices.
    pub fn analysed(&self, file: &Path, through: Option<f64>) {
        if !self.is_open(file) {
            return;
        }
        {
            let mut ed = lock(&self.editor);
            ed.words = None;
            ed.envelope = None;
            ed.through = through;
        }
        self.reload_edit();
    }

    /// `file` (absolute) is transcribed up to `through` seconds on every track (a piece landed):
    /// if it is the open recording, its words are read again, up to there, when next
    /// asked for. `None`: the finished transcript's.
    pub fn transcribed(&self, file: &Path, through: Option<f64>) {
        if !self.is_open(file) {
            return;
        }
        let mut ed = lock(&self.editor);
        ed.words = None;
        ed.through = through;
    }

    fn is_open(&self, file: &Path) -> bool {
        let open = self.file();
        open.and_then(|f| std::path::absolute(f).ok()).as_deref() == Some(file)
    }

    /// Closes the recording: mpv idles and what was kept for it goes (loads, list, watch, history,
    /// mutes). Edits are saved as made, so nothing is lost; an analysis of it goes on.
    pub fn close(&self) -> Result<(), String> {
        let mut ed = lock(&self.editor);
        ed.reset(None);
        *lock(&self.so_far) = transcribe::SoFar::default();
        let mut c = lock(&self.ctl);
        let left = self.leave_review(&mut c);
        c.watch = None;
        c.file = None;
        c.edit_path = None;
        c.fps = None;
        c.edit = Edit::default();
        c.status = EditorStatus::default();
        c.requested = Form::Source;
        // A load asked for but not begun must not start the recording again; the loads stay locked
        // until the state is closed, so mpv's `start-file` for one cannot land in between.
        let stopped = {
            let mut loads = lock(&self.loads);
            loads.waiting.clear();
            let stopped = self.mpv.command(&["stop"]);
            let mut s = lock(&self.state);
            s.set_reference(None);
            s.close();
            stopped
        };
        *lock(&self.mixer) = Mixer::default();
        self.emit_edit(&c);
        drop(c);
        drop(ed);
        self.emit_state();
        self.emit_mixer();
        stopped.and(left)
    }

    /// The edit list's path and the list in play (test mode compares them).
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub fn edit_file(&self) -> (Option<PathBuf>, Option<Arc<EditList>>) {
        let c = lock(&self.ctl);
        (c.edit_path.clone(), c.edit.list.clone())
    }

    /// How many loads the player has asked mpv for (test mode checks saves do not reload twice).
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub fn loads(&self) -> i64 {
        lock(&self.loads).last
    }

    fn emit_state(&self) {
        let snapshot = lock(&self.state).shown();
        let _ = self.app.emit(STATE_EVENT, snapshot);
    }

    fn emit_mixer(&self) {
        let state = lock(&self.mixer).state();
        let _ = self.app.emit(MIXER_EVENT, state);
    }

    fn emit_edit(&self, c: &Ctl) {
        let defaults = lock(&self.caption_defaults).clone();
        let _ = self.app.emit(
            EDIT_EVENT,
            EditState::new(&c.edit, c.skipping.on(), c.fps, &c.status, &defaults),
        );
    }
}

#[cfg(test)]
mod tests;
