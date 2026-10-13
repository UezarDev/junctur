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
//! in the order editor, ctl, state, mixer (state and mixer are never held together).
//!
//! Every audio track plays at once, each with a meter and a mute (2e, `mixer`): each load carries
//! the mixer's filter graphs, a mute is sent to the playing graph in place, and the event thread
//! reads the meters while playing.

use std::collections::VecDeque;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use junctur_core::edit::{Captions, EditList, Speakers};
use junctur_core::export::srt::{self, Cue};
use junctur_core::export::{self, CaptionFiles};
use junctur_core::pipeline::{self, Found};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::cuts::{self, edl, watch, Edit, EditState, EditorStatus, Form, Fps, Load};
use crate::editor::{Edited, Editor, Op, Step, TranscriptView, Words};
use crate::lock::lock;
use crate::mixer::{self, Mixer, MixerState};
use crate::mpv::{self, Event, Events, Mpv};
use crate::state::{Change, PlayerState, OBSERVED};
use crate::trace::Trace;
use crate::voices;

pub const STATE_EVENT: &str = "player-state";
pub const EDIT_EVENT: &str = "edit-state";
pub const MIXER_EVENT: &str = "mixer-state";
pub const LEVELS_EVENT: &str = "levels";

/// How often the meters are read while playing: smooth enough for a meter, and one small property
/// read each time.
const LEVELS_EVERY: Duration = Duration::from_millis(50);

