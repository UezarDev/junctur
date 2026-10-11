//! What the frontend shows of playback, folded from mpv's events. Kept apart from the player so
//! the folding is tested without libmpv.

use serde::Serialize;

use crate::mpv::{audio_tracks, AudioTrack, EndReason, Event, Format, Value};

/// The properties the player observes, with the ids mpv echoes back in its change events.
pub const OBSERVED: [(u64, &str, Format); 6] = [
    (PATH, "path", Format::Str),
    (TIME_POS, "time-pos", Format::Double),
    (DURATION, "duration", Format::Double),
    (PAUSE, "pause", Format::Flag),
    (TRACK_LIST, "track-list", Format::Str),
    (HWDEC, "hwdec-current", Format::Str),
];
const PATH: u64 = 1;
const TIME_POS: u64 = 2;
const DURATION: u64 = 3;
const PAUSE: u64 = 4;
const TRACK_LIST: u64 = 5;
const HWDEC: u64 = 6;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub path: Option<String>,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub paused: bool,
    pub audio_tracks: Vec<AudioTrack>,
    /// The hardware decoder in use (`nvdec`, ...), `no` for software decoding.
    pub hwdec: Option<String>,
    /// Why the last file did not play, until another one loads.
    pub error: Option<String>,
}

/// How an event changed the state, so the position (which changes every frame) can be sent to
/// the frontend less often than everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    Position,
    Other,
}

impl PlayerState {
    pub fn apply(&mut self, e: &Event) -> Change {
        match e {
            Event::FileLoaded => {
                self.error = None;
                Change::Other
            }
            Event::EndFile {
                reason: EndReason::Error,
                error,
            } => {
                self.error = Some(error.clone().unwrap_or_else(|| "playback failed".into()));
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
                self.position = num(value);
                return Change::Position;
            }
            PATH => self.path = text(value),
            HWDEC => self.hwdec = text(value),
            DURATION => self.duration = num(value),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(id: u64, value: Value) -> Event {
        Event::Property { id, value }
    }

    #[test]
    fn folds_properties() {
        let mut s = PlayerState::default();
        assert_eq!(
            s.apply(&prop(PATH, Value::Str("C:/a.mp4".into()))),
            Change::Other
        );
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
        assert_eq!(s.path.as_deref(), Some("C:/a.mp4"));
        assert_eq!(s.duration, Some(12.5));
        assert!(s.paused);
        assert_eq!(s.position, Some(3.0));
        assert_eq!(s.audio_tracks.len(), 2);
        assert!(s.audio_tracks[1].selected);
    }

    #[test]
    fn unavailable_properties_clear() {
        // mpv reports a property with no value (MPV_FORMAT_NONE) when no file is loaded.
        let mut s = PlayerState {
            position: Some(1.0),
            duration: Some(2.0),
            path: Some("x".into()),
            ..Default::default()
        };
        s.apply(&prop(TIME_POS, Value::None));
        s.apply(&prop(DURATION, Value::None));
        s.apply(&prop(PATH, Value::None));
        assert_eq!(
            (s.position, s.duration, s.path.as_deref()),
            (None, None, None)
        );
        assert_eq!(s.apply(&prop(99, Value::None)), Change::None);
    }

    #[test]
    fn errors_last_until_a_file_loads() {
        let mut s = PlayerState::default();
        s.apply(&Event::EndFile {
            reason: EndReason::Error,
            error: Some("unrecognized file format".into()),
        });
        assert_eq!(s.error.as_deref(), Some("unrecognized file format"));
        // Replacing a file ends the old one with Stop: not an error, and it keeps the message.
        assert_eq!(
            s.apply(&Event::EndFile {
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
    fn observed_ids_are_distinct() {
        let ids: std::collections::HashSet<u64> = OBSERVED.iter().map(|o| o.0).collect();
        assert_eq!(ids.len(), OBSERVED.len());
    }
}
