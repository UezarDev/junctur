//! "Find all like this" in the app (7b, a prototype): what the Like this tab and lane show of a
//! recording's search ([`junctur_core::like`]), the user's examples and settings saved as they
//! change, the search itself (the track's spectrogram built the first time), and the proposals
//! turned into edits of the list in play. A module because the view gathers from the project's
//! like file, its track report and its edit list, and the answers edit through the player.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use junctur_core::labels::busy;
use junctur_core::like::{self, mel, Example, Kind, Like, Searched, SendKind};
use junctur_core::pipeline::{self, Project};
use junctur_core::tracks::TrackReport;
use serde::{Deserialize, Serialize};

use crate::lock::lock;
use crate::player::Core;

/// What the page shows of a recording's search.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LikeView {
    #[serde(flatten)]
    pub like: Like,
    /// The track a search reads: the user's pick, else the game track, else the mix, else 1.
    pub effective_track: usize,
    /// The game track (else the mix) the track roles found, for the picker to name.
    pub game_track: Option<usize>,
    pub tracks: usize,
    pub duration: f64,
    /// The tracks whose spectrogram is kept already (a search on them starts at once).
    pub ready: Vec<usize>,
}

fn open(file: &Path, cache_dir: Option<PathBuf>) -> Result<Project, String> {
    Project::open(file, cache_dir, None).map_err(|e| e.to_string())
}

fn load(p: &Project) -> Result<Like, String> {
    like::load(&p.dir).map_err(|e| e.to_string())
}

fn save(p: &Project, l: &Like) -> Result<(), String> {
    like::save(&p.dir, l).map_err(|e| e.to_string())
}

/// Every read-change-write of a like file goes through here, one at a time: the page saves its
/// slider while a search runs for seconds, and either one's copy would otherwise overwrite the
/// other's (an accepted edit's undo record among them).
static FILES: Mutex<()> = Mutex::new(());

/// `f` on `p`'s like file as it is now, saved when it succeeds; the view after.
fn change(
    p: &Project,
    f: impl FnOnce(&mut Like) -> Result<(), String>,
) -> Result<LikeView, String> {
    let _held = lock(&FILES);
    let mut l = load(p)?;
    f(&mut l)?;
    save(p, &l)?;
    Ok(of(p, l))
}

/// The recording's length: the track report's, else the edit list's, else 0 (not known).
fn length(p: &Project, report: Option<&TrackReport>) -> f64 {
    report
        .map(|r| r.duration)
        .filter(|d| *d > 0.0)
        .or_else(|| p.edit().ok().map(|l| l.duration))
        .unwrap_or(0.0)
}

/// The view of `p` with `like` as it is now.
fn of(p: &Project, like: Like) -> LikeView {
    let report = pipeline::cached_tracks(p);
    let game_track = report.as_ref().and_then(busy::track).map(|t| t.track);
    let tracks = report.as_ref().map_or(0, |r| r.tracks.len());
    let duration = length(p, report.as_ref());
    let ready = (0..tracks.max(1))
        .filter(|&t| mel::path(&p.dir, t).is_file())
        .collect();
    LikeView {
        effective_track: like.track.or(game_track).unwrap_or(0),
        like,
        game_track,
        tracks,
        duration,
        ready,
    }
}

pub fn view(file: &Path, cache_dir: Option<PathBuf>) -> Result<LikeView, String> {
    let p = open(file, cache_dir)?;
    let l = load(&p)?;
    Ok(of(&p, l))
}

/// What the page owns of a search: saved as it changes, read back by the next search.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub track: Option<usize>,
    pub examples: Vec<Example>,
    pub range: Option<(f64, f64)>,
    pub threshold: f64,
    pub before: f64,
    pub after: f64,
    pub confirmed: Vec<f64>,
}

pub fn set(file: &Path, cache_dir: Option<PathBuf>, s: Settings) -> Result<LikeView, String> {
    let p = open(file, cache_dir)?;
    change(&p, |l| {
        // Another track's or range's matches are not this search's: they go, and a proposal made
        // of them with them, so nothing acts on a search of something else.
        if l.track != s.track || l.range != s.range {
            l.matches.clear();
            l.searched = None;
            l.proposal = None;
        }
        l.track = s.track;
        l.examples = s.examples;
        l.range = s.range;
        l.threshold = s.threshold.clamp(0.0, 1.0);
        l.before = s.before.clamp(0.0, 60.0);
        l.after = s.after.clamp(0.0, 60.0);
        l.confirmed = s.confirmed;
        Ok(())
    })
}

/// Searches `file` with its saved examples: the track's spectrogram read (built and kept the first
/// time, a few seconds for two hours), the matches saved with how long it took. The search runs on
/// a copy without holding the file; its matches go into the file as it is then, unless the track
/// or range changed meanwhile (they would be another search's).
pub fn search(file: &Path, cache_dir: Option<PathBuf>) -> Result<LikeView, String> {
    let p = open(file, cache_dir)?;
    let now = {
        let _held = lock(&FILES);
        load(&p)?
    };
    let v = of(&p, now);
    let started = Instant::now();
    let (spec, built) =
        mel::of_track(&p.dir, &p.file, v.effective_track, v.duration).map_err(|e| e.to_string())?;
    let found = like::find(&spec, &v.like.examples, v.like.range).map_err(|e| e.to_string())?;
    let searched = Searched {
        track: v.effective_track,
        range: v.like.range,
        examples: v.like.examples.len(),
        seconds: (started.elapsed().as_secs_f64() * 100.0).round() / 100.0,
        built,
        capped: found.len() >= like::MAX_MATCHES,
    };
    change(&p, |l| {
        if l.track != v.like.track || l.range != v.like.range {
            return Err("the track or the range changed while searching: search again".into());
        }
        l.found(found, searched);
        Ok(())
    })
}