/// A meter reading that has not moved for this long is old: astats only measures audio as mpv
/// filters it, so a stalled player leaves its last reading behind. mpv filters in bursts as its
/// buffer drains, so a few reads in a row can match while all is well.
const LEVELS_STALE: Duration = Duration::from_millis(300);

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
    /// Shared with the event thread, which resends the mutes when playback restarts.
    mixer: Arc<Mutex<Mixer>>,
    /// Held while `mutes.json` is written, so writes land in the order of the clicks.
    saving: Mutex<()>,
    /// The user's caption defaults (2i), for an edit list without settings of its own.
    caption_defaults: Mutex<Captions>,
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
            // A first list (the app's own analysis just wrote it, 2d) had no history to clear.
            let notice =
                c.edit.list.is_some().then(|| {
                    "edit.junctur changed on disk: took it, undo history cleared".to_string()
                });
            c.status = EditorStatus {
                notice,
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

    /// Renames speaker `id` in the edit list and saves it (first taking any outside change). A
    /// speaker that is a remembered person renames the person (5b), in every recording.
    pub fn rename_speaker(&self, id: u32, name: &str) -> Result<(), String> {
        let name = junctur_core::edit::check_name(name)?;
        match self.file().map(|f| voices::rename(&f, id, name)) {
            Some(Ok(Some(found))) => self.respeak(&found),
            Some(Err(e)) => Err(e),
            _ => self.change(|ed, list, _| ed.rename(list, id, name)),
        }
    }

    /// Sets speaker `id`'s caption colour in the edit list and saves it; a remembered person's
    /// colour is theirs in every recording (5b).
    pub fn recolor_speaker(&self, id: u32, color: &str) -> Result<(), String> {
        match self.file().map(|f| voices::recolor(&f, id, color)) {
            Some(Ok(Some(found))) => self.respeak(&found),
            Some(Err(e)) => Err(e),
            _ => self.change(|ed, list, _| ed.recolor(list, id, color)),
        }
    }

    /// Remembers `name`'s voice from transcript words `first..=last` of the open recording (5b)
    /// and labels its speakers again; returns a line for the user. Works from the windows the
    /// speakers stage kept, so nothing is decoded and it takes a moment at most.
    pub fn enroll_voice(&self, first: usize, last: usize, name: &str) -> Result<String, String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        let stretch = words
            .stretch(first, last)
            .ok_or("no such words in the transcript")?;
        let file = self.file().ok_or("no recording is open")?;
        let (found, line) = voices::enroll(&file, name, stretch)?;
        self.respeak(&found)?;
        Ok(line)
    }

    /// Hears the open recording's speakers again from the audio (5c), the windows cut at the
    /// seconds pinned to people, and puts them into the list in play. Slow: the caller keeps it off
    /// the async runtime.
    ///
    /// The recording open when it ends may not be the one heard (another was opened meanwhile):
    /// the speakers go into the list in play only when that list is the heard recording's, checked
    /// under the editor's lock; otherwise into its edit list on disk. The re-hearing guard is held
    /// until then, so no enrollment slips in between.
    pub fn reanalyze_speakers(&self) -> Result<(), String> {
        let file = self.file().ok_or("no recording is open")?;
        let (found, _rehearing) = voices::rehear(&file)?;
        let mut here = false;
        if self.file().as_deref() == Some(file.as_str()) {
            self.change(|ed, list, _| {
                if !junctur_core::output::same_file(&list.source, Path::new(&file)) {
                    return Ok(None);
                }
                here = true;
                let speakers = pipeline::speakers_for(Some(&found), Some(&list.speakers));
                ed.respeak(list, speakers)
            })?;
        }
        if !here {
            voices::respeak_on_disk(&file, &found)?;
        }
        Ok(())
    }

    /// Forgets voice `name` and labels the open recording's speakers again without it.
    pub fn forget_voice(&self, name: &str) -> Result<(), String> {
        match voices::forget(self.file().as_deref(), name)? {
            Some(found) => self.respeak(&found),
            None => Ok(()),
        }
    }

    /// The open recording's path, as it was opened.
    pub fn file(&self) -> Option<String> {
        lock(&self.ctl).file.clone()
    }

    /// An analysis of `file` (absolute) ended (2d). If it is the open recording, the words and
    /// envelope read so far are dropped (a re-analysis may have replaced them) and the edit list on
    /// disk is taken now rather than at the watch's next poll.
    pub fn analysed(&self, file: &Path) {
        let open = self.file();
        if open.and_then(|f| std::path::absolute(f).ok()).as_deref() != Some(file) {
            return;
        }
        {
            let mut ed = lock(&self.editor);
            ed.words = None;
            ed.envelope = None;
        }
        self.reload_edit();
    }

    /// Puts newly labelled speakers into the list in play (names of speakers nobody enrolled are
    /// kept by id, as a re-cut does) and saves it; the kept ranges and the undo history stay.
    fn respeak(&self, found: &Found) -> Result<(), String> {
        self.change(|ed, list, _| {
            let speakers = pipeline::speakers_for(Some(found), Some(&list.speakers));
            ed.respeak(list, speakers)
        })
    }

    /// Closes the recording (2i: back to the start window). mpv stops and idles, and what was kept
    /// for the recording goes: loads not begun, the edit list and its watch, the editor's history
    /// and words, the mutes. Edits are saved as they are made, so nothing is lost. An analysis of it
    /// is not the player's and goes on (see `analysis`).
    pub fn close(&self) -> Result<(), String> {
        let mut ed = lock(&self.editor);
        ed.reset(None);
        let mut c = lock(&self.ctl);
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
        stopped
    }

    /// Sets the edit list's caption settings and saves them.
    pub fn set_captions(&self, captions: Captions) -> Result<(), String> {
        self.change(|ed, list, _| ed.set_captions(list, captions))
    }

    /// Writes the captions of the list in play to `out` as one SRT (5a, 5e), on the edited timeline
    /// at the frame rate the player rounds to. Read and written outside the locks once the words
    /// and the list are in hand.
    pub fn export_captions(&self, out: &Path) -> Result<CaptionFiles, String> {
        let (list, captions, words, fps) = self.caption_source()?;
        let tracked: Vec<_> = words
            .tracks
            .iter()
            .copied()
            .zip(words.list.iter())
            .collect();
        export::write_captions(&list, &captions, &tracked, fps.num, fps.den, out)
            .map_err(|e| format!("writing {}: {e}", out.display()))
    }

    /// The captions of the list in play as the SRT export cuts them (5d's preview over the
    /// picture): the same cues on the same output timeline, so the preview shows what Resolve
    /// will, cue for cue.
    pub fn caption_cues(&self) -> Result<Vec<Cue>, String> {
        let (list, captions, words, fps) = self.caption_source()?;
        let tracked = words.tracks.iter().copied().zip(words.list.iter());
        Ok(srt::cues(&list, &captions, tracked, fps.num, fps.den))
    }

    /// What a caption file is cut from: the list in play, its caption settings (its own, else the
    /// defaults it shows, 2i, so export and preview agree with the panel), the words the editor
    /// holds (an `Arc`, not a copy; each lock is held only to take its part) and the frame rate.
    fn caption_source(&self) -> Result<(Arc<EditList>, Captions, Arc<Words>, Fps), String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("captions need a transcript: run the pipeline on this recording")?;
        let (list, fps) = {
            let c = lock(&self.ctl);
            let list = c.edit.list.clone().ok_or("no edit list yet")?;
            (list, c.fps.ok_or("the frame rate is not known")?)
        };
        let captions = list
            .captions
            .unwrap_or_else(|| *lock(&self.caption_defaults));
        Ok((list, captions, words, fps))
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

    /// Keeps the right-hand `right`, bottom `bottom` and top `top` shares of the window (0..1) free
    /// of video, for the side panel, the play bar and track lanes under the picture (2g) and the
    /// title bar above it (2i); mpv centres the picture in what is left.
    pub fn set_video_margins(&self, right: f64, bottom: f64, top: f64) -> Result<(), String> {
        let share = |x: f64| {
            if x.is_finite() {
                x.clamp(0.0, 0.9)
            } else {
                0.0
            }
        };
        self.mpv
            .set_string("video-margin-ratio-right", &format!("{:.4}", share(right)))?;
        self.mpv
            .set_string("video-margin-ratio-top", &format!("{:.4}", share(top)))?;
        self.mpv.set_string(
            "video-margin-ratio-bottom",
            &format!("{:.4}", share(bottom)),
        )
    }

    /// The margins mpv has now, right, bottom and top (test mode; the top one is the title bar, 2i).
    pub fn video_margins(&self) -> [Option<String>; 3] {
        ["right", "bottom", "top"]
            .map(|side| self.mpv.get_string(&format!("video-margin-ratio-{side}")))
    }

    /// The edit list's speaker turns, for the Speakers lane (2g).
    pub fn turns(&self) -> TurnsView {
        let list = lock(&self.ctl).edit.list.clone();
        list.map_or_else(TurnsView::default, |l| TurnsView::new(&l.speakers))
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
        let defaults = *lock(&self.caption_defaults);
        EditState::new(&c.edit, c.skip, c.fps, &c.status, &defaults)
    }

    /// New caption defaults from Settings (2i): a list without its own settings shows them at once.
    pub fn set_caption_defaults(&self, captions: Captions) {
        *lock(&self.caption_defaults) = captions;
        self.emit_edit(&lock(&self.ctl));
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

    pub fn mixer(&self) -> MixerState {
        lock(&self.mixer).state()
    }

    /// Mutes or unmutes audio track `track` (0-based) at once, in the graph playing, and remembers
    /// it for the recording. Playback only: the edit list and renders never see it. The mute is
    /// applied (and shown) even when it cannot be remembered; the error then says so.
    pub fn set_muted(&self, track: usize, muted: bool) -> Result<(), String> {
        {
            let mut m = lock(&self.mixer);
            m.set_muted(track, muted)?;
            // Under the lock, so two quick clicks reach mpv in order.
            send_weights(&self.mpv, &m);
        }
        self.emit_mixer();
        // Written outside the mixer's lock (the event thread takes it on every restart and meter
        // tick); the mutes are read under `saving`, so the last write holds the last click.
        let _turn = lock(&self.saving);
        let memory = lock(&self.mixer).memory();
        match memory {
            Some(m) => m
                .write()
                .map_err(|e| format!("muted in the preview, but not {e}")),
            None => Ok(()),
        }
    }

    /// Solos or unsolos audio track `track` (0-based) at once (2g). Playback only and not remembered.
    pub fn set_solo(&self, track: usize, on: bool) -> Result<(), String> {
        {
            let mut m = lock(&self.mixer);
            m.set_solo(track, on)?;
            send_weights(&self.mpv, &m);
        }
        self.emit_mixer();
        Ok(())
    }

    /// The analysis of `file` (absolute) has its `tracks` stage done (2d): the mixer takes the
    /// roles it left, on a first analysis or a re-analysis that changed them, moving only the
    /// mutes the user never set, and tells the page when anything changed.
    pub fn tracks_known(&self, file: &Path) {
        let project = {
            let m = lock(&self.mixer);
            if !m.is_for(file) {
                return;
            }
            m.project().cloned()
        };
        // Read outside the lock.
        let Some(report) = project.as_ref().and_then(pipeline::cached_tracks) else {
            return;
        };
        {
            let mut m = lock(&self.mixer);
            // Another recording may have opened meanwhile.
            if !m.is_for(file) || !m.learn(&report) {
                return;
            }
            send_weights(&self.mpv, &m);
        }
        eprintln!("junctur-app: track roles taken for the mutes");
        self.emit_mixer();
    }

    /// The meters (each track's peak) and what is heard after the mutes, in dBFS, read now.
    pub fn levels(&self) -> (Vec<Option<f32>>, Option<f32>) {
        let n = lock(&self.mixer).len();
        let meter = meter(&self.mpv);
        let heard = self.mpv.get_string(&format!("af-metadata/{}", mixer::MIX));
        (
            meter.map_or_else(|| vec![None; n], |j| mixer::levels(&j, n)),
            heard.and_then(|j| mixer::heard(&j)),
        )
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
        // The playlist index (-1: none) comes before per-file options since mpv 0.38. The mixer's
        // graphs go with each load, with the mutes as they are now.
        let mut options = format!("start={start:.6}");
        if let Some(m) = lock(&self.mixer).load_options() {
            options.push(',');
            options.push_str(&m);
        }
        let args = ["loadfile", url.as_str(), "replace", "-1", options.as_str()];
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

    fn emit_mixer(&self) {
        let state = lock(&self.mixer).state();
        let _ = self.app.emit(MIXER_EVENT, state);
    }

    fn emit_edit(&self, c: &Ctl) {
        let defaults = *lock(&self.caption_defaults);
        let _ = self.app.emit(
            EDIT_EVENT,
            EditState::new(&c.edit, c.skip, c.fps, &c.status, &defaults),
        );
    }
}

/// Who talks when, as three arrays (source seconds, speaker ids from 1), sorted by start: a 30 h
/// recording has thousands of turns, sent once per change of `speakersKey`.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct TurnsView {
    pub start: Vec<f64>,
    pub end: Vec<f64>,
    pub speaker: Vec<u32>,
}

