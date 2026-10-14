//! The event thread: mpv's events turned into [`PlayerState`], positions sent a few times a second
//! and the meters on a tick of their own.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use super::meters::{meter, send_weights, Levels, Meters};
use super::playback::Loads;
use super::{LEVELS_EVENT, STATE_EVENT};
use crate::lock::lock;
use crate::mixer::Mixer;
use crate::mpv::{Event, Events, Mpv};
use crate::state::{Change, PlayerState};
use crate::trace::Trace;

/// Position updates arrive every frame; the frontend needs a few a second.
const POSITION_EVERY: Duration = Duration::from_millis(100);

/// What the event thread shares with the player.
pub(super) struct Shared {
    pub(super) state: Arc<Mutex<PlayerState>>,
    pub(super) loads: Arc<Mutex<Loads>>,
    pub(super) mixer: Arc<Mutex<Mixer>>,
    pub(super) mpv: Arc<Mpv>,
    pub(super) app: AppHandle,
    pub(super) trace: Option<Arc<Trace>>,
}

/// When the state goes to the page: a change other than the position at once, positions at most
/// every [`POSITION_EVERY`]. A held-back position is sent once its interval is up, so the last one
/// (a seek while paused) is never lost.
pub(super) struct Pacer {
    last_sent: Instant,
    held: bool,
}

impl Pacer {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last_sent: now - POSITION_EVERY,
            held: false,
        }
    }

    /// How long until a held-back position is due at `now`; `None` with none held.
    pub(super) fn due_in(&self, now: Instant) -> Option<Duration> {
        self.held
            .then(|| POSITION_EVERY.saturating_sub(now.duration_since(self.last_sent)))
    }

    /// A wait ended at `now` with no event: whether the held-back position goes now.
    pub(super) fn idle(&mut self, now: Instant) -> bool {
        let due = self.held && now.duration_since(self.last_sent) >= POSITION_EVERY;
        if due {
            self.held = false;
        }
        due
    }

    /// The state changed by `change` at `now`: whether it goes now (else a position is held).
    pub(super) fn changed(&mut self, change: Change, now: Instant) -> bool {
        let due = match change {
            Change::None => false,
            Change::Position => now.duration_since(self.last_sent) >= POSITION_EVERY,
            Change::Other => true,
        };
        if due {
            self.held = false;
        } else if change == Change::Position {
            self.held = true;
        }
        due
    }

    /// The state was sent at `now`.
    pub(super) fn sent(&mut self, now: Instant) {
        self.last_sent = now;
    }
}

pub(super) fn event_loop(events: &mut Events, shared: &Shared) {
    let state = &shared.state;
    let mut pacer = Pacer::new(Instant::now());
    let mut meters = Meters::new();
    let send = |pacer: &mut Pacer| {
        pacer.sent(Instant::now());
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
        let timeout = [pacer.due_in(Instant::now()), meters.due_in(playing)]
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
            if pacer.idle(Instant::now()) {
                send(&mut pacer);
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
                // a recording up with no file open.
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
        if pacer.changed(change, Instant::now()) {
            send(&mut pacer);
        }
    }
}
