//! What the frontend shows of playback, folded from mpv's events. Kept apart from the player so
//! the folding is tested without libmpv.
//!
//! mpv's own position means source time when the whole file plays and output time when only the
//! kept ranges play (an EDL); this folds both into the two clocks the UI shows, with the timeline
//! of the load that is actually playing.

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
    // The picture's shape (2g: the editor's layout follows it); integers read as doubles.
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

    /// The recording was closed (2i): nothing plays. The decoder in use, the pause state and the
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
    /// picture before rotation (checked against the bundled libmpv: a 640x360 clip tagged 90 degrees
    /// reads 640x360 and `rotate` 270, while `video-out-params` says 360x640), so a quarter turn
    /// swaps them.
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
mod tests {
    use super::*;
    use crate::cuts::timeline::EDL_BIAS;
    use crate::cuts::Fps;
    use junctur_core::edit::Range;

    fn prop(id: u64, value: Value) -> Event {
        Event::Property { id, value }
    }

    fn source(file: &str) -> Load {
        Load {
            entry: 1,
            file: file.into(),
            form: Form::Source,
        }
    }

    /// Kept 1-2 s and 5-7 s of 10 s.
    fn timeline() -> Arc<Timeline> {
        let keep = [
            Range {
                start: 1.0,
                end: 2.0,
            },
            Range {
                start: 5.0,
                end: 7.0,
            },
        ];
        Arc::new(Timeline::new(&keep, 10.0, Fps { num: 30, den: 1 }))
    }

    #[test]
    fn folds_properties() {
        let mut s = PlayerState::default();
        s.start(source("C:/a.mp4"));
        assert_eq!(s.apply(&prop(DURATION, Value::Double(12.5))), Change::Other);
        assert_eq!(s.apply(&prop(PAUSE, Value::Flag(true))), Change::Other);
        assert_eq!(
            s.apply(&prop(TIME_POS, Value::Double(3.0))),
            Change::Position
        );
        let tracks = r#"[{"id":1,"type":"audio"},{"id":2,"type":"audio","selected":true}]"#;
        s.apply(&prop(TRACK_LIST, Value::Str(tracks.into())));
        s.apply(&prop(HWDEC, Value::Str("nvdec".into())));
        assert_eq!(s.hwdec.as_deref(), Some("nvdec"));
        assert_eq!(s.aspect, None, "no picture yet");
        s.apply(&prop(VIDEO_W, Value::Double(1920.0)));
        assert_eq!(s.aspect, None, "half a size is none");
        s.apply(&prop(VIDEO_H, Value::Double(1080.0)));
        assert!((s.aspect.unwrap() - 16.0 / 9.0).abs() < 1e-9);
        assert_eq!((s.width, s.height), (Some(1920), Some(1080)));
        // A phone recording stored landscape and rotated: mpv reads it as above with `rotate` 90
        // or 270 (as the bundled libmpv does), and it is portrait.
        s.apply(&prop(ROTATE, Value::Double(270.0)));
        assert_eq!((s.width, s.height), (Some(1080), Some(1920)));
        assert!(s.aspect.unwrap() < 1.0);
        s.apply(&prop(VIDEO_W, Value::None));
        assert_eq!(s.aspect, None, "the video went away");
        assert_eq!(s.path.as_deref(), Some("C:/a.mp4"));
        assert_eq!(s.duration, Some(12.5));
        assert!(s.paused);
        assert_eq!(s.position, Some(3.0));
        assert_eq!(s.audio_tracks.len(), 2);
        assert!(s.audio_tracks[1].selected);
        // No edit list: no output clock.
        assert_eq!((s.output, s.cuts, s.in_cut), (None, false, false));
    }

    #[test]
    fn closing_leaves_nothing_playing() {
        let mut s = PlayerState::default();
        s.start(source("C:/a.mp4"));
        s.apply(&prop(TIME_POS, Value::Double(3.0)));
        s.apply(&prop(DURATION, Value::Double(12.5)));
        s.apply(&prop(VIDEO_W, Value::Double(1920.0)));
        s.apply(&prop(VIDEO_H, Value::Double(1080.0)));
        s.apply(&prop(HWDEC, Value::Str("nvdec".into())));
        s.close();
        assert_eq!(s.path, None, "the start window shows");
        assert_eq!((s.position, s.duration, s.aspect), (None, None, None));
        assert_eq!(s.hwdec.as_deref(), Some("nvdec"));
        // mpv's own events after `stop` keep it empty.
        s.apply(&prop(TIME_POS, Value::None));
        assert_eq!((s.path, s.position), (None, None));
    }