impl TurnsView {
    pub fn new(s: &Speakers) -> Self {
        let mut v = Self::default();
        for t in &s.turns {
            v.start.push(t.start);
            v.end.push(t.end);
            v.speaker.push(t.speaker);
        }
        v
    }
}

/// Sends `m`'s mutes to the graph playing. It fails when there is none (no file, no audio, or a
/// load whose chain is not up yet); the next `playback-restart` sends them again.
fn send_weights(mpv: &Mpv, m: &Mixer) {
    if m.len() > 0 {
        let w = m.weights();
        let _ = mpv.command(&["af-command", mixer::MIX, "weights", &w, mixer::MIX_FILTER]);
    }
}

/// The meter reading, as mpv's JSON (see `mixer::levels`); `None` without an audio chain.
fn meter(mpv: &Mpv) -> Option<String> {
    mpv.get_string(&format!("af-metadata/{}", mixer::METER))
}

#[derive(Clone, serde::Serialize)]
struct Levels {
    /// Each track's peak in dBFS (`None`: nothing live, shown as silence).
    levels: Vec<Option<f32>>,
}

/// The meters the event thread sends: every [`LEVELS_EVERY`] while playing, on a tick of their own
/// (not on mpv's position events), and silence once when playback pauses or stalls, so the page
/// never keeps a last reading up.
struct Meters {
    sent: Instant,
    /// The last reading and when it last moved.
    last: Option<String>,
    moved: Instant,
    /// Silence was sent last.
    silent: bool,
}

