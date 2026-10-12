//! The player: one mpv instance, its embedded video output, and an event thread that turns mpv's
//! events into [`PlayerState`] updates for the frontend (`player-state`).
//!
//! A recording plays in one of two forms: the whole file, or only the edit list's kept ranges as an
//! mpv EDL (see `cuts`). Which one is the user's skip toggle, while there is an edit list keeping
//! something; switching forms, or an edit list that changes what plays, reloads at the same source
//! position.
//!
//! The parts that commands and the edit-list watch share live in [`Core`], behind an `Arc`, so the
//! watch reaches the player directly (through a `Weak`) instead of through the app's player lock.
//!
//! Edits (`editor`) reach playback the way a changed file does, through [`Core::take`], but
//! directly: the app never waits for its own save to come back through the watch. Locks are taken
//! in the order editor, ctl, state.

use std::collections::VecDeque;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use junctur_core::edit::{EditList, Speakers};
use junctur_core::pipeline;
use tauri::{AppHandle, Emitter};

use crate::cuts::{self, edl, watch, Edit, EditState, EditorStatus, Form, Fps, Load};
use crate::editor::{Edited, Editor, Op, Step, TranscriptView, Words};
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

/// Loads mpv has been asked for but not yet begun. mpv gives every `loadfile` the next playlist
/// entry id and names it in `start-file`; a load replaced before it began (a double-tapped skip
/// toggle) never starts, so matching by id rather than by order keeps every later load's
/// timeline right.
#[derive(Default)]
struct Loads {
    /// The last entry id mpv handed out (they count from 1).
    last: i64,
    waiting: VecDeque<Load>,
}

impl Loads {
    fn push(&mut self, file: String, form: Form) -> i64 {
        self.last += 1;
        self.waiting.push_back(Load {
            entry: self.last,
            file,
            form,
        });
        self.last
    }

    /// A `loadfile` mpv refused never got an entry.
    fn unpush(&mut self) {
        self.waiting.pop_back();
        self.last -= 1;
    }

    /// The load mpv began as `entry`; earlier ones were replaced and are dropped.
    fn started(&mut self, entry: i64) -> Option<Load> {
        while self.waiting.front().is_some_and(|l| l.entry < entry) {
            self.waiting.pop_front();
        }
        match self.waiting.front() {
            Some(l) if l.entry == entry => self.waiting.pop_front(),
            _ => None,
        }
    }
}

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
    watch: Option<watch::Watch>,
    /// The editor's undo/redo state and last notice, copied here so the edit state can be sent
    /// under this lock.
    status: EditorStatus,
}

impl Ctl {
    fn wanted(&self) -> Form {
        match (&self.edit.timeline, self.skip) {
            (Some(t), true) => Form::Cuts(t.clone()),
            _ => Form::Source,
        }
    }
}

