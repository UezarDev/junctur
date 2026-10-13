//! Every audio track heard at once in the preview (2e), each with a meter and a mute. A module of
//! its own because it is mpv filter graphs, a rule for the default mutes and a small file, none of
//! which needs mpv to test.
//!
//! mpv plays the tracks together through `lavfi-complex`: each one is made 48 kHz stereo and they
//! are merged into one stream of 2 channels per track, so the `af` chain after it still sees every
//! track apart. There an `astats` (`@meter`) measures each channel, which mpv exposes as
//! `af-metadata/meter`, and an `amix` (`@mix`) sums the tracks with one weight each: a mute is a
//! weight of 0, changed in place by `af-command` (amix's `weights` is a runtime command), so a click
//! never rebuilds the chain or leaves a gap. Mutes in `pan` (no runtime command), `volume` filters
//! in `lavfi-complex` (a rebuild per click), or meters inside `lavfi-complex` (mpv gives no metadata
//! from it) were each ruled out against the bundled libmpv.
//!
//! The graphs go in each `loadfile`'s own options, so they last one load and a recording with other
//! tracks never meets a stale graph. Mutes are playback only: nothing here reaches the edit list or
//! a render.

use std::path::{Path, PathBuf};

use junctur_core::ffmpeg;
use junctur_core::pipeline::{self, Project};
use junctur_core::tracks::{Role, TrackReport};
use serde::{Deserialize, Serialize};

/// Tracks taking part: 2 channels each, and mpv carries at most 64 (OBS records at most 6).
pub const MAX_TRACKS: usize = 32;

/// The file in a recording's project folder that remembers its mutes.
pub const FILE: &str = "mutes.json";

/// The `af` labels and the filter `af-command` talks to.
pub const METER: &str = "meter";
pub const MIX: &str = "mix";
pub const MIX_FILTER: &str = "amix";

/// Where the mutes in play came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// No track roles yet (the recording is not analysed): track 1 only, as before 2e.
    #[default]
    Provisional,
    /// From the roles: the mix alone, or every separate track.
    Roles,
    /// At least one track is muted or heard because the user said so (read from or saved to
    /// [`FILE`]); the others follow the roles.
    Remembered,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    /// 0-based audio ordinal (mpv's `aid` is this + 1).
    pub index: usize,
    pub muted: bool,
    /// The user set this track's mute: the roles' defaults no longer move it.
    pub set: bool,
    /// The role of the distinct track this is, when the analysis got that far.
    pub role: Option<Role>,
    /// The track this one decodes the same as (or nearly), when it is a copy.
    pub copy_of: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MixerState {
    pub tracks: Vec<Track>,
    pub source: Source,
}

/// The open recording's tracks and mutes.
#[derive(Debug, Clone, Default)]
pub struct Mixer {
    tracks: Vec<Track>,
    /// A track report has been taken (its roles are in `tracks`).
    roles: bool,
    /// The recording's project (its folder holds [`FILE`]); `None` when it could not be opened.
    project: Option<Project>,
}

/// [`FILE`]'s contents. `set` says which mutes are the user's; a file without it counts all.
#[derive(Serialize, Deserialize)]
struct Remembered {
    muted: Vec<bool>,
    #[serde(default)]
    set: Option<Vec<bool>>,
}

/// The mutes to write to [`FILE`], taken under the mixer's lock and written after it.
pub struct Memory {
    path: PathBuf,
    text: Vec<u8>,
}

impl Memory {
    pub fn write(&self) -> Result<(), String> {
        junctur_core::output::write_replacing(&self.path, &self.text)
            .map_err(|e| format!("remembering the mutes in {}: {e}", self.path.display()))
    }
}

impl Mixer {
    /// The mixer for recording `file` with project folder `dir`: its track count from the track
    /// report the pipeline cached, else from ffprobe (a process start, so callers do this outside
    /// their locks); none when neither can say (mpv then plays the file as it would).
    pub fn open(file: &str, dir: Option<PathBuf>) -> Self {
        let path = Path::new(file);
        let project = dir.and_then(|d| Project::open(path, None, Some(d)).ok());
        let report = project.as_ref().and_then(pipeline::cached_tracks);
        let count = match &report {
            Some(r) if !r.tracks.is_empty() => r.tracks.len(),
            _ => ffmpeg::probe(path).map_or(0, |p| p.audio.len()),
        };
        Self::new(count, report.as_ref(), project)
    }