impl Meters {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            sent: now - LEVELS_EVERY,
            last: None,
            moved: now,
            silent: true,
        }
    }

    /// How long until the meters are due, when they need the event thread at all: while
    /// `playing`, and once more after it stops, for the silence.
    fn due_in(&self, playing: bool) -> Option<Duration> {
        (playing || !self.silent).then(|| LEVELS_EVERY.saturating_sub(self.sent.elapsed()))
    }

    /// What to send for `reading` at `now` (`playing`, `n` tracks): the levels, silence (all
    /// `None`) once when nothing live is left to show, or nothing.
    fn take(
        &mut self,
        reading: Option<String>,
        playing: bool,
        n: usize,
        now: Instant,
    ) -> Option<Vec<Option<f32>>> {
        self.sent = now;
        if reading != self.last {
            self.last = reading;
            self.moved = now;
        }
        let live = playing && n > 0 && now.duration_since(self.moved) < LEVELS_STALE;
        match (&self.last, live) {
            (Some(json), true) => {
                self.silent = false;
                Some(mixer::levels(json, n))
            }
            _ if !self.silent => {
                self.silent = true;
                Some(vec![None; n])
            }
            _ => None,
        }
    }
}

/// What the event thread shares with the player.
struct Shared {
    state: Arc<Mutex<PlayerState>>,
    loads: Arc<Mutex<Loads>>,
    mixer: Arc<Mutex<Mixer>>,
    mpv: Arc<Mpv>,
    app: AppHandle,
    trace: Option<Arc<Trace>>,
}

