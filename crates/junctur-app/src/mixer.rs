//! Every audio track heard at once in the preview, each with a meter and a mute (playback only):
//! mpv filter graphs and the default-mute rule, testable without mpv. A mute is an amix weight
//! changed in place by `af-command`, so a click never rebuilds the chain or leaves a gap.

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
    /// No track roles yet (the recording is not analysed): track 1 only.
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
    /// Soloed: while any track is, only the soloed ones are heard, muted or not. Playback only
    /// and never remembered: a recording opens with no solo.
    pub solo: bool,
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
                    solo: false,
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

    /// Solos or unsolos track `index`; nothing is remembered.
    pub fn set_solo(&mut self, index: usize, on: bool) -> Result<(), String> {
        let t = self
            .tracks
            .get_mut(index)
            .ok_or_else(|| format!("no audio track {}", index + 1))?;
        t.solo = on;
        Ok(())
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

    /// amix's `weights`: 1 for a track heard, 0 for one that is not. With a solo, the soloed tracks
    /// are heard whatever their mute (soloing a muted mic is how to hear it alone; a solo that kept
    /// the mutes would play nothing where only the mix is heard); without, the unmuted ones.
    pub fn weights(&self) -> String {
        let solo = self.tracks.iter().any(|t| t.solo);
        let w: Vec<&str> = self
            .tracks
            .iter()
            .map(|t| {
                if (solo && t.solo) || (!solo && !t.muted) {
                    "1"
                } else {
                    "0"
                }
            })
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

/// Which tracks the roles say to hear: with a mix, the mix alone; without one, every
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
#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
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
mod tests;
