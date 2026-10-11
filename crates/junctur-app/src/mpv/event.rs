//! mpv events, copied out of mpv's memory into owned values (mpv reuses an event's memory on the
//! next `mpv_wait_event`).

use std::ffi::{c_char, CStr};

use super::ffi;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Str(String),
    Flag(bool),
    Double(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stop,
    Quit,
    Error,
    Other,
}

impl EndReason {
    pub fn from_raw(reason: i32) -> Self {
        match reason {
            0 => Self::Eof,
            2 => Self::Stop,
            3 => Self::Quit,
            4 => Self::Error,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Shutdown,
    FileLoaded,
    /// `error` is mpv's message when the file could not be played.
    EndFile {
        reason: EndReason,
        error: Option<String>,
    },
    Property {
        id: u64,
        value: Value,
    },
    Log {
        level: String,
        prefix: String,
        text: String,
    },
    Other,
}

/// # Safety
/// `e` must be a live event from `mpv_wait_event` on a handle whose library `api` is.
pub(super) unsafe fn decode(api: &ffi::Api, e: &ffi::Event) -> Option<Event> {
    Some(match e.event_id {
        ffi::EVENT_NONE => return None,
        ffi::EVENT_SHUTDOWN => Event::Shutdown,
        ffi::EVENT_FILE_LOADED => Event::FileLoaded,
        ffi::EVENT_END_FILE => {
            let end = &*(e.data as *const ffi::EventEndFile);
            let reason = EndReason::from_raw(end.reason);
            Event::EndFile {
                reason,
                error: (reason == EndReason::Error).then(|| text((api.error_string)(end.error))),
            }
        }
        ffi::EVENT_PROPERTY_CHANGE => {
            let p = &*(e.data as *const ffi::EventProperty);
            let value = if p.data.is_null() {
                Value::None
            } else {
                match p.format {
                    ffi::FORMAT_STRING => Value::Str(text(*(p.data as *const *const c_char))),
                    ffi::FORMAT_FLAG => Value::Flag(*(p.data as *const i32) != 0),
                    ffi::FORMAT_DOUBLE => Value::Double(*(p.data as *const f64)),
                    _ => Value::None,
                }
            };
            Event::Property {
                id: e.reply_userdata,
                value,
            }
        }
        ffi::EVENT_LOG_MESSAGE => {
            let m = &*(e.data as *const ffi::EventLogMessage);
            Event::Log {
                level: text(m.level),
                prefix: text(m.prefix),
                text: text(m.text).trim_end().to_string(),
            }
        }
        _ => Event::Other,
    })
}

/// # Safety
/// `p` is null or a NUL-terminated string.
pub(super) unsafe fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_reasons_follow_client_h() {
        assert_eq!(EndReason::from_raw(0), EndReason::Eof);
        assert_eq!(EndReason::from_raw(2), EndReason::Stop);
        assert_eq!(EndReason::from_raw(4), EndReason::Error);
        // 1 was never assigned; unknown future reasons are not errors.
        assert_eq!(EndReason::from_raw(1), EndReason::Other);
        assert_eq!(EndReason::from_raw(99), EndReason::Other);
    }
}