/// What the edit bar should say after a saved edit: a backup made (and why), or word boundaries
/// placed without the envelope.
fn notice(e: &Edited) -> Option<String> {
    let name = |p: &std::path::Path| {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    match (&e.saved.backup, e.saved.replaced_unreadable) {
        (Some(b), true) => Some(format!(
            "edit.junctur did not parse; it was kept as {} and replaced",
            name(b)
        )),
        (None, true) => Some("edit.junctur did not parse; replaced it (a backup holds it)".into()),
        (Some(b), false) => Some(format!("the previous edit.junctur was kept as {}", name(b))),
        (None, false) if e.unsnapped => Some(
            "no speech envelope in this project: word cuts use the pads, not snapped to quiet"
                .into(),
        ),
        _ => None,
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
    state: Arc<Mutex<PlayerState>>,
    loads: Arc<Mutex<Loads>>,
    /// Also serialises edits: one is applied, saved and handed to playback before the next.
    editor: Mutex<Editor>,
    ctl: Mutex<Ctl>,
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
        let loads = Arc::new(Mutex::new(Loads::default()));
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
        let core = Arc::new(Core {
            mpv,
            app,
            state,
            loads,
            editor: Mutex::new(Editor::default()),
            ctl: Mutex::new(Ctl {
                file: None,
                edit_path: None,
                fps: None,
                edit: Edit::default(),
                skip: true,
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
        // Stamp, then read (both outside the lock: the read may start ffprobe).
        let baseline = edit_path.as_deref().map(watch::stamp);
        let (fps, read) = match &edit_path {
            Some(p) => cuts::fetch(path, p, None),
            None => (None, None),
        };
        let mut ed = lock(&self.editor);
        ed.reset(edit_path.clone());
        let mut c = lock(&self.ctl);
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
        let form = c.wanted();
        let loaded = self.load(&mut c, form, None, false);
        self.emit_edit(&c);
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

impl Core {
    pub fn state(&self) -> PlayerState {
        lock(&self.state).clone()
    }

    /// The edit list changed on disk: takes it and, when that changes what plays, reloads at the
    /// same source position. The file is read outside the ctl lock. The app's own save is already
    /// in play, so finding it here does nothing (no reload loop, no jump of the playhead); any
    /// other change ends the undo history, which would otherwise undo over it.
    pub fn reload_edit(&self) {
        let mut ed = lock(&self.editor);
        if let Err(e) = self.sync(&mut ed, "watch") {
            eprintln!("junctur-app: reload after an edit-list change: {e}");
        }
    }

    /// Takes the edit list on disk when someone other than the app changed it (the watch, and
    /// every edit before it applies, so an outside change inside the watch's settle time is played
    /// and edited on, not overwritten unseen). The app's own last save is skipped. A list that
    /// parses and differs from the one in play ends the undo history; one that does not parse
    /// keeps the ranges in play (`cuts::merge`) and says why in the edit state.
    /// `by` names the caller in the log line an outside change leaves.
    fn sync(&self, ed: &mut Editor, by: &str) -> Result<(), String> {
        let (file, path, fps) = {
            let c = lock(&self.ctl);
            let (Some(file), Some(path)) = (c.file.clone(), c.edit_path.clone()) else {
                return Ok(());
            };
            (file, path, c.fps)
        };
        if std::fs::read_to_string(&path).is_ok_and(|t| ed.is_own(&t)) {
            return Ok(());
        }
        let (fps, read) = cuts::fetch(&file, &path, fps);
        let mut c = lock(&self.ctl);
        if c.file.as_deref() != Some(file.as_str()) {
            return Ok(()); // another file was opened meanwhile
        }
        c.fps = c.fps.or(fps);
        let new_list = read.as_ref().and_then(|e| e.list.as_deref());
        if new_list.is_some() && new_list != c.edit.list.as_deref() {
            eprintln!("junctur-app: edit.junctur changed on disk, taken ({by})");
            ed.changed_elsewhere();
            c.status = EditorStatus {
                notice: Some("edit.junctur changed on disk: took it, undo history cleared".into()),
                ..EditorStatus::default()
            };
        }
        let merged = cuts::merge(&c.edit, read);
        self.take(&mut c, merged)
    }

    /// Plays `new` in place of the edit list in play: when its frames differ, maps positions with
    /// it and reloads the kept ranges at the same source position (snapped forward if now cut),
    /// keeping pause. Then reports the edit state.
    fn take(&self, c: &mut Ctl, new: Edit) -> Result<(), String> {
        let before = c.edit.timeline.clone();
        c.edit = new;
        let same = match (&before, &c.edit.timeline) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let mut result = Ok(());
        if !same {
            lock(&self.state).set_reference(c.edit.timeline.clone());
            self.emit_state();
            let want = c.wanted();
            if !same_form(&want, &c.requested) {
                result = self.reload(c, want);
            }
        }
        self.emit_edit(c);
        result
    }

    /// Applies an edit, undo or redo to the edit list (first taking any outside change on disk),
    /// saves it and plays the result.
    pub fn edit(&self, step: Step) -> Result<(), String> {
        if let Step::Do(Op::Words { .. }) = step {
            self.prepare();
        }
        self.change(|ed, list, fps| ed.edit(list, step, fps))
    }

    /// Renames speaker `id` in the edit list and saves it (first taking any outside change).
    pub fn rename_speaker(&self, id: u32, name: &str) -> Result<(), String> {
        self.change(|ed, list, _| ed.rename(list, id, name))
    }

    /// Takes any outside change on disk, lets `f` change and save the list in play, and plays
    /// the result.
    fn change(
        &self,
        f: impl FnOnce(&mut Editor, &EditList, Fps) -> Result<Option<Edited>, String>,
    ) -> Result<(), String> {
        let mut ed = lock(&self.editor);
        let synced = self.sync(&mut ed, "before an edit");
        let (list, fps) = {
            let c = lock(&self.ctl);
            let list = c.edit.list.clone().ok_or_else(|| {
                c.edit.error.clone().unwrap_or_else(|| {
                    "no edit list yet: run the pipeline on this recording".into()
                })
            })?;
            (list, c.fps.ok_or("the frame rate is not known")?)
        };
        synced.map_err(|e| format!("the preview did not reload: {e}"))?;
        let edited = f(&mut ed, &list, fps)?;
        let mut c = lock(&self.ctl);
        let mut status = EditorStatus {
            can_undo: ed.can_undo(),
            can_redo: ed.can_redo(),
            notice: None,
        };
        let Some(edited) = edited else {
            c.status = status;
            self.emit_edit(&c);
            return Ok(());
        };
        status.notice = notice(&edited);
        c.status = status;
        let merged = cuts::merge(&c.edit, Some(cuts::from_list(edited.list, fps)));
        self.take(&mut c, merged)
            .map_err(|e| format!("saved, but the preview did not reload: {e}"))
    }

    /// Reads the transcript and the speech envelope, if not yet in the editor, outside its lock
    /// (a 30 h transcript or envelope is tens of MB) so the watch and other edits are not held up.
    fn prepare(&self) {
        let (dir, words, env) = {
            let ed = lock(&self.editor);
            let Some(dir) = ed.project_dir().map(PathBuf::from) else {
                return;
            };
            (dir, ed.words.is_none(), ed.envelope.is_none())
        };
        let w = words.then(|| Words::load(&dir).map(Arc::new)).flatten();
        let e = env
            .then(|| pipeline::envelope(&dir).map(Arc::new))
            .flatten();
        let mut ed = lock(&self.editor);
        if ed.project_dir() == Some(dir.as_path()) {
            ed.words = ed.words.take().or(w);
            ed.envelope = ed.envelope.take().or(e);
        }
    }

    /// The transcript's words, read on first use (and again while there are none, as the pipeline
    /// may write them), labelled with the speakers of the list in play; `None` without a project
    /// or transcript.
    pub fn transcript(&self) -> Option<TranscriptView> {
        self.prepare();
        let words = lock(&self.editor).words.clone()?;
        let list = lock(&self.ctl).edit.list.clone();
        let none = Speakers::default();
        let speakers = list.as_deref().map_or(&none, |l| &l.speakers);
        Some(TranscriptView::new(&words, speakers))
    }

    /// Keeps the right-hand `share` of the window (0..1) free of video, for a panel there.
    pub fn set_video_margin(&self, share: f64) -> Result<(), String> {
        let share = if share.is_finite() {
            share.clamp(0.0, 0.8)
        } else {
            0.0
        };
        self.mpv
            .set_string("video-margin-ratio-right", &format!("{share:.4}"))
    }

    /// The edit list's path and the list in play (test mode compares them).
    pub fn edit_file(&self) -> (Option<PathBuf>, Option<Arc<EditList>>) {
        let c = lock(&self.ctl);
        (c.edit_path.clone(), c.edit.list.clone())
    }

    /// How many loads the player has asked mpv for (test mode checks saves do not reload twice).
    pub fn loads(&self) -> i64 {
        lock(&self.loads).last
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
        EditState::new(&c.edit, c.skip, c.fps, &c.status)
    }

    pub fn toggle_pause(&self) -> Result<(), String> {
        self.mpv.command(&["cycle", "pause"])
    }

    /// Seeks to source time `seconds` and returns the source time it lands on: while kept ranges
    /// play, a time inside a cut lands on the next kept range (past the last one, on its last
    /// frame). `exact` lands on the frame (slower on long GOPs); without it mpv seeks to a
    /// keyframe, which is what a dragged seek bar wants until it is released.
    pub fn seek(&self, seconds: f64, exact: bool) -> Result<f64, String> {
        let (target, lands) = match &lock(&self.ctl).requested {
            Form::Cuts(t) => t.seek_target(seconds),
            Form::Source => (seconds, seconds),
        };
        let mode = if exact {
            "absolute+exact"
        } else {
            "absolute+keyframes"
        };
        self.mpv.command(&["seek", &format!("{target:.6}"), mode])?;
        lock(&self.state).expect(lands);
        Ok(lands)
    }

    pub fn set_audio_track(&self, id: i64) -> Result<(), String> {
        self.mpv.set_string("aid", &id.to_string())
    }

    /// Loads `form` where playback is (or is about to be, after a seek or load still landing),
    /// keeping pause.
    fn reload(&self, c: &mut Ctl, form: Form) -> Result<(), String> {
        let (pos, paused) = {
            let s = lock(&self.state);
            (s.resume_from(), s.paused)
        };
        self.load(c, form, pos, paused)
    }

    /// Asks mpv to play `form` from source time `from` (the start when `None`).
    fn load(&self, c: &mut Ctl, form: Form, from: Option<f64>, paused: bool) -> Result<(), String> {
        let file = c.file.clone().ok_or("no file is open")?;
        let from = from.unwrap_or(0.0);
        let (url, start, lands) = match &form {
            Form::Source => (file.clone(), from, from),
            Form::Cuts(t) => {
                let (o, s) = t.seek_target(from);
                (edl::url(&file, t), o, s)
            }
        };
        self.mpv
            .set_string("pause", if paused { "yes" } else { "no" })?;
        // The playlist index (-1: none) comes before per-file options since mpv 0.38.
        let start = format!("start={start:.6}");
        let args = ["loadfile", url.as_str(), "replace", "-1", start.as_str()];
        lock(&self.loads).push(file, form.clone());
        if let Err(e) = self.mpv.command(&args) {
            lock(&self.loads).unpush();
            return Err(e);
        }
        lock(&self.state).expect(lands);
        c.requested = form;
        Ok(())
    }

    fn emit_state(&self) {
        let snapshot = lock(&self.state).clone();
        let _ = self.app.emit(STATE_EVENT, snapshot);
    }

    fn emit_edit(&self, c: &Ctl) {
        let _ = self.app.emit(
            EDIT_EVENT,
            EditState::new(&c.edit, c.skip, c.fps, &c.status),
        );
    }
}

/// What the event thread shares with the player.
struct Shared {
    state: Arc<Mutex<PlayerState>>,
    loads: Arc<Mutex<Loads>>,
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
            Event::StartFile { entry } => {
                let Some(load) = lock(&shared.loads).started(*entry) else {
                    eprintln!(
                        "junctur-app: mpv started entry {entry}, which the player did not load"
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_load_does_not_shift_later_ones() {
        let mut l = Loads::default();
        let a = l.push("f".into(), Form::Source);
        let b = l.push("f".into(), Form::Source);
        let c = l.push("f".into(), Form::Source);
        assert_eq!((a, b, c), (1, 2, 3));
        // 2 was replaced before it began, so mpv starts 3 after 1.
        assert_eq!(l.started(1).map(|x| x.entry), Some(1));
        assert_eq!(l.started(3).map(|x| x.entry), Some(3));
        assert!(l.waiting.is_empty());
        // A refused loadfile takes back its id.
        l.push("f".into(), Form::Source);
        l.unpush();
        assert_eq!(l.push("f".into(), Form::Source), 4);
        assert!(l.started(9).is_none(), "an entry the player never loaded");
    }
}
