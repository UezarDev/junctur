//! The player: one mpv instance, its embedded video output, and an event thread that turns mpv's
//! events into [`PlayerState`] updates for the frontend (`player-state`).
//!
//! A recording plays in one of two forms: the whole file, or only the edit list's kept ranges as an
//! mpv EDL (see `cuts`). Which one is the user's skip toggle, while there is an edit list keeping
//! something; switching forms, or an edit list that changes what plays, reloads at the same source
//! position.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::cuts::{self, edl, watch::Watch, Edit, EditState, Form, Fps, Load};
use crate::lock::lock;
use crate::mpv::{self, Event, Events, Mpv};
use crate::state::{Change, PlayerState, OBSERVED};
use crate::trace::Trace;

pub const STATE_EVENT: &str = "player-state";
pub const EDIT_EVENT: &str = "edit-state";

/// Position updates arrive every frame; the frontend needs a few a second.
const POSITION_EVERY: Duration = Duration::from_millis(100);

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

/// Loads mpv has been asked for but not yet begun (its `start-file` pops them), in order.
type Loads = Arc<Mutex<VecDeque<Load>>>;

/// The recording and its edit list, as the commands left them.
struct Ctl {
    file: Option<String>,
    edit_path: Option<PathBuf>,
    /// Probed once per file, when an edit list first needs it.
    fps: Option<Fps>,
    edit: Edit,
    /// The user's toggle; on by default.
    skip: bool,
    /// The form of the last load asked for: what plays once in-flight loads land.
    requested: Form,
    watch: Option<Watch>,
}

impl Ctl {
    fn wanted(&self) -> Form {
        match (&self.edit.timeline, self.skip) {
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
    mpv: Arc<Mpv>,
    app: AppHandle,
    state: Arc<Mutex<PlayerState>>,
    loads: Loads,
    ctl: Mutex<Ctl>,
    events: Option<JoinHandle<()>>,
    #[cfg(windows)]
    video: Option<crate::video::VideoOutput>,
}

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
        let state = Arc::new(Mutex::new(PlayerState::default()));
        let loads = Loads::default();
        let events = {
            let mut reader = mpv.events().ok_or("mpv's event reader is already taken")?;
            let shared = Shared {
                state: state.clone(),
                loads: loads.clone(),
                app: app.clone(),
                trace,
            };
            std::thread::Builder::new()
                .name("mpv-events".into())
                .spawn(move || event_loop(&mut reader, &shared))
                .map_err(|e| e.to_string())?
        };
        Ok(Self {
            mpv,
            app,
            state,
            loads,
            ctl: Mutex::new(Ctl {
                file: None,
                edit_path: None,
                fps: None,
                edit: Edit::default(),
                skip: true,
                requested: Form::Source,
                watch: None,
            }),
            events: Some(events),
            #[cfg(windows)]
            video,
        })
    }

    pub fn state(&self) -> PlayerState {
        lock(&self.state).clone()
    }

