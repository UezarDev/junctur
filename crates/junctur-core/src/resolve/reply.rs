//! Reading the script's answers out of `Fusion.prefs`: Resolve 21's script state cannot write
//! files, but `fusion:SetPrefs` + `SavePrefs` can, so the script leaves `JuncturAck = "<id>"` and
//! `JuncturReply = "<id>:<hex>"` under `Global.JuncturBridge`, hex-encoded `key<TAB>value` lines
//! so nothing needs escaping. Names differ from auto-subs' same channel so neither reads the
//! other's.

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

/// The timeline and the caption template, as the `build` request finds them.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub name: String,
    /// Resolve's id for it (its name when Resolve gives none), so `place` can refuse a timeline
    /// switched to after `info`.
    pub id: String,
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
    /// Text+ inputs the template did not take (a user's own title may lack some): those clips are
    /// placed with the template's own value for them.
    pub unstyled: Vec<String>,
    /// The timeline they went on: the one the send made.
    pub timeline: String,
    /// Clips Junctur did not place that sit where new captions go on its tracks, found by a send
    /// asking first: when not zero, no caption was placed and the user is asked to replace, skip
    /// or use a new track. `blocked` is how many captions they are in the way of. Only an update
    /// meets these, since a new timeline's caption tracks are new.
    pub in_the_way: usize,
    pub blocked: usize,
    /// What the choice did: those clips deleted (`replacedClips`, beside `replaced`),
    /// the captions left out, the tracks added beside a speaker's for its new captions.
    pub replaced_clips: usize,
    pub skipped: usize,
    pub new_tracks: usize,
    /// Whether it replaced a timeline of that name.
    pub replaced: bool,
    /// Whether only the captions were updated on a timeline already there, its cut untouched.
    pub updated: bool,
    /// What went wrong making the timeline without stopping the send: an old timeline Resolve
    /// would not delete or a name it would not take; on an update, a cut that no longer matches.
    pub note: Option<String>,
}

