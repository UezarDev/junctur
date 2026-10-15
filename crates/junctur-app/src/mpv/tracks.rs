//! mpv's `track-list` property (JSON when read as a string), narrowed to the audio tracks: their
//! titles name the mixer's strips.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    /// mpv's track id (`aid`), 1-based.
    pub id: i64,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub codec: Option<String>,
    pub channels: Option<i64>,
    pub selected: bool,
}

#[derive(Deserialize)]
struct RawTrack {
    #[serde(rename = "type")]
    kind: String,
    id: i64,
    title: Option<String>,
    lang: Option<String>,
    codec: Option<String>,
    #[serde(rename = "demux-channel-count")]
    channels: Option<i64>,
    #[serde(default)]
    selected: bool,
}

pub fn audio_tracks(json: &str) -> Result<Vec<AudioTrack>, String> {
    let raw: Vec<RawTrack> = serde_json::from_str(json).map_err(|e| format!("track-list: {e}"))?;
    Ok(raw
        .into_iter()
        .filter(|t| t.kind == "audio")
        .map(|t| AudioTrack {
            id: t.id,
            title: t.title,
            lang: t.lang,
            codec: t.codec,
            channels: t.channels,
            selected: t.selected,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_audio_tracks_in_order() {
        let json = r#"[
            {"id":1,"type":"video","codec":"h264","selected":true},
            {"id":1,"type":"audio","codec":"aac","title":"Mic","demux-channel-count":2,"selected":true},
            {"id":2,"type":"audio","codec":"aac","lang":"spa"},
            {"id":1,"type":"sub","codec":"subrip"}
        ]"#;
        let t = audio_tracks(json).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].title.as_deref(), Some("Mic"));
        assert_eq!(t[0].channels, Some(2));
        assert!(t[0].selected);
        assert_eq!(t[1].id, 2);
        assert_eq!(t[1].lang.as_deref(), Some("spa"));
        assert!(!t[1].selected);
    }

    #[test]
    fn empty_and_bad_lists() {
        assert_eq!(audio_tracks("[]").unwrap(), vec![]);
        assert!(audio_tracks("not json").is_err());
    }
}