    /// New mpv frames drawn onto the surface so far (redraws of the same frame not counted).
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
        let mut c = lock(&self.ctl);
        c.watch = None;
        c.file = Some(path.to_string());
        c.fps = None;
        c.edit = Edit::default();
        c.edit_path = edit_path.clone();
        self.read_edit(&mut c);
        if let Some(p) = edit_path {
            let app = self.app.clone();
            c.watch = Some(Watch::start(p, move || crate::commands::reload_edit(&app)));
        }
        lock(&self.state).set_reference(c.edit.timeline.clone());
        let form = c.wanted();
        let loaded = self.load(&mut c, form, None, false);
        self.emit_edit(&c);
        loaded
    }

    /// The edit list changed on disk: takes it and, when that changes what plays, reloads at the
    /// same source position.
    pub fn reload_edit(&self) {
        let mut c = lock(&self.ctl);
        if c.file.is_none() {
            return;
        }
        let before = c.edit.timeline.clone();
        self.read_edit(&mut c);
        let same = match (&before, &c.edit.timeline) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            lock(&self.state).set_reference(c.edit.timeline.clone());
            self.emit_state();
            let want = c.wanted();
            if !same_form(&want, &c.requested) {
                if let Err(e) = self.reload(&mut c, want) {
                    eprintln!("junctur-app: reload after an edit-list change: {e}");
                }
            }
        }
        self.emit_edit(&c);
    }

    pub fn set_skip(&self, on: bool) -> Result<(), String> {
        let mut c = lock(&self.ctl);
        c.skip = on;
        let want = c.wanted();
        let r = if c.file.is_some() && !same_form(&want, &c.requested) {
            self.reload(&mut c, want)
        } else {
            Ok(())
        };
        self.emit_edit(&c);
        r
    }

    pub fn edit_state(&self) -> EditState {
        let c = lock(&self.ctl);
        EditState::new(&c.edit, c.skip)
    }

    pub fn toggle_pause(&self) -> Result<(), String> {
        self.mpv.command(&["cycle", "pause"])
    }

    /// Seeks to source time `seconds`. While kept ranges play, a time inside a cut lands on the
    /// next kept range (past the last one, on the end). `exact` lands on the frame (slower on
    /// long GOPs); without it mpv seeks to a keyframe, which is what a dragged seek bar wants
    /// until it is released.
    pub fn seek(&self, seconds: f64, exact: bool) -> Result<(), String> {
        let target = match &lock(&self.ctl).requested {
            Form::Cuts(t) => cuts_start(t, seconds),
            Form::Source => seconds,
        };
        let mode = if exact {
            "absolute+exact"
        } else {
            "absolute+keyframes"
        };
        self.mpv.command(&["seek", &format!("{target:.6}"), mode])
    }

    pub fn set_audio_track(&self, id: i64) -> Result<(), String> {
        self.mpv.set_string("aid", &id.to_string())
    }

    /// Re-reads the edit list. A file that does not parse keeps the old timeline (a hand edit
    /// half-typed should not change what plays); one with the same frames keeps the same
    /// timeline, so only a real change reloads.
    fn read_edit(&self, c: &mut Ctl) {
        let (Some(file), Some(path)) = (c.file.clone(), c.edit_path.clone()) else {
            return;
        };
        if !path.is_file() {
            c.edit = Edit::default();
            return;
        }
        let fps = match c.fps {
            Some(f) => f,
            None => match cuts::probe_fps(file.as_ref()) {
                Ok(f) => *c.fps.insert(f),
                Err(e) => {
                    c.edit = Edit {
                        error: Some(format!("frame rate: {e}")),
                        ..Edit::default()
                    };
                    return;
                }
            },
        };
        let new = cuts::read(&path, fps);
        if new.summary.is_none() && new.error.is_some() && c.edit.timeline.is_some() {
            c.edit.error = new.error;
            return;
        }
        if let (Some(old), Some(t)) = (&c.edit.timeline, &new.timeline) {
            if old == t {
                c.edit = Edit {
                    timeline: Some(old.clone()),
                    ..new
                };
                return;
            }
        }
        c.edit = new;
    }

    /// Loads `form` at the current source position, keeping pause.
    fn reload(&self, c: &mut Ctl, form: Form) -> Result<(), String> {
        let (pos, paused) = {
            let s = lock(&self.state);
            (s.position, s.paused)
        };
        self.load(c, form, pos, paused)
    }

    /// Asks mpv to play `form` from source time `from` (the start when `None`).
    fn load(&self, c: &mut Ctl, form: Form, from: Option<f64>, paused: bool) -> Result<(), String> {
        let file = c.file.clone().ok_or("no file is open")?;
        let (url, start) = match &form {
            Form::Source => (file.clone(), from),
            Form::Cuts(t) => (edl::url(&file, t), from.map(|s| cuts_start(t, s))),
        };
        self.mpv
            .set_string("pause", if paused { "yes" } else { "no" })?;
        let start = start.map(|s| format!("start={s:.6}"));
        let mut args = vec!["loadfile", url.as_str(), "replace"];
        if let Some(s) = &start {
            // The playlist index (-1: none) comes before per-file options since mpv 0.38.
            args.extend(["-1", s.as_str()]);
        }
        lock(&self.loads).push_back(Load {
            file,
            form: form.clone(),
        });
        if let Err(e) = self.mpv.command(&args) {
            lock(&self.loads).pop_back();
            return Err(e);
        }
        c.requested = form;
        Ok(())
    }

    fn emit_state(&self) {
        let snapshot = lock(&self.state).clone();
        let _ = self.app.emit(STATE_EVENT, snapshot);
    }

    fn emit_edit(&self, c: &Ctl) {
        let _ = self.app.emit(EDIT_EVENT, EditState::new(&c.edit, c.skip));
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

/// Where source time `s` lands in an EDL of `t`: its output time, the next kept range's start when
/// `s` is cut, the end past the last range.
fn cuts_start(t: &cuts::Timeline, s: f64) -> f64 {
    t.snap(s)
        .map_or(t.output_duration(), |kept| t.to_output(kept).0)
}

/// What the event thread shares with the player.
struct Shared {
    state: Arc<Mutex<PlayerState>>,
    loads: Loads,
    app: AppHandle,
    trace: Option<Arc<Trace>>,
}

fn event_loop(events: &mut Events, shared: &Shared) {
    let state = &shared.state;
    let mut last_sent = Instant::now() - POSITION_EVERY;
    // A held-back position is sent once its interval is up, so the last one (a seek while
    // paused) is never lost.
    let mut held = false;
    let send = |last_sent: &mut Instant| {
        *last_sent = Instant::now();
        let snapshot = lock(state).clone();
        let _ = shared.app.emit(STATE_EVENT, snapshot);
    };
    loop {
        let timeout = if held {
            POSITION_EVERY
                .saturating_sub(last_sent.elapsed())
                .as_secs_f64()
        } else {
            -1.0
        };
        let Some(event) = events.wait(timeout) else {
            if held && last_sent.elapsed() >= POSITION_EVERY {
                held = false;
                send(&mut last_sent);
            }
            continue;
        };
        let change = match &event {
            Event::Shutdown => return,
            Event::Log {
                level,
                prefix,
                text,
            } => {
                eprintln!("mpv [{prefix}] {level}: {text}");
                continue;
            }
            Event::StartFile => {
                let Some(load) = lock(&shared.loads).pop_front() else {
                    eprintln!("junctur-app: mpv started a file the player did not load");
                    continue;
                };
                if let Some(t) = &shared.trace {
                    t.start(&load.form);
                }
                lock(state).start(load);
                Change::Other
            }
            e => {
                let mut s = lock(state);
                let change = s.apply(e);
                if let (Change::Position, true, Some(t), Some(o)) =
                    (change, s.cuts, &shared.trace, s.output)
                {
                    t.point(o);
                }
                change
            }
        };
        let due = match change {
            Change::None => false,
            Change::Position => last_sent.elapsed() >= POSITION_EVERY,
            Change::Other => true,
        };
        if due {
            held = false;
            send(&mut last_sent);
        } else if change == Change::Position {
            held = true;
        }
    }
}