/// The open project's timeline names, as the `check` request lists them: `timelines` = n,
/// then `timeline1` … `timelineN`.
pub fn timeline_names(a: &HashMap<String, String>) -> Vec<String> {
    let n: usize = a
        .get("timelines")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    (1..=n)
        .filter_map(|k| a.get(&format!("timeline{k}")).cloned())
        .collect()
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
        id: get("id").to_string(),
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
        unstyled: a
            .get("unstyled")
            .map(|u| {
                u.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        timeline: timeline.to_string(),
        in_the_way: n("inTheWay"),
        blocked: n("blocked"),
        replaced_clips: n("replaced"),
        skipped: n("skipped"),
        new_tracks: n("newTracks"),
        replaced: false,
        updated: false,
        note: None,
    }
}

/// Whether the script answering `check` is the version this Junctur needs or newer: it says which
/// (`script`), compared by length then text so "5z" < "5ab". An older one would ignore what a send
/// asks of it.
pub fn script_is_current(info: &HashMap<String, String>) -> bool {
    let version = |v: &str| (v.len(), v.to_string());
    info.get("script")
        .is_some_and(|v| version(v.trim()) >= version(SCRIPT))
}

/// The script version this Junctur needs (`junctur.lua`'s `SCRIPT`), bumped with it when a send
/// asks something an older script would ignore.
pub const SCRIPT: &str = "5ab";

/// The Fusion Titles a `templates` request found in the media pool, in the script's order
/// (`count`, then `t1..tN`), each trimmed by the rule the script and the send use
/// ([`crate::edit::title_name`]). A name the template setting could not hold (a quote, a `|` or a
/// control character, refused by [`crate::edit::Captions::check`]) is left out rather than
/// offered and then refused.
pub fn templates(a: &HashMap<String, String>) -> Vec<String> {
    let count: usize = a
        .get("count")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    (1..=count)
        .filter_map(|k| a.get(&format!("t{k}")))
        .map(|n| crate::edit::title_name(n).to_string())
        .filter(|n| !n.is_empty() && !n.contains(['"', '|']) && !n.chars().any(char::is_control))
        .collect()
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
            hex("placed\t3\nfirstError\t\nremoved\t1\nunstyled\tUnderline, Thickness2")
        );
        assert_eq!(pref(&text, "JuncturAck"), Some("2"));
        assert!(acked(&text, "2") && !acked(&text, "1"));
        let a = answer(&text, "2").unwrap();
        assert_eq!(a["placed"], "3");
        let s = sent(&a, "Main");
        assert_eq!((s.placed, s.removed, s.failed), (3, 1, 0));
        assert_eq!(s.first_error, None);
        assert_eq!(s.unstyled, ["Underline", "Thickness2"]);
        // An answer without the in-the-way fields, or one that found nothing in the way: all zero.
        assert_eq!(
            (
                s.in_the_way,
                s.blocked,
                s.replaced_clips,
                s.skipped,
                s.new_tracks
            ),
            (0, 0, 0, 0, 0)
        );
        // Another request's answer, or one cut off mid-write, is not this one's.
        assert!(answer(&text, "1").is_none());
        assert!(answer("JuncturReply = \"2:7061f\"", "2").is_none());
        assert!(answer("JuncturReply = \"2:zz\"", "2").is_none());
    }

    #[test]
    fn timeline_info_and_frame_rates() {
        let text = format!(
            "JuncturReply = \"5:{}\"",
            hex("name\tMain édit\nid\tT-1\nfps\t29.97\nstart\t108000\nend\t110000\nwidth\t1920\nheight\t1080\ntemplate\tJunctur Caption\ntemplateFps\t")
        );
        let t = timeline(&answer(&text, "5").unwrap()).unwrap();
        assert_eq!((t.name.as_str(), t.id.as_str()), ("Main édit", "T-1"));
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

    #[test]
    fn clips_in_the_way_and_what_was_done_with_them_are_read() {
        let text = format!(
            "JuncturReply = \"3:{}\"",
            hex("inTheWay\t2\nblocked\t3\nplaced\t0")
        );
        let s = sent(&answer(&text, "3").unwrap(), "Main");
        assert_eq!((s.in_the_way, s.blocked, s.placed), (2, 3, 0));
        let text = format!(
            "JuncturReply = \"4:{}\"",
            hex("placed\t5\nreplaced\t1\nskipped\t2\nnewTracks\t1")
        );
        let s = sent(&answer(&text, "4").unwrap(), "Main");
        assert_eq!((s.replaced_clips, s.skipped, s.new_tracks), (1, 2, 1));
    }

    #[test]
    fn only_a_script_of_this_round_or_newer_is_current() {
        let info = |v: Option<&str>| -> HashMap<String, String> {
            v.map(|v| [("script".to_string(), v.to_string())].into())
                .unwrap_or_default()
        };
        assert!(!script_is_current(&info(None)));
        assert!(!script_is_current(&info(Some("5m"))));
        assert!(!script_is_current(&info(Some("5n"))));
        assert!(!script_is_current(&info(Some("5o"))));
        assert!(!script_is_current(&info(Some("5z"))));
        assert!(!script_is_current(&info(Some("5aa"))));
        assert!(script_is_current(&info(Some("5ab"))));
        assert!(script_is_current(&info(Some("5ac"))));
        assert!(!script_is_current(&info(Some(""))));
    }

    #[test]
    fn the_script_says_the_version_junctur_needs() {
        // The version the script answers in `check` and the one Rust asks for are kept
        // in two languages; bumping one without the other fails here, not in Resolve.
        let script = include_str!("junctur.lua");
        assert!(
            script.contains(&format!("\nlocal SCRIPT = \"{SCRIPT}\"")),
            "junctur.lua's SCRIPT is not {SCRIPT:?}"
        );
    }

    #[test]
    fn the_templates_found_are_read_in_order_and_unusable_names_dropped() {
        let text = format!(
            "JuncturReply = \"7:{}\"",
            hex("count\t5\nt1\tLower third \nt2\tSay \"hi\"\nt3\tTítulo grande\nt4\t \nt5\ta|b")
        );
        let a = answer(&text, "7").unwrap();
        assert_eq!(templates(&a), ["Lower third", "Título grande"]);
        // ASCII spaces only, as the script trims a clip's name: the listed one is the one it finds.
        assert_eq!(
            crate::edit::title_name("\u{a0}Lower third \t"),
            "\u{a0}Lower third"
        );
        let none: HashMap<String, String> = [("count".to_string(), "0".to_string())].into();
        assert!(templates(&none).is_empty());
        assert!(templates(&HashMap::new()).is_empty());
    }

    #[test]
    fn the_projects_timelines_as_check_lists_them() {
        let text = format!(
            "JuncturReply = \"8:{}\"",
            hex("timelines\t2\ntimeline1\tMain\ntimeline2\trec (Junctur)")
        );
        let a = answer(&text, "8").unwrap();
        assert_eq!(timeline_names(&a), ["Main", "rec (Junctur)"]);
        assert!(timeline_names(&HashMap::new()).is_empty());
        // What a send made, beside the captions' counts: none unless the build says.
        let s = sent(&a, "rec (Junctur)");
        assert_eq!((s.replaced, s.note), (false, None));
    }
}