    /// Whether this is the mixer of `file` (absolute, as the analysis names it).
    pub fn is_for(&self, file: &Path) -> bool {
        self.project.as_ref().is_some_and(|p| p.file == file)
    }

    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    /// `count` audio tracks (capped at [`MAX_TRACKS`]): the mutes the user set in `project`, the
    /// others as the roles in `report` say, else track 1 only.
    pub fn new(count: usize, report: Option<&TrackReport>, project: Option<Project>) -> Self {
        let count = count.min(MAX_TRACKS);
        let mut m = Self {
            tracks: (0..count)
                .map(|index| Track {
                    index,
                    muted: index > 0,
                    set: false,
                    role: None,
                    copy_of: None,
                })
                .collect(),
            roles: false,
            project,
        };
        if let Some(r) = m.project.as_ref().and_then(|p| remembered(&p.dir, count)) {
            let set = r.set.unwrap_or_else(|| vec![true; count]);
            for ((t, muted), set) in m.tracks.iter_mut().zip(r.muted).zip(set) {
                if set {
                    t.muted = muted;
                    t.set = true;
                }
            }
        }
        if let Some(r) = report {
            m.learn(r);
        }
        m
    }

    pub fn state(&self) -> MixerState {
        let source = if self.tracks.iter().any(|t| t.set) {
            Source::Remembered
        } else if self.roles {
            Source::Roles
        } else {
            Source::Provisional
        };
        MixerState {
            tracks: self.tracks.clone(),
            source,
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Takes the roles from `report` (a first analysis, or a re-analysis that changed them) and
    /// their default mutes for every track the user has not set. Returns whether anything shown
    /// changed: a role, a copy, or a mute.
    pub fn learn(&mut self, report: &TrackReport) -> bool {
        let before = self.tracks.clone();
        for t in &mut self.tracks {
            t.role = None;
            t.copy_of = None;
        }
        for l in &report.logical {
            let Some(&first) = l.members.first() else {
                continue;
            };
            for (k, &m) in l.members.iter().enumerate() {
                if let Some(t) = self.tracks.get_mut(m) {
                    t.role = Some(l.role);
                    t.copy_of = (k > 0).then_some(first);
                }
            }
        }
        let heard = default_heard(report, self.tracks.len());
        for (t, h) in self.tracks.iter_mut().zip(heard) {
            if !t.set {
                t.muted = !h;
            }
        }
        let first = !self.roles;
        self.roles = true;
        first || self.tracks != before
    }

    /// Mutes or unmutes track `index` as the user asked; returns the mutes to remember, for the
    /// caller to write outside its lock (`None` without a project folder).
    pub fn set_muted(&mut self, index: usize, muted: bool) -> Result<Option<Memory>, String> {
        let t = self
            .tracks
            .get_mut(index)
            .ok_or_else(|| format!("no audio track {}", index + 1))?;
        t.muted = muted;
        t.set = true;
        Ok(self.memory())
    }

    /// The mutes as [`FILE`] holds them.
    pub fn memory(&self) -> Option<Memory> {
        let dir = &self.project.as_ref()?.dir;
        let r = Remembered {
            muted: self.tracks.iter().map(|t| t.muted).collect(),
            set: Some(self.tracks.iter().map(|t| t.set).collect()),
        };
        Some(Memory {
            path: dir.join(FILE),
            text: serde_json::to_vec(&r).ok()?,
        })
    }

    /// amix's `weights`: 1 for a track heard, 0 for a muted one.
    pub fn weights(&self) -> String {
        let w: Vec<&str> = self
            .tracks
            .iter()
            .map(|t| if t.muted { "0" } else { "1" })
            .collect();
        w.join(" ")
    }

    /// The per-file `loadfile` options (`lavfi-complex=...,af=...`) that play every track with the
    /// mutes as they are; `None` without audio, which mpv then plays as it would.
    pub fn load_options(&self) -> Option<String> {
        let n = self.tracks.len();
        if n == 0 {
            return None;
        }
        const FORMAT: &str = "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo";
        const HEARD: &str =
            "astats=metadata=1:reset=1:measure_perchannel=none:measure_overall=Peak_level";
        let mix = format!(
            "{MIX_FILTER}=inputs={n}:normalize=0:weights={},{HEARD}",
            self.weights()
        );
        let (complex, mix) = if n == 1 {
            (format!("[aid1]{FORMAT}[ao]"), mix)
        } else {
            let each: Vec<String> = (0..n)
                .map(|i| format!("[aid{}]{FORMAT}[a{i}]", i + 1))
                .collect();
            let merged: String = (0..n).map(|i| format!("[a{i}]")).collect();
            let split: String = (0..n).map(|i| format!("[s{i}]")).collect();
            let pans: Vec<String> = (0..n)
                .map(|i| format!("[s{i}]pan=stereo|c0=c{}|c1=c{}[t{i}]", 2 * i, 2 * i + 1))
                .collect();
            let summed: String = (0..n).map(|i| format!("[t{i}]")).collect();
            (
                format!("{};{merged}amerge=inputs={n}[ao]", each.join(";")),
                format!("asplit={n}{split};{};{summed}{mix}", pans.join(";")),
            )
        };
        let af = format!(
            "@{METER}:lavfi=[astats=metadata=1:reset=1:measure_perchannel=Peak_level:measure_overall=none],@{MIX}:lavfi=[{mix}]"
        );
        Some(format!(
            "lavfi-complex={},af={}",
            escape(&complex),
            escape(&af)
        ))
    }
}

/// mpv's `%<bytes>%` quoting: the graphs hold commas, `=` and brackets.
fn escape(s: &str) -> String {
    format!("%{}%{s}", s.len())
}

/// Which tracks the roles say to hear (decision 2): with a mix, the mix alone; without one, every
/// separate track. Copies are never heard: a copy only doubles its original.
fn default_heard(report: &TrackReport, n: usize) -> Vec<bool> {
    let mut heard = vec![false; n];
    let firsts = report.logical.iter().filter_map(|l| {
        let first = *l.members.first()?;
        Some((first, l.role))
    });
    let mix = report
        .logical
        .iter()
        .find(|l| l.role == Role::Mix)
        .and_then(|l| l.members.first());
    match mix {
        Some(&m) => {
            if let Some(h) = heard.get_mut(m) {
                *h = true;
            }
        }
        None => {
            for (first, _) in firsts {
                if let Some(h) = heard.get_mut(first) {
                    *h = true;
                }
            }
        }
    }
    // A report that names none of the file's tracks (it never should) leaves track 1 heard.
    if !heard.contains(&true) {
        if let Some(h) = heard.first_mut() {
            *h = true;
        }
    }
    heard
}

/// The mutes remembered in project folder `dir`, when there are `count` of them.
fn remembered(dir: &Path, count: usize) -> Option<Remembered> {
    let r: Remembered = serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()?;
    let fits = r.muted.len() == count && r.set.as_ref().is_none_or(|s| s.len() == count);
    fits.then_some(r)
}

/// The level silence is reported at (astats says `-inf`, which JSON cannot carry).
pub const FLOOR_DB: f32 = -120.0;

/// Each track's peak in dBFS from `af-metadata/meter` (`lavfi.astats.<channel>.Peak_level`,
/// channels counted from 1, two per track): the louder of its pair, at least [`FLOOR_DB`]; `None`
/// where it is missing.
pub fn levels(json: &str, tracks: usize) -> Vec<Option<f32>> {
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(json).unwrap_or_default();
    let channel = |c: usize| -> Option<f32> {
        let v = map.get(&format!("lavfi.astats.{c}.Peak_level"))?.as_str()?;
        // astats writes `-inf` for silence, which parses as negative infinity.
        v.parse::<f32>()
            .ok()
            .filter(|x| !x.is_nan())
            .map(|x| x.max(FLOOR_DB))
    };
    (0..tracks)
        .map(|t| match (channel(2 * t + 1), channel(2 * t + 2)) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        })
        .collect()
}

/// What is heard after the mutes, in dBFS, from `af-metadata/mix`.
pub fn heard(json: &str) -> Option<f32> {
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json).ok()?;
    let v: f32 = map
        .get("lavfi.astats.Overall.Peak_level")?
        .as_str()?
        .parse()
        .ok()?;
    Some(v.max(FLOOR_DB))
}

