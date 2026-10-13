//! Reading the script's answers out of `Fusion.prefs` (5g). Resolve 21's script state cannot write
//! files, but `fusion:SetPrefs` + `SavePrefs` writes Fusion's preferences to disk, so the script
//! leaves `JuncturAck = "<id>"` (request read) and `JuncturReply = "<id>:<hex>"` there, under
//! `Global.JuncturBridge`. The reply is `key<TAB>value` lines, hex-encoded so nothing in it needs
//! escaping inside the prefs file's quotes. auto-subs found the same channel (`resolve_bridge.rs`);
//! the names differ so the two never read each other's answers.

use std::collections::HashMap;

/// The value of `key = "…"` in prefs text: the last one, and only where `key` is a whole word, so
/// `JuncturReply` never matches inside a longer name.
pub fn pref<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key} = \"");
    let mut found = None;
    let mut from = 0;
    while let Some(at) = text[from..].find(&needle) {
        let at = from + at;
        let start = at + needle.len();
        let Some(len) = text[start..].find('"') else {
            break;
        };
        let whole = at == 0
            || !text.as_bytes()[at - 1].is_ascii_alphanumeric() && text.as_bytes()[at - 1] != b'_';
        if whole {
            found = Some(&text[start..start + len]);
        }
        from = start + len;
    }
    found
}

/// Whether the script has read request `id`.
pub fn acked(text: &str, id: &str) -> bool {
    pref(text, "JuncturAck") == Some(id)
}

/// Request `id`'s answer, once it is there whole: `None` for another request's, or a file caught
/// half-written (bad hex), so the caller keeps waiting.
pub fn answer(text: &str, id: &str) -> Option<HashMap<String, String>> {
    let (rid, hex) = pref(text, "JuncturReply")?.split_once(':')?;
    if rid != id {
        return None;
    }
    let bytes = unhex(hex)?;
    // Names Resolve hands back can be in the Windows code page rather than UTF-8; they are only
    // shown, so a lossy read keeps the answer.
    let text = String::from_utf8_lossy(&bytes);
    Some(
        text.lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The open timeline and the caption template, as the `info` request finds them.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub name: String,
    /// Frames per second, as an exact rate (29.97 is 30000/1001).
    pub fps: f64,
    /// The timeline's first frame (Resolve starts at 01:00:00:00, 86400 at 24 fps).
    pub start: i64,
    pub end: i64,
    pub width: u32,
    pub height: u32,
    /// The template's clip name and frame rate: a clip's length is given in the template's frames.
    pub template: String,
    pub template_fps: f64,
}

/// What a `place` request did.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    /// New clips placed.
    pub placed: usize,
    /// Clips already there from an earlier send, restyled in place.
    pub restyled: usize,
    /// Clips of an earlier send no caption matches any more (an edit moved or removed it), deleted.
    pub removed: usize,
    pub failed: usize,
    pub first_error: Option<String>,
    /// The timeline they went on.
    pub timeline: String,
}

/// The script's own error, if it answered with one.
pub fn error(a: &HashMap<String, String>) -> Option<String> {
    a.get("error").cloned()
}

pub fn timeline(a: &HashMap<String, String>) -> Result<Timeline, String> {
    let get = |k: &str| a.get(k).map(String::as_str).unwrap_or("");
    let num = |k: &str| {
        get(k)
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("Resolve answered {k} = {:?}", get(k)))
    };
    let fps = rate(get("fps")).ok_or_else(|| format!("Resolve's frame rate {:?}", get("fps")))?;
    Ok(Timeline {
        name: get("name").to_string(),
        fps,
        start: num("start")?.round() as i64,
        end: num("end")?.round() as i64,
        width: num("width")?.round() as u32,
        height: num("height")?.round() as u32,
        template: get("template").to_string(),
        // A title with no frame rate of its own plays at the timeline's.
        template_fps: rate(get("templateFps")).unwrap_or(fps),
    })
}

pub fn sent(a: &HashMap<String, String>, timeline: &str) -> Sent {
    let n = |k: &str| a.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    Sent {
        placed: n("placed"),
        restyled: n("restyled"),
        removed: n("removed"),
        failed: n("failed"),
        first_error: a.get("firstError").filter(|e| !e.is_empty()).cloned(),
        timeline: timeline.to_string(),
    }
}

/// Resolve's frame rate setting (`"25"`, `"29.97"`, `"23.976"`) as the rate it stands for: the
/// NTSC rates are n×1000/1001, which "29.97" rounds; a frame off per ~9 minutes otherwise.
pub fn rate(s: &str) -> Option<f64> {
    let v: f64 = s.trim().parse().ok()?;
    if !(v.is_finite() && v > 0.0) {
        return None;
    }
    let ntsc = (v * 1.001).round();
    Some(
        if (ntsc * 1000.0 / 1001.0 - v).abs() < 0.005 && v.fract() != 0.0 {
            ntsc * 1000.0 / 1001.0
        } else {
            v
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn prefs_values_are_read_by_whole_key_last_one_wins() {
        let text = format!(
            "{{ Global = {{ AutoSubsBridge = {{ Ack = \"9\", }}, JuncturBridge = {{ \
             JuncturAck = \"1\", NotJuncturAck = \"x\", JuncturAck = \"2\", \
             JuncturReply = \"2:{}\", }}, }} }}",
            hex("placed\t3\nfirstError\t\nremoved\t1")
        );
        assert_eq!(pref(&text, "JuncturAck"), Some("2"));
        assert!(acked(&text, "2") && !acked(&text, "1"));
        let a = answer(&text, "2").unwrap();
        assert_eq!(a["placed"], "3");
        let s = sent(&a, "Main");
        assert_eq!((s.placed, s.removed, s.failed), (3, 1, 0));
        assert_eq!(s.first_error, None);
        // Another request's answer, or one cut off mid-write, is not this one's.
        assert!(answer(&text, "1").is_none());
        assert!(answer("JuncturReply = \"2:7061f\"", "2").is_none());
        assert!(answer("JuncturReply = \"2:zz\"", "2").is_none());
    }

    #[test]
    fn timeline_info_and_frame_rates() {
        let text = format!(
            "JuncturReply = \"5:{}\"",
            hex("name\tMain édit\nfps\t29.97\nstart\t108000\nend\t110000\nwidth\t1920\nheight\t1080\ntemplate\tJunctur Caption\ntemplateFps\t")
        );
        let t = timeline(&answer(&text, "5").unwrap()).unwrap();
        assert_eq!(t.name, "Main édit");
        assert!((t.fps - 30000.0 / 1001.0).abs() < 1e-9);
        assert_eq!(
            (t.start, t.end, t.width, t.height),
            (108000, 110000, 1920, 1080)
        );
        assert_eq!(t.template_fps, t.fps);
        assert_eq!(rate("25"), Some(25.0));
        assert_eq!(rate("24"), Some(24.0));
        assert!((rate("23.976").unwrap() - 24000.0 / 1001.0).abs() < 1e-9);
        assert!((rate("59.94").unwrap() - 60000.0 / 1001.0).abs() < 1e-9);
        assert_eq!(rate("0"), None);
        assert_eq!(rate("fast"), None);
        let mut bad = answer(&text, "5").unwrap();
        bad.insert("width".into(), "wide".into());
        assert!(timeline(&bad).is_err());
        let e: HashMap<String, String> = [("error".to_string(), "No timeline".to_string())].into();
        assert_eq!(error(&e).as_deref(), Some("No timeline"));
    }
}