fn event_loop(events: &mut Events, shared: &Shared) {
    let state = &shared.state;
    let mut last_sent = Instant::now() - POSITION_EVERY;
    let mut meters = Meters::new();
    // A held-back position is sent once its interval is up, so the last one (a seek while
    // paused) is never lost.
    let mut held = false;
    let send = |last_sent: &mut Instant| {
        *last_sent = Instant::now();
        let snapshot = lock(state).clone();
        let _ = shared.app.emit(STATE_EVENT, snapshot);
    };
    loop {
        let tracks = lock(&shared.mixer).len();
        let playing = tracks > 0 && {
            let s = lock(state);
            s.path.is_some() && !s.paused
        };
        // Wake for a held-back position or the meters, whichever is due first.
        let timeout = [
            held.then(|| POSITION_EVERY.saturating_sub(last_sent.elapsed())),
            meters.due_in(playing),
        ]
        .into_iter()
        .flatten()
        .min()
        .map_or(-1.0, |d| d.as_secs_f64());
        let event = events.wait(timeout);
        if meters.due_in(playing).is_some_and(|d| d.is_zero()) {
            let reading = if playing { meter(&shared.mpv) } else { None };
            if let Some(levels) = meters.take(reading, playing, tracks, Instant::now()) {
                let _ = shared.app.emit(LEVELS_EVENT, Levels { levels });
            }
        }
        let Some(event) = event else {
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
            Event::PlaybackRestart => {
                // A mute changed after this load was asked for is not in its graph yet.
                send_weights(&shared.mpv, &lock(&shared.mixer));
                continue;
            }
            Event::StartFile { entry } => {
                // The state starts under the loads lock (taken before the state's, as `close`
                // takes them): a close between taking the load and starting it would otherwise put
                // a recording up with no file open (2i).
                let mut loads = lock(&shared.loads);
                let Some(load) = loads.started(*entry) else {
                    eprintln!(
                        "junctur-app: mpv started entry {entry}, which the player did not load"
                    );
                    continue;
                };
                if let Some(t) = &shared.trace {
                    t.start(&load.form);
                }
                lock(state).start(load);
                drop(loads);
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

    #[test]
    fn meters_fall_silent_once_when_paused_or_stalled() {
        let mut m = Meters::new();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let json = |db: &str| Some(format!(r#"{{"lavfi.astats.1.Peak_level":"{db}"}}"#));
        assert_eq!(m.due_in(false), None, "nothing to do before playing");
        assert_eq!(m.take(json("-10"), true, 1, at(0)), Some(vec![Some(-10.0)]));
        assert_eq!(
            m.take(json("-12"), true, 1, at(50)),
            Some(vec![Some(-12.0)])
        );
        // The same reading a few times running is still live (mpv filters in bursts)...
        assert_eq!(
            m.take(json("-12"), true, 1, at(200)),
            Some(vec![Some(-12.0)])
        );
        // ...until it is old: a stalled player. Silence is sent once.
        assert_eq!(m.take(json("-12"), true, 1, at(400)), Some(vec![None]));
        assert_eq!(m.take(json("-12"), true, 1, at(450)), None);
        assert_eq!(m.take(json("-8"), true, 1, at(500)), Some(vec![Some(-8.0)]));
        // Paused: silence once, then no more ticks.
        assert!(m.due_in(false).is_some(), "one more tick for the silence");
        assert_eq!(m.take(None, false, 1, at(550)), Some(vec![None]));
        assert_eq!(m.due_in(false), None);
    }
}