    #[test]
    fn unavailable_properties_clear() {
        // mpv reports a property with no value (MPV_FORMAT_NONE) when no file is loaded.
        let mut s = PlayerState::default();
        s.start(source("x"));
        s.apply(&prop(TIME_POS, Value::Double(1.0)));
        s.apply(&prop(DURATION, Value::Double(2.0)));
        s.apply(&prop(TIME_POS, Value::None));
        s.apply(&prop(DURATION, Value::None));
        assert_eq!((s.position, s.duration), (None, None));
        assert_eq!(s.apply(&prop(99, Value::None)), Change::None);
    }

    #[test]
    fn errors_last_until_a_file_loads() {
        let mut s = PlayerState::default();
        s.start(source("bad.mp4"));
        // A replaced load's failure is not this one's.
        s.apply(&Event::EndFile {
            entry: 0,
            reason: EndReason::Error,
            error: Some("old".into()),
        });
        assert_eq!(s.error, None);
        s.apply(&Event::EndFile {
            entry: 1,
            reason: EndReason::Error,
            error: Some("unrecognized file format".into()),
        });
        assert_eq!(s.error.as_deref(), Some("unrecognized file format"));
        assert_eq!(s.path, None, "nothing is playing");
        // Replacing a file ends the old one with Stop: not an error, and it keeps the message.
        assert_eq!(
            s.apply(&Event::EndFile {
                entry: 1,
                reason: EndReason::Stop,
                error: None
            }),
            Change::None
        );
        assert!(s.error.is_some());
        s.apply(&Event::FileLoaded);
        assert_eq!(s.error, None);
    }

    #[test]
    fn whole_file_maps_to_output_time() {
        let mut s = PlayerState::default();
        s.start(source("a.mp4"));
        s.set_reference(Some(timeline()));
        s.apply(&prop(TIME_POS, Value::Double(5.5)));
        assert_eq!(s.position, Some(5.5));
        assert!((s.output.unwrap() - 1.5).abs() < 1e-3);
        assert_eq!((s.frame, s.output_frame), (Some(165), Some(45)));
        assert!(!s.in_cut && !s.cuts);
        s.apply(&prop(TIME_POS, Value::Double(3.0)));
        assert!(s.in_cut);
        assert_eq!(s.output_frame, Some(30), "a cut shows the next kept frame");
        assert!((s.output_duration.unwrap() - 3.0).abs() < 1e-9);
        // The edit list's length, not mpv's, so both forms show the same total.
        s.apply(&prop(DURATION, Value::Double(10.02)));
        assert_eq!(s.duration, Some(10.0));
    }

    #[test]
    fn a_requested_position_stands_in_until_mpv_reaches_it() {
        let mut s = PlayerState::default();
        s.start(source("a.mp4"));
        s.apply(&prop(TIME_POS, Value::Double(1.0)));
        assert_eq!(s.resume_from(), Some(1.0));
        s.expect(40.0);
        // mpv still reports where it was before the seek.
        s.apply(&prop(TIME_POS, Value::Double(1.03)));
        assert_eq!(s.resume_from(), Some(40.0));
        s.apply(&prop(TIME_POS, Value::Double(40.0)));
        s.apply(&prop(TIME_POS, Value::Double(40.5)));
        assert_eq!(s.resume_from(), Some(40.5));
        // A fresh load has no position yet.
        s.expect(0.0);
        s.start(source("a.mp4"));
        assert_eq!((s.position, s.resume_from()), (None, Some(0.0)));
    }

    #[test]
    fn kept_ranges_map_to_source_time() {
        let mut s = PlayerState::default();
        s.start(Load {
            entry: 1,
            file: "a.mp4".into(),
            form: Form::Cuts(timeline()),
        });
        // In the EDL mpv counts output time: frame 165 (5.5 s of source) shows at 1.5 s + the bias.
        s.apply(&prop(TIME_POS, Value::Double(1.5 + EDL_BIAS)));
        s.apply(&prop(DURATION, Value::Double(3.0)));
        assert!(s.cuts);
        assert_eq!(s.path.as_deref(), Some("a.mp4"));
        assert_eq!(s.output, Some(1.5 + EDL_BIAS));
        assert!((s.position.unwrap() - 5.5).abs() < 1e-3);
        assert_eq!((s.frame, s.output_frame), (Some(165), Some(45)));
        assert!((s.duration.unwrap() - 10.0).abs() < 1e-9, "source length");
        // The next load starts clean, in its own form.
        s.start(source("a.mp4"));
        assert_eq!((s.position, s.cuts), (None, false));
    }

    #[test]
    fn observed_ids_are_distinct() {
        let ids: std::collections::HashSet<u64> = OBSERVED.iter().map(|o| o.0).collect();
        assert_eq!(ids.len(), OBSERVED.len());
    }
}
