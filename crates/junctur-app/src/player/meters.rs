//! The mixer side of playback: mutes and solo sent to the graph playing, track roles taken once
//! known, and the meters the event thread sends.

use std::path::Path;
use std::time::{Duration, Instant};

use junctur_core::pipeline;

use super::Core;
use crate::lock::lock;
use crate::mixer::{self, Mixer, MixerState};
use crate::mpv::Mpv;

/// How often the meters are read while playing: smooth enough for a meter, and one small property
/// read each time.
const LEVELS_EVERY: Duration = Duration::from_millis(50);

/// A meter reading that has not moved for this long is old: astats only measures audio as mpv
/// filters it, so a stalled player leaves its last reading behind. mpv filters in bursts as its
/// buffer drains, so a few reads in a row can match while all is well.
const LEVELS_STALE: Duration = Duration::from_millis(300);

impl Core {
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

    /// Solos or unsolos audio track `track` (0-based) at once. Playback only and not remembered.
    pub fn set_solo(&self, track: usize, on: bool) -> Result<(), String> {
        {
            let mut m = lock(&self.mixer);
            m.set_solo(track, on)?;
            send_weights(&self.mpv, &m);
        }
        self.emit_mixer();
        Ok(())
    }

    /// The analysis of `file` (absolute) has its `tracks` stage done: the mixer takes the
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
}

/// Sends `m`'s mutes to the graph playing. It fails when there is none (no file, no audio, or a
/// load whose chain is not up yet); the next `playback-restart` sends them again.
pub(super) fn send_weights(mpv: &Mpv, m: &Mixer) {
    if m.len() > 0 {
        let w = m.weights();
        let _ = mpv.command(&["af-command", mixer::MIX, "weights", &w, mixer::MIX_FILTER]);
    }
}

/// The meter reading, as mpv's JSON (see `mixer::levels`); `None` without an audio chain.
pub(super) fn meter(mpv: &Mpv) -> Option<String> {
    mpv.get_string(&format!("af-metadata/{}", mixer::METER))
}

#[derive(Clone, serde::Serialize)]
pub(super) struct Levels {
    /// Each track's peak in dBFS (`None`: nothing live, shown as silence).
    pub(super) levels: Vec<Option<f32>>,
}

/// The meters the event thread sends: every [`LEVELS_EVERY`] while playing, on a tick of their own
/// (not on mpv's position events), and silence once when playback pauses or stalls, so the page
/// never keeps a last reading up.
pub(super) struct Meters {
    sent: Instant,
    /// The last reading and when it last moved.
    last: Option<String>,
    moved: Instant,
    /// Silence was sent last.
    silent: bool,
}

impl Meters {
    pub(super) fn new() -> Self {
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
    pub(super) fn due_in(&self, playing: bool) -> Option<Duration> {
        (playing || !self.silent).then(|| LEVELS_EVERY.saturating_sub(self.sent.elapsed()))
    }

    /// What to send for `reading` at `now` (`playing`, `n` tracks): the levels, silence (all
    /// `None`) once when nothing live is left to show, or nothing.
    pub(super) fn take(
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
