//! What the frontend shows of playback, folded from mpv's events into the source and output clocks
//! with the timeline of the load actually playing. Apart from the player so it is tested without
//! libmpv.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::cuts::{Form, Load, Timeline};
use crate::mpv::{audio_tracks, AudioTrack, EndReason, Event, Format, Value};

/// The properties the player observes, with the ids mpv echoes back in its change events. Not
/// `path`: in EDL form it is the whole EDL (a megabyte at 30 h); the file comes from the load.
pub const OBSERVED: [(u64, &str, Format); 8] = [
    (TIME_POS, "time-pos", Format::Double),
    (DURATION, "duration", Format::Double),
    (PAUSE, "pause", Format::Flag),
    (TRACK_LIST, "track-list", Format::Str),
    (HWDEC, "hwdec-current", Format::Str),
    // The picture's shape, which the editor's layout follows; integers read as doubles.
    (VIDEO_W, "video-params/dw", Format::Double),
    (VIDEO_H, "video-params/dh", Format::Double),
    (ROTATE, "video-params/rotate", Format::Double),
];
const TIME_POS: u64 = 2;
const DURATION: u64 = 3;
const PAUSE: u64 = 4;
const TRACK_LIST: u64 = 5;
const HWDEC: u64 = 6;
const VIDEO_W: u64 = 7;
const VIDEO_H: u64 = 8;
const ROTATE: u64 = 9;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    /// The recording playing (in either form).
    pub path: Option<String>,
    /// Source time, in either form.
    pub position: Option<f64>,
    /// Source length: the edit list's frame grid when there is one (so both forms agree), else
    /// mpv's.
    pub duration: Option<f64>,
    pub paused: bool,
    pub audio_tracks: Vec<AudioTrack>,
    /// The hardware decoder in use (`nvdec`, ...), `no` for software decoding.
    pub hwdec: Option<String>,
    /// Why the last file did not play, until another one loads.
    pub error: Option<String>,
    /// Only the kept ranges are playing.
    pub cuts: bool,
    /// Output (edited) time and length in mpv's EDL timeline, when there is a timeline.
    pub output: Option<f64>,
    pub output_duration: Option<f64>,
    /// The source frame and output frame on screen, when there is a timeline: what the clocks
    /// show, counted here once so the frontend does no frame rounding of its own.
    pub frame: Option<u64>,
    pub output_frame: Option<u64>,
    /// The source position is inside a cut (only when the whole file plays).
    pub in_cut: bool,
    /// The picture as shown: display size (pixel aspect applied) and its width over height, both
    /// turned for a rotated recording (a phone's 90 degrees). `None` without video.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub aspect: Option<f64>,
    #[serde(skip)]
    video: (Option<f64>, Option<f64>, f64),
    #[serde(skip)]
    mpv_pos: Option<f64>,
    #[serde(skip)]
    mpv_duration: Option<f64>,
    #[serde(skip)]
    form: Form,
    /// The current edit list's timeline, for output time while the whole file plays.
    #[serde(skip)]
    reference: Option<Arc<Timeline>>,
    /// Where a load or seek just asked to go (source time), until mpv's position gets there.
    #[serde(skip)]
    pending: Option<(f64, Instant)>,
    /// mpv's playlist entry of the playing load.
    #[serde(skip)]
    entry: i64,
}

/// How long a requested position stands in for mpv's before it is taken as having landed elsewhere.
const PENDING_FOR: Duration = Duration::from_secs(3);

/// How an event changed the state, so the position (which changes every frame) can be sent to
/// the frontend less often than everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    Position,
    Other,
}

impl PlayerState {
    /// mpv began `load` (its `start-file`).
    pub fn start(&mut self, load: Load) {
        self.entry = load.entry;
        self.path = Some(load.file);
        self.form = load.form;
        self.mpv_pos = None;
        self.mpv_duration = None;
        self.derive();
    }

    /// The recording was closed: nothing plays. The decoder in use, the pause state and the
    /// playlist entry stay what mpv last said; mpv's events after its `stop` clear the rest anyway.
    pub fn close(&mut self) {
        *self = Self {
            hwdec: self.hwdec.take(),
            paused: self.paused,
            entry: self.entry,
            ..Self::default()
        };
    }

    /// A load or seek is going to source time `s`.
    pub fn expect(&mut self, s: f64) {
        self.pending = Some((s, Instant::now()));
    }