/// What to do with every shown match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Act {
    KeepAround,
    CutEach,
    Mark,
    Clip,
}

/// Proposes `act` (an edit waiting for Accept), or sends the matches to the Moments list (where
/// they wait as moments do).
pub fn act(file: &Path, cache_dir: Option<PathBuf>, act: Act) -> Result<LikeView, String> {
    let p = open(file, cache_dir)?;
    let duration = length(&p, pipeline::cached_tracks(&p).as_ref());
    change(&p, |l| {
        let r = match act {
            Act::KeepAround => l.propose(Kind::KeepAround, duration).map(|_| ()),
            Act::CutEach => l.propose(Kind::CutEach, duration).map(|_| ()),
            Act::Mark => l.send(SendKind::Mark, duration).map(|_| ()),
            Act::Clip => l.send(SendKind::Clip, duration).map(|_| ()),
        };
        r.map_err(|e| e.to_string())
    })
}

/// The user's answer to a proposal, or taking back what was done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Answer {
    /// The proposal's edits, as one undo step.
    Accept,
    Discard,
    /// An accepted proposal's range put back as the list had it.
    Undo,
    /// A batch sent to the Moments list taken out of it.
    TakeBack,
}

/// Applies `answer` (to applied or sent `id` for [`Answer::Undo`] and [`Answer::TakeBack`]) on the
/// open recording `file` as `core` plays it: the edits first, then the like file, so a refused
/// edit records nothing.
pub fn answer(
    core: &Core,
    file: Option<&str>,
    cache_dir: Option<PathBuf>,
    answer: Answer,
    id: Option<u64>,
) -> Result<LikeView, String> {
    let path = file.ok_or("no recording is open")?;
    let p = open(Path::new(path), cache_dir)?;
    change(&p, |l| {
        match answer {
            Answer::Accept => {
                let list = p.edit().map_err(|e| e.to_string())?;
                let edits = l.accept(&list.keep).ok_or("nothing is proposed")?;
                crate::moments::apply(core, file, &edits)?;
            }
            Answer::Discard => l.proposal = None,
            Answer::Undo => {
                let edits = l.undo(id.ok_or("which one to undo")?);
                if edits.is_empty() {
                    return Err("that edit is no longer there to undo".into());
                }
                crate::moments::apply(core, file, &edits)?;
            }
            Answer::TakeBack => {
                if !l.take_back(id.ok_or("which one to take back")?) {
                    return Err("those moments were taken back already".into());
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_save_and_a_new_range_drops_the_proposal() {
        let dir = std::env::temp_dir().join(format!("junctur-app-like-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("rec.mkv");
        std::fs::write(&source, b"").unwrap();
        let cache = dir.join("cache");
        let p = Project::open(&source, Some(cache.clone()), None).unwrap();
        let v = view(&source, Some(cache.clone())).unwrap();
        assert_eq!(
            (v.effective_track, v.game_track, v.like.threshold),
            (0, None, like::THRESHOLD)
        );
        let mut l = Like {
            matches: vec![like::Match {
                start: 5.0,
                end: 5.5,
                point: 5.0,
                score: 0.9,
            }],
            ..Default::default()
        };
        l.propose(Kind::CutEach, 10.0).unwrap();
        like::save(&p.dir, &l).unwrap();
        let s = |range| Settings {
            track: Some(2),
            examples: vec![Example::click(5.25, true)],
            range,
            threshold: 1.5,
            before: 0.2,
            after: 0.3,
            confirmed: vec![],
        };
        let v = set(&source, Some(cache.clone()), s(None)).unwrap();
        assert_eq!(
            (v.effective_track, v.like.threshold, v.like.before),
            (2, 1.0, 0.2)
        );
        assert!(v.like.proposal.is_none(), "another track's proposal goes");
        assert!(v.like.matches.is_empty(), "and its matches");
        // A search on track 3 found it.
        let mut l = like::load(&p.dir).unwrap();
        l.matches = vec![like::Match {
            start: 5.0,
            end: 5.5,
            point: 5.0,
            score: 0.9,
        }];
        like::save(&p.dir, &l).unwrap();
        let v = act(&source, Some(cache.clone()), Act::Mark);
        assert!(v.is_err(), "nothing is shown at threshold 1: {v:?}");
        let v = set(
            &source,
            Some(cache.clone()),
            Settings {
                threshold: 0.5,
                ..s(None)
            },
        )
        .unwrap();
        assert_eq!(v.like.examples.len(), 1);
        let v = act(&source, Some(cache.clone()), Act::Clip).unwrap();
        assert_eq!(v.like.sent.len(), 1);
        // The sent clip is a moment now.
        let m = crate::moments::view(&source, Some(cache)).unwrap();
        assert_eq!(m.moments.len(), 1);
        assert_eq!(
            (m.moments[0].moment.start, m.moments[0].moment.end),
            (4.8, 5.3)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
