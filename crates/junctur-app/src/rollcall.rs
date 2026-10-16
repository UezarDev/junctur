//! The speaker roll call in the app (phase 7a, a prototype): the cards the full-window screen shows
//! (one per speaker, with the seconds to play and the track to play them on), the stretches it asks
//! about, and whether it was already shown for a recording. Built from the project's edit list,
//! speakers stage and transcript ([`junctur_core::rollcall`]); the renames, enrolments and "who is
//! this" answers go through the player like any speaker change.

use std::path::{Path, PathBuf};

use junctur_core::edit::default_name;
use junctur_core::pipeline::{Method, Project};
use junctur_core::rollcall::{self, Unsure};
use junctur_core::transcribe;
use serde::{Deserialize, Serialize};

/// In the project folder: the roll call was shown for this recording, so it does not open by itself
/// again.
const FILE: &str = "rollcall.json";
/// "Remember this voice" enrols at most this many seconds of a speaker.
pub const REMEMBER_S: f64 = 30.0;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: u32,
    pub name: String,
    pub color: String,
    /// Still the name Junctur gave ("Speaker 2"): the name box starts empty.
    pub unnamed: bool,
    /// A remembered person: renaming them renames the voice in every recording.
    pub person: bool,
    /// The seconds to play, and the audio track (0-based) to play them on.
    pub sample: Option<(f64, f64)>,
    pub track: Option<usize>,
    /// Seconds this speaker talks in the recording.
    pub seconds: f64,
    /// What "Remember this voice" enrols ([`rollcall::voice_stretches`]). Sent back as seconds, not
    /// by id: every enrolment labels the speakers again, and their ids may then renumber.
    pub voice: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollCallView {
    pub cards: Vec<Card>,
    pub unsure: Vec<Unsure>,
    /// Voices can be remembered here (speakers told apart by voice, heard by this Junctur).
    pub can_remember: bool,
    /// The roll call was already shown for this recording.
    pub seen: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Seen {
    seen: bool,
}

/// The roll call of `file` as its project has it now.
pub fn view(file: &Path, cache_dir: Option<PathBuf>) -> Result<RollCallView, String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let seen = std::fs::read(p.dir.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Seen>(&b).ok())
        .is_some_and(|s| s.seen);
    let Ok(list) = p.edit() else {
        return Ok(RollCallView {
            seen,
            ..Default::default()
        });
    };
    let s = &list.speakers;
    let found = p.speakers();
    let heard_track = found
        .as_ref()
        .and_then(|f| f.heard.as_ref())
        .map(|h| h.track);
    let can_remember = found
        .as_ref()
        .is_some_and(|f| f.method == Method::Voices && !f.heard_by_an_older_junctur());
    let person = |id: u32| {
        found
            .as_ref()
            .and_then(|f| f.people.get(id.checked_sub(1)? as usize))
            .is_some_and(Option::is_some)
    };
    let cards = s
        .names
        .iter()
        .map(|n| Card {
            id: n.id,
            name: n.name.clone(),
            color: n.color.clone(),
            unnamed: n.name == default_name(n.id),
            person: person(n.id),
            sample: rollcall::sample(s, n.id),
            track: n.track.or(heard_track),
            voice: rollcall::voice_stretches(s, n.id, REMEMBER_S),
            seconds: s
                .turns
                .iter()
                .filter(|t| t.speaker == n.id)
                .map(|t| t.end - t.start)
                .sum(),
        })
        .collect();
    let mut words: Vec<(usize, f64, f64)> = transcribe::load(&p.dir)
        .map(|t| t.segments)
        .or_else(|| transcribe::so_far(&p.dir))
        .unwrap_or_default()
        .iter()
        .flat_map(|seg| seg.words.iter().map(move |w| (seg.track, w.start, w.end)))
        .collect();
    words.sort_by(|a, b| a.1.total_cmp(&b.1));
    Ok(RollCallView {
        cards,
        unsure: rollcall::unsure(s, &words),
        can_remember,
        seen,
    })
}

/// Marks the roll call shown for `file`.
pub fn seen(file: &Path, cache_dir: Option<PathBuf>) -> Result<(), String> {
    let p = Project::open(file, cache_dir, None).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(&Seen { seen: true }).map_err(|e| e.to_string())?;
    junctur_core::output::write_replacing(&p.dir.join(FILE), bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use junctur_core::edit::{EditList, Speakers, Turn};

    #[test]
    fn cards_come_from_the_list_and_the_roll_call_is_seen_once() {
        let dir = std::env::temp_dir().join(format!("junctur-app-rollcall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("rec.mkv");
        std::fs::write(&source, b"").unwrap();
        let cache = dir.join("cache");
        let p = Project::open(&source, Some(cache.clone()), None).unwrap();
        // No edit list yet: nothing to show, not an error.
        let v = view(&source, Some(cache.clone())).unwrap();
        assert!(v.cards.is_empty() && !v.seen);
        let t = |start: f64, end: f64, speaker: u32| Turn {
            start,
            end,
            speaker,
        };
        let mut speakers = Speakers::new(
            vec![t(0.0, 10.0, 1), t(10.5, 11.5, 2), t(12.0, 20.0, 1)],
            &[None, None],
            None,
        );
        speakers.rename(1, "Delay");
        let list = EditList {
            source: p.file.clone(),
            duration: 30.0,
            settings: Default::default(),
            keep: vec![],
            speakers,
            captions: None,
            fixes: vec![],
            times: vec![],
            windows: vec![],
        };
        junctur_core::edit::save(&p.edit_path(), &list).unwrap();
        let v = view(&source, Some(cache.clone())).unwrap();
        assert_eq!(v.cards.len(), 2);
        assert_eq!(
            (v.cards[0].name.as_str(), v.cards[0].unnamed),
            ("Delay", false)
        );
        assert!(v.cards[1].unnamed);
        assert_eq!(v.cards[0].sample, Some((3.5, 6.5)));
        assert_eq!(v.cards[0].seconds, 18.0);
        // No speakers stage result: no voices to remember, no track known.
        assert!(!v.can_remember && v.cards[0].track.is_none());
        assert_eq!(v.unsure.len(), 1);
        seen(&source, Some(cache.clone())).unwrap();
        assert!(view(&source, Some(cache)).unwrap().seen);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