#[cfg(test)]
mod tests {
    use super::*;
    use junctur_core::signal::Activity;
    use junctur_core::tracks::{LogicalTrack, Sameness};

    fn logical(members: &[usize], role: Role) -> LogicalTrack {
        LogicalTrack {
            members: members.to_vec(),
            sameness: if members.len() > 1 {
                Sameness::Exact
            } else {
                Sameness::Single
            },
            activity: Activity {
                mean_db: -30.0,
                peak_db: -10.0,
                near_silent_share: 0.0,
                silent: false,
            },
            speech: None,
            role,
            confidence: 1.0,
            reasons: vec![],
        }
    }

    fn report(logical: Vec<LogicalTrack>) -> TrackReport {
        TrackReport {
            file: PathBuf::from("rec.mp4"),
            duration: 1.0,
            windows: vec![],
            window_seconds: 1.0,
            tracks: vec![],
            logical,
            speech_error: None,
            from_cache: true,
        }
    }

    fn muted(m: &Mixer) -> Vec<bool> {
        m.state().tracks.iter().map(|t| t.muted).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("junctur-mixer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn with_a_mix_only_the_mix_is_heard() {
        let r = report(vec![
            logical(&[0], Role::Mix),
            logical(&[1], Role::Voice),
            logical(&[2], Role::GamePc),
        ]);
        let m = Mixer::new(3, Some(&r), None);
        assert_eq!(muted(&m), [false, true, true]);
        assert_eq!(m.state().source, Source::Roles);
        assert_eq!(m.state().tracks[1].role, Some(Role::Voice));
        assert_eq!(m.weights(), "1 0 0");
        // The mix need not be track 1.
        let r = report(vec![logical(&[0], Role::Voice), logical(&[1], Role::Mix)]);
        assert_eq!(muted(&Mixer::new(2, Some(&r), None)), [true, false]);
    }

    #[test]
    fn separate_tracks_are_all_heard_but_copies_are_not() {
        let r = report(vec![
            logical(&[0, 3], Role::Voice),
            logical(&[1], Role::Voice),
            logical(&[2], Role::Silent),
        ]);
        let m = Mixer::new(4, Some(&r), None);
        assert_eq!(muted(&m), [false, false, false, true]);
        assert_eq!(m.state().tracks[3].copy_of, Some(0));
        assert_eq!(m.state().tracks[0].copy_of, None);
        // Four copies of one mix (the June sample): track 1 alone.
        let r = report(vec![logical(&[0, 1, 2, 3], Role::Mix)]);
        assert_eq!(
            muted(&Mixer::new(4, Some(&r), None)),
            [false, true, true, true]
        );
    }

    #[test]
    fn without_roles_track_one_until_they_come() {
        let mut m = Mixer::new(3, None, None);
        assert_eq!(m.state().source, Source::Provisional);
        assert_eq!(muted(&m), [false, true, true]);
        let r = report(vec![
            logical(&[0], Role::Voice),
            logical(&[1], Role::Voice),
            logical(&[2], Role::Voice),
        ]);
        assert!(m.learn(&r), "the roles changed what is heard");
        assert_eq!(muted(&m), [false, false, false]);
        assert_eq!(m.state().source, Source::Roles);
        assert!(!m.learn(&r), "the same roles again change nothing");
        // A report naming no track of the file still leaves something heard.
        let m = Mixer::new(2, Some(&report(vec![logical(&[7], Role::Mix)])), None);
        assert_eq!(muted(&m), [false, true]);
    }

    /// A project in a scratch folder (the file itself is never read).
    fn project(name: &str) -> (PathBuf, Project) {
        let d = scratch(name);
        let file = d.join("rec.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        let p = Project::open(&file, Some(d.join("cache")), Some(d.join("project"))).unwrap();
        (d, p)
    }

    fn remember(m: &mut Mixer, track: usize, mute: bool) {
        m.set_muted(track, mute).unwrap().unwrap().write().unwrap();
    }

    #[test]
    fn the_users_mutes_are_remembered_and_beat_the_roles() {
        let (d, p) = project("remember");
        let r = report(vec![logical(&[0], Role::Mix), logical(&[1], Role::Voice)]);
        let mut m = Mixer::new(2, Some(&r), Some(p.clone()));
        remember(&mut m, 1, false);
        remember(&mut m, 0, true);
        assert!(m.set_muted(2, true).is_err(), "no third track");
        let again = Mixer::new(2, Some(&r), Some(p.clone()));
        assert_eq!(muted(&again), [true, false]);
        assert_eq!(again.state().source, Source::Remembered);
        assert_eq!(again.state().tracks[0].role, Some(Role::Mix));
        // Roles learned later do not undo them.
        let mut later = Mixer::new(2, None, Some(p.clone()));
        later.learn(&r);
        assert_eq!(muted(&later), [true, false]);
        // A file with another track count ignores them.
        assert_eq!(
            muted(&Mixer::new(3, Some(&r), Some(p.clone()))),
            [false, true, true]
        );
        // Broken: ignored.
        std::fs::write(p.dir.join(FILE), b"{").unwrap();
        assert_eq!(
            muted(&Mixer::new(2, Some(&r), Some(p.clone()))),
            [false, true]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Opened mid-analysis, the user unmutes track 3; the roles then say three separate tracks:
    /// track 2, never touched, follows them, and track 3 stays the user's.
    #[test]
    fn roles_move_only_the_tracks_the_user_left_alone() {
        let (d, p) = project("untouched");
        let mut m = Mixer::new(3, None, Some(p.clone()));
        assert_eq!(muted(&m), [false, true, true]);
        remember(&mut m, 2, false);
        let separate = report(vec![
            logical(&[0], Role::Voice),
            logical(&[1], Role::Voice),
            logical(&[2], Role::Voice),
        ]);
        assert!(m.learn(&separate));
        assert_eq!(muted(&m), [false, false, false]);
        // So on the next open, from the file: track 3 is the user's, the rest follow the roles.
        let with_mix = report(vec![
            logical(&[0], Role::Voice),
            logical(&[1], Role::Mix),
            logical(&[2], Role::Voice),
        ]);
        let reopened = Mixer::new(3, Some(&with_mix), Some(p.clone()));
        assert_eq!(muted(&reopened), [true, false, false]);
        assert_eq!(
            reopened
                .state()
                .tracks
                .iter()
                .map(|t| t.set)
                .collect::<Vec<_>>(),
            [false, false, true]
        );
        // A re-analysis that changes the roles moves the untouched tracks and the labels.
        let mut m = reopened;
        assert!(m.learn(&separate));
        assert_eq!(muted(&m), [false, false, false]);
        assert_eq!(m.state().tracks[1].role, Some(Role::Voice));
        // A file from before `set` was kept counts every mute as the user's.
        std::fs::write(p.dir.join(FILE), br#"{"muted":[true,false,true]}"#).unwrap();
        let old = Mixer::new(3, Some(&separate), Some(p.clone()));
        assert_eq!(muted(&old), [true, false, true]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn graphs_for_several_one_and_no_tracks() {
        assert_eq!(Mixer::new(0, None, None).load_options(), None);
        let one = Mixer::new(1, None, None).load_options().unwrap();
        assert!(one.contains("[aid1]aformat="), "{one}");
        assert!(one.contains("[ao]"));
        assert!(!one.contains("amerge"), "one track needs no merge: {one}");
        assert!(one.contains("amix=inputs=1:normalize=0:weights=1,"));
        let r = report(vec![logical(&[0], Role::Mix), logical(&[1], Role::Voice)]);
        let two = Mixer::new(2, Some(&r), None).load_options().unwrap();
        assert!(two.contains("[a0][a1]amerge=inputs=2[ao]"), "{two}");
        assert!(two.contains("[s1]pan=stereo|c0=c2|c1=c3[t1]"), "{two}");
        assert!(
            two.contains("amix=inputs=2:normalize=0:weights=1 0,"),
            "{two}"
        );
        assert!(two.contains("@meter:lavfi=[astats="));
        // Each graph is quoted by its byte length, so its commas do not split the options.
        let (complex, af) = two.split_once(",af=").unwrap();
        let body = |o: &str| {
            let (len, rest) = o.trim_start_matches('%').split_once('%').unwrap();
            assert_eq!(len.parse::<usize>().unwrap(), rest.len(), "{o}");
        };
        body(complex.strip_prefix("lavfi-complex=").unwrap());
        body(af);
        // More tracks than mpv can carry: the first 32 take part.
        assert_eq!(Mixer::new(40, None, None).len(), MAX_TRACKS);
    }

    #[test]
    fn levels_take_the_louder_channel_of_each_track() {
        let json = r#"{"lavfi.astats.1.Peak_level":"-20.5","lavfi.astats.2.Peak_level":"-18.0",
            "lavfi.astats.3.Peak_level":"-inf","lavfi.astats.4.Peak_level":"-inf",
            "lavfi.astats.5.Peak_level":"-30"}"#;
        let l = levels(json, 4);
        assert_eq!(l[0], Some(-18.0));
        assert_eq!(l[1], Some(FLOOR_DB), "silence");
        assert_eq!(l[2], Some(-30.0), "one channel present");
        assert_eq!(l[3], None);
        assert_eq!(levels("not json", 2), vec![None, None]);
        assert_eq!(
            heard(r#"{"lavfi.astats.Overall.Peak_level":"-12.25"}"#),
            Some(-12.25)
        );
        assert_eq!(heard("{}"), None);
    }
}
