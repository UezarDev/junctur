//! What mpv plays and where: loads matched to the entries mpv starts, seeking, review's loop and
//! speed, the skip toggle and the video margins.

use std::collections::VecDeque;

use std::sync::Arc;

use junctur_core::edit::Range;

use super::{same_form, Core, Ctl, Skipping};
use crate::cuts::{edl, Form, Load, Timeline};
use crate::lock::lock;
use crate::mpv::Mpv;

/// Loads mpv has been asked for but not yet begun. mpv gives every `loadfile` the next playlist
/// entry id and names it in `start-file`; a load replaced before it began (a double-tapped skip
/// toggle) never starts, so matching by id rather than by order keeps every later load's
/// timeline right.
#[derive(Default)]
pub(super) struct Loads {
    /// The last entry id mpv handed out (they count from 1).
    pub(super) last: i64,
    pub(super) waiting: VecDeque<Load>,
}

impl Loads {
    pub(super) fn push(&mut self, file: String, form: Form) -> i64 {
        self.last += 1;
        self.waiting.push_back(Load {
            entry: self.last,
            file,
            form,
        });
        self.last
    }

    /// A `loadfile` mpv refused never got an entry.
    pub(super) fn unpush(&mut self) {
        self.waiting.pop_back();
        self.last -= 1;
    }

    /// The load mpv began as `entry`; earlier ones were replaced and are dropped.
    pub(super) fn started(&mut self, entry: i64) -> Option<Load> {
        while self.waiting.front().is_some_and(|l| l.entry < entry) {
            self.waiting.pop_front();
        }
        match self.waiting.front() {
            Some(l) if l.entry == entry => self.waiting.pop_front(),
            _ => None,
        }
    }
}

/// Whether `play_from` can start at source second `at`: a time mpv can seek to, never NaN, an
/// infinity or before the start, which would reach mpv as text it refuses or reads otherwise.
pub(super) fn playable(at: f64) -> Result<(), String> {
    if at.is_finite() && at >= 0.0 {
        Ok(())
    } else {
        Err(format!("cannot play from {at}"))
    }
}

/// Clears mpv's A-B loop (review's loop of one clip).
fn clear_loop(mpv: &Mpv) -> Result<(), String> {
    mpv.set_string("ab-loop-a", "no")?;
    mpv.set_string("ab-loop-b", "no")
}

/// The speeds the player takes: review offers 1x to 2x; mpv's pitch correction holds voices
/// natural well past that, and outside a quarter to four times nothing is judged by ear.
pub(super) fn speed_ok(speed: f64) -> bool {
    (0.25..=4.0).contains(&speed)
}

impl Core {
    /// Keeps the right-hand `right`, bottom `bottom` and top `top` shares of the window (0..1) free
    /// of video, for the side panel, the play bar and track lanes under the picture and the title
    /// bar above it; mpv centres the picture in what is left.
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

    /// The margins mpv has now, right, bottom and top (test mode).
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub fn video_margins(&self) -> [Option<String>; 3] {
        ["right", "bottom", "top"]
            .map(|side| self.mpv.get_string(&format!("video-margin-ratio-{side}")))
    }

    pub fn set_skip(&self, on: bool) -> Result<(), String> {
        let mut c = lock(&self.ctl);
        if c.skipping.reviewing() {
            return Err("Skip cuts stays off while reviewing".into());
        }
        c.skipping = Skipping::Toggle(on);
        let want = c.wanted();
        let r = if c.file.is_some() && !same_form(&want, &c.requested) {
            self.reload(&mut c, want)
        } else {
            Ok(())
        };
        self.emit_edit(&c);
        r
    }

    /// Plays `keep` (source seconds, sorted) in place of the list's kept ranges, skipping on, until
    /// `None`, an edit of the list or another recording: how 7c's ghosts are scrubbed before they
    /// are accepted. Refused in review, and when it keeps nothing.
    pub fn set_preview(&self, keep: Option<&[Range]>) -> Result<(), String> {
        let mut c = lock(&self.ctl);
        if c.skipping.reviewing() {
            return Err("reviewing: end the review to preview".into());
        }
        c.preview = match keep {
            None => None,
            Some(keep) => {
                let list = c.edit.list.clone().ok_or("no edit list to preview")?;
                let fps = c.fps.ok_or("the frame rate is not known")?;
                let t = Timeline::new(keep, list.duration, fps);
                if t.is_empty() {
                    return Err("the preview keeps nothing".into());
                }
                c.skipping = Skipping::Toggle(true);
                Some(Arc::new(t))
            }
        };
        let want = c.wanted();
        let r = if c.file.is_some() && !same_form(&want, &c.requested) {
            self.reload(&mut c, want)
        } else {
            Ok(())
        };
        self.emit_edit(&c);
        r
    }