    /// The source position to reload at: the one asked for while mpv has not reached it yet (a
    /// fresh load has no position, and right after a seek mpv still reports the old one), else
    /// mpv's.
    pub fn resume_from(&self) -> Option<f64> {
        match self.pending {
            Some((s, at)) if at.elapsed() < PENDING_FOR => Some(s),
            _ => self.position,
        }
    }

    /// The edit list changed: maps the whole file's positions with `t` from now on.
    pub fn set_reference(&mut self, t: Option<Arc<Timeline>>) {
        self.reference = t;
        self.derive();
    }

    pub fn apply(&mut self, e: &Event) -> Change {
        match e {
            Event::FileLoaded => {
                self.error = None;
                Change::Other
            }
            // Only the playing entry's failure: a load replaced while it opened is not an error.
            Event::EndFile {
                entry,
                reason: EndReason::Error,
                error,
            } if *entry == self.entry => {
                self.error = Some(error.clone().unwrap_or_else(|| "playback failed".into()));
                self.path = None;
                self.form = Form::Source;
                self.derive();
                Change::Other
            }
            Event::Property { id, value } => self.property(*id, value),
            _ => Change::None,
        }
    }

    fn property(&mut self, id: u64, value: &Value) -> Change {
        let num = |v: &Value| match v {
            Value::Double(d) => Some(*d),
            _ => None,
        };
        let text = |v: &Value| match v {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        };
        match id {
            TIME_POS => {
                self.mpv_pos = num(value);
                self.derive();
                return Change::Position;
            }
            HWDEC => self.hwdec = text(value),
            VIDEO_W => self.picture(|v| v.0 = num(value)),
            VIDEO_H => self.picture(|v| v.1 = num(value)),
            ROTATE => self.picture(|v| v.2 = num(value).unwrap_or(0.0)),
            DURATION => {
                self.mpv_duration = num(value);
                self.derive();
            }
            PAUSE => self.paused = matches!(value, Value::Flag(true)),
            TRACK_LIST => {
                self.audio_tracks = match value {
                    Value::Str(json) => audio_tracks(json).unwrap_or_else(|e| {
                        eprintln!("junctur-app: {e}");
                        Vec::new()
                    }),
                    _ => Vec::new(),
                }
            }
            _ => return Change::None,
        }
        Change::Other
    }

    /// Folds a change of the picture's size or rotation into its shape. mpv's `dw`/`dh` are the
    /// picture before rotation, so a quarter turn swaps them.
    fn picture(&mut self, f: impl FnOnce(&mut (Option<f64>, Option<f64>, f64))) {
        f(&mut self.video);
        let (w, h, rotate) = self.video;
        let size = w.zip(h).filter(|&(w, h)| w >= 1.0 && h >= 1.0);
        let turned = (rotate.round() as i64).rem_euclid(180) == 90;
        let size = size.map(|(w, h)| if turned { (h, w) } else { (w, h) });
        self.width = size.map(|s| s.0.round() as u32);
        self.height = size.map(|s| s.1.round() as u32);
        self.aspect = size.map(|(w, h)| w / h);
    }

    /// The two clocks from mpv's raw position and the timeline in play.
    fn derive(&mut self) {
        let t = match &self.form {
            Form::Cuts(t) => {
                self.cuts = true;
                self.in_cut = false;
                self.output = self.mpv_pos;
                self.position = self.mpv_pos.map(|o| t.to_source(o));
                Some(t.clone())
            }
            Form::Source => {
                self.cuts = false;
                self.position = self.mpv_pos;
                let t = self.reference.clone();
                let mapped = t.as_deref().zip(self.mpv_pos).map(|(t, s)| t.to_output(s));
                self.output = mapped.map(|m| m.0);
                self.in_cut = mapped.is_some_and(|m| m.1);
                t
            }
        };
        self.duration = t
            .as_deref()
            .map(Timeline::source_duration)
            .or(self.mpv_duration);
        self.output_duration = t.as_deref().map(Timeline::output_duration);
        let at = t.as_deref().zip(self.position);
        self.frame = at.map(|(t, s)| t.fps.frame_at(s));
        self.output_frame = at.map(|(t, s)| t.output_frame(s));
        if let (Some((want, _)), Some(pos)) = (self.pending, self.position) {
            if (pos - want).abs() < 0.5 {
                self.pending = None;
            }
        }
    }
}

#[cfg(test)]
mod tests;