    pub fn toggle_pause(&self) -> Result<(), String> {
        self.mpv.command(&["cycle", "pause"])
    }

    /// Loops source seconds `start` to `end` with mpv's A-B loop (exact frame, and past
    /// `keep-open`'s pause at the end). The loop is in source time, so the first call enters
    /// review: the whole file plays until `end_review` or an open or close.
    pub fn play_between(&self, start: f64, end: f64) -> Result<(), String> {
        if !(start.is_finite() && end.is_finite() && start >= 0.0 && end > start) {
            return Err(format!("no clip from {start} to {end}"));
        }
        let mut c = lock(&self.ctl);
        if c.file.is_none() {
            return Err("no file is open".into());
        }
        if let Skipping::Toggle(toggle) = c.skipping {
            c.skipping = Skipping::Review { toggle };
            self.emit_edit(&c);
        }
        if same_form(&Form::Source, &c.requested) {
            // The seek first: one refused leaves no loop behind.
            self.mpv
                .command(&["seek", &format!("{start:.6}"), "absolute+exact"])?;
            lock(&self.state).expect(start);
        } else {
            // A seek sent while this load begins would be lost; the load starts at `start`.
            self.load(&mut c, Form::Source, Some(start), false)?;
        }
        self.mpv.set_string("ab-loop-a", &format!("{start:.6}"))?;
        self.mpv.set_string("ab-loop-b", &format!("{end:.6}"))?;
        self.mpv.set_string("pause", "no")
    }

    /// Plays from source second `at`. Over the kept ranges they are loaded again from there, since
    /// a seek sent while the edge move's load begins is lost. Refused in review.
    pub fn play_from(&self, at: f64) -> Result<(), String> {
        playable(at)?;
        let mut c = lock(&self.ctl);
        if c.file.is_none() {
            return Err("no file is open".into());
        }
        if c.skipping.reviewing() {
            return Err("reviewing: the clip under review plays".into());
        }
        if same_form(&Form::Source, &c.requested) {
            self.mpv
                .command(&["seek", &format!("{at:.6}"), "absolute+exact"])?;
            lock(&self.state).expect(at);
            self.mpv.set_string("pause", "no")
        } else {
            let form = c.requested.clone();
            self.load(&mut c, form, Some(at), false)
        }
    }

    /// Stops `play_between`'s loop and pauses, staying in review (past the last clip).
    pub fn pause_review(&self) -> Result<(), String> {
        clear_loop(&self.mpv)?;
        self.mpv.set_string("pause", "yes")
    }

    /// Leaves review: no loop, speed 1x, the skip toggle as it was before. Nothing to do when
    /// not reviewing (an open or close already ended it).
    pub fn end_review(&self) -> Result<(), String> {
        let mut c = lock(&self.ctl);
        self.leave_review(&mut c)?;
        let want = c.wanted();
        let r = if c.file.is_some() && !same_form(&want, &c.requested) {
            self.reload(&mut c, want)
        } else {
            Ok(())
        };
        self.emit_edit(&c);
        r
    }

    /// Ends a review in `c`, if one is on, without reloading: the caller loads what it wants next.
    pub(super) fn leave_review(&self, c: &mut Ctl) -> Result<(), String> {
        if let Skipping::Review { toggle } = c.skipping {
            c.skipping = Skipping::Toggle(toggle);
            clear_loop(&self.mpv)?;
            self.mpv.set_string("speed", "1")?;
        }
        Ok(())
    }

    /// Playback speed (review's speed control), with mpv's pitch correction.
    pub fn set_speed(&self, speed: f64) -> Result<(), String> {
        if !speed_ok(speed) {
            return Err(format!("no playback speed {speed}"));
        }
        self.mpv.set_string("speed", &format!("{speed}"))
    }

    /// The speed and A-B loop mpv has now (test mode).
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub fn looping(&self) -> [Option<String>; 3] {
        ["speed", "ab-loop-a", "ab-loop-b"].map(|p| self.mpv.get_string(p))
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

    /// Loads `form` where playback is (or is about to be, after a seek or load still landing),
    /// keeping pause.
    pub(super) fn reload(&self, c: &mut Ctl, form: Form) -> Result<(), String> {
        let (pos, paused) = {
            let s = lock(&self.state);
            (s.resume_from(), s.paused)
        };
        self.load(c, form, pos, paused)
    }

    /// Asks mpv to play `form` from source time `from` (the start when `None`).
    pub(super) fn load(
        &self,
        c: &mut Ctl,
        form: Form,
        from: Option<f64>,
        paused: bool,
    ) -> Result<(), String> {
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
        // A review's loop was set in the times of the load it was set for.
        clear_loop(&self.mpv)?;
        // The playlist index (-1: none) comes before per-file options (mpv 0.38 on). The mixer's
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
}
