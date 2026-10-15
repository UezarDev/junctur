//! The `speakers` stage: who talks when. Two or more voice tracks are a speaker each; otherwise
//! voices are told apart on one track by slow **hearing**, kept ([`Heard`], `windows.f32`), then
//! cheap **labelling** ([`relabel`]), rerun whenever a voice is enrolled, with [`Pin`]s overruling.

mod embeddings;
mod labels;
mod sample;

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Speech, SpeechTrack};
use crate::diarize::{self, Embedder, Overlap, Segmenter};
use crate::edit::{Person, Turn};
use crate::ffmpeg::{PcmReader, Span};
use crate::hash::{fnv, Fnv};
use crate::vad::{Region, RATE};
use crate::voices::Voice;
use crate::{models, Result};

pub use labels::{label, relabel, remember_known};
pub use sample::{embed_stretches, voice_sample};

use embeddings::{heard_embeddings, save_embeddings};

/// Turn pieces shorter than this, left between two pins, are dropped (turns are held to the ms).
const SLIVER_S: f64 = 0.001;

/// Seconds the user said are one person's: the stretch picked to remember their voice. At
/// every labelling those seconds are that person and nobody else ([`label`]); hearing again cuts
/// its windows at them ([`seeded`]). By name, so a pin follows the voice the name is; a pin whose
/// name is no usable voice (forgotten, another model) is ignored but kept, and remembering the
/// name again brings it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub start: f64,
    pub end: f64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Each speech track is a speaker.
    Tracks,
    /// Speakers told apart by voice on the one speech track.
    Voices,
    /// No track carries speech.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub method: Method,
    /// Per speaker (speaker `i + 1`): the track it is, when speakers are tracks.
    pub tracks: Vec<Option<usize>>,
    /// Per speaker: the enrolled voice it was recognised as, whose name and colour it takes.
    #[serde(default)]
    pub people: Vec<Option<Person>>,
    /// Sorted by start; speaker ids from 1. Two people's turns overlap where both talk at once.
    pub turns: Vec<Turn>,
    /// [`speech_key`] of the speech regions this was found from: when the speech stage runs again
    /// and finds other regions, this result is stale and the stage runs again too.
    #[serde(default)]
    pub speech: String,
    /// What was heard on the track, for labelling again without reading it; `None` when the
    /// speakers are tracks, and in an older file (which is then heard again once).
    #[serde(default)]
    pub heard: Option<Heard>,
    /// Every name an enrolled voice has had while labelling this recording, current or since
    /// forgotten: a speaker's name that is one of these is the voice's, never carried onto a
    /// cluster ([`crate::edit::Speakers::found`]).
    #[serde(default)]
    pub known: Vec<String>,
    /// The seconds the user pinned to a person, sorted by start, never overlapping. The
    /// project's own record of what the user said, so it is carried over when hearing runs again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
}

impl Found {
    /// Pins `stretches` (source seconds) to `name`: a newer pin takes its seconds from older pins
    /// of other people, and one person's pins that touch or overlap become one.
    pub fn pin(&mut self, name: &str, stretches: &[(f64, f64)]) {
        let same = |p: &Pin| p.name.to_lowercase() == name.trim().to_lowercase();
        let mut mine: Vec<(f64, f64)> = stretches
            .iter()
            .copied()
            .filter(|(a, b)| b - a > SLIVER_S)
            .chain(
                self.pins
                    .iter()
                    .filter(|p| same(p))
                    .map(|p| (p.start, p.end)),
            )
            .collect();
        mine.sort_by(|a, b| a.0.total_cmp(&b.0));
        let joined = crate::spans::joined(mine, crate::spans::Join::Touching);
        let mut pins: Vec<Pin> = self
            .pins
            .iter()
            .filter(|p| !same(p))
            .flat_map(|p| {
                outside(p.start, p.end, &joined)
                    .into_iter()
                    .map(|(start, end)| Pin {
                        start,
                        end,
                        name: p.name.clone(),
                    })
            })
            .collect();
        pins.extend(joined.into_iter().map(|(start, end)| Pin {
            start,
            end,
            name: name.trim().to_string(),
        }));
        pins.sort_by(|a, b| a.start.total_cmp(&b.start));
        self.pins = pins;
    }

    /// Renames the pins of `from` to `to` (a remembered person renamed).
    pub fn rename_pins(&mut self, from: &str, to: &str) {
        for p in &mut self.pins {
            if p.name.to_lowercase() == from.trim().to_lowercase() {
                p.name = to.trim().to_string();
            }
        }
    }
}

/// The pieces of `start..end` outside every one of `cut` (sorted by start, not overlapping), each
/// longer than [`SLIVER_S`].
fn outside(start: f64, end: f64, cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut at = start;
    for &(a, b) in cut {
        if b <= at || a >= end {
            continue;
        }
        if a - at > SLIVER_S {
            out.push((at, a));
        }
        at = at.max(b);
    }
    if end - at > SLIVER_S {
        out.push((at, end));
    }
    out
}

/// Speech regions for hearing, cut at every pin's edges, with the pins outside the regions added,
/// so the pinned seconds get windows of their own. A piece under 1.5 s gets one centred window
/// ([`diarize::segments`]) reaching past its edges, still overruled inside the pin ([`label`]).
pub fn seeded(regions: &[Region], pins: &[Pin]) -> Vec<Region> {
    let mut edges: Vec<f64> = pins.iter().flat_map(|p| [p.start, p.end]).collect();
    edges.sort_by(f64::total_cmp);
    let mut out: Vec<Region> = Vec::new();
    for r in regions {
        let mut at = r.start;
        for &e in edges.iter().filter(|&&e| e > r.start && e < r.end) {
            if e > at {
                out.push(Region { start: at, end: e });
                at = e;
            }
        }
        out.push(Region {
            start: at,
            end: r.end,
        });
    }
    let spoken: Vec<(f64, f64)> = regions.iter().map(|r| (r.start, r.end)).collect();
    for p in pins {
        for (start, end) in outside(p.start, p.end, &spoken) {
            out.push(Region { start, end });
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// What hearing found on the diarized track, besides the embeddings (kept in `windows.f32`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    pub track: usize,
    /// The embedding windows, in the order of the embeddings.
    pub windows: Vec<Region>,
    /// Where two people talk at once.
    pub overlaps: Vec<Overlap>,
    /// [`hearing_key`] of how this was heard: another model, window or threshold means hearing
    /// again.
    pub key: String,
    /// Numbers per embedding in `windows.f32`.
    pub dim: usize,
    /// FNV-1a of `windows.f32` as written with this: the two files are one result, and a
    /// `windows.f32` from another run (a copy, a crash between the writes) is not these windows.
    pub embeddings: String,
}

impl Heard {
    /// Heard by this build's hearing ([`hearing_key`]). Windows heard otherwise (maybe on the GPU,
    /// not CAM++'s numbers) are never reused, labelled with or taken as a voice sample.
    pub fn is_current(&self) -> bool {
        self.key == hearing_key(self.track)
    }
}

impl Found {
    /// Told apart by voice, but heard by an older Junctur (no windows kept, or maybe the GPU's).
    /// Its speakers cannot be labelled again until they are heard again; its pins can be renamed.
    pub fn heard_by_an_older_junctur(&self) -> bool {
        self.method == Method::Voices && !self.heard.as_ref().is_some_and(Heard::is_current)
    }
}

/// What hearing track `track` depends on besides the speech regions: both models (by digest) and
/// every window and threshold that shapes the windows and the overlap stretches.
pub fn hearing_key(track: usize) -> String {
    let what = format!(
        "{track}|{}|{}|{}",
        models::SPEAKER_EMBEDDING.sha256,
        models::SEGMENTATION.sha256,
        diarize::hearing_settings()
    );
    format!("{:016x}", fnv(what.as_bytes()))
}

/// FNV-1a over the speech tracks and their regions.
pub fn speech_key(speech: &Speech) -> String {
    let mut h = Fnv::new();
    for t in &speech.tracks {
        h.u64(t.track as u64);
        for r in &t.regions {
            h.u64(r.start.to_bits()).u64(r.end.to_bits());
        }
    }
    h.hex()
}

/// How speakers are found for these speech tracks.
#[derive(Debug, PartialEq)]
enum Plan<'a> {
    /// Two or more voice tracks: each is a speaker (a mix beside them is left out).
    ByTrack(Vec<&'a SpeechTrack>),
    /// Fewer: tell voices apart on this one track (a mix if there is one, since it holds
    /// everyone; else the one voice track).
    Diarize(&'a SpeechTrack),
    Nothing,
}

/// `voice`: the audio ordinals whose role is a single voice (not a mix).
fn plan<'a>(speech: &'a Speech, voice: &[usize]) -> Plan<'a> {
    let voices: Vec<&SpeechTrack> = speech
        .tracks
        .iter()
        .filter(|t| voice.contains(&t.track))
        .collect();
    if voices.len() >= 2 {
        return Plan::ByTrack(voices);
    }
    let mix = speech.tracks.iter().find(|t| !voice.contains(&t.track));
    match mix.or(speech.tracks.first()) {
        Some(t) => Plan::Diarize(t),
        None => Plan::Nothing,
    }
}

/// What [`find`] hears and where it keeps what it heard.
pub struct Job<'a> {
    /// The recording.
    pub file: &'a Path,
    /// The speech stage's regions.
    pub speech: &'a Speech,
    /// The single-voice speech tracks.
    pub voice: &'a [usize],
    /// The remembered voices the speakers are labelled with.
    pub voices: &'a [Voice],
    /// The user's pins, which cut the windows ([`seeded`]).
    pub pins: &'a [Pin],
    /// The project folder, where hearing by voice writes the embeddings for [`relabel`].
    pub dir: &'a Path,
    /// Where the models are kept.
    pub cache_dir: &'a Path,
}

/// Who talks when in `job.file`, from the speech stage's regions, labelled with the remembered
/// voices ([`Job`]); `heard` is told the track and how far it has got.
pub fn find(job: &Job, heard: &mut dyn FnMut(usize, f64)) -> Result<Found> {
    let Job {
        file,
        speech,
        voice,
        voices,
        pins,
        dir,
        cache_dir,
    } = *job;
    let found = match plan(speech, voice) {
        Plan::Nothing => Found {
            method: Method::None,
            tracks: Vec::new(),
            people: Vec::new(),
            turns: Vec::new(),
            speech: speech_key(speech),
            heard: None,
            known: Vec::new(),
            pins: pins.to_vec(),
        },
        // Pins are the user's whatever the plan: kept for when the tracks are heard by voice again.
        Plan::ByTrack(tracks) => Found {
            speech: speech_key(speech),
            pins: pins.to_vec(),
            ..by_track(&tracks)
        },
        Plan::Diarize(one) => {
            let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
            let mut emb = Embedder::open(&model)?;
            let seg_model = models::ensure(&models::SEGMENTATION, cache_dir)?;
            let mut seg = Segmenter::open(&seg_model)?;
            let mut r = PcmReader::open(file, one.track, RATE, 1, Span::default())?;
            // Not `finish`ed: hearing stops reading after the last speech window, and the decoder
            // is dropped (killed) rather than waited for.
            let track = one.track;
            heard(track, 0.0);
            let regions = seeded(&one.regions, pins);
            let a = diarize::analyze(&mut r, &regions, &mut emb, Some(&mut seg), &mut |at| {
                heard(track, at)
            })?;
            // The rest of the track has no speech to hear: done.
            heard(track, f64::INFINITY);
            let embeddings = save_embeddings(dir, &a.embeddings)?;
            let unlabelled = Found {
                method: Method::Voices,
                tracks: Vec::new(),
                people: Vec::new(),
                turns: Vec::new(),
                speech: speech_key(speech),
                heard: Some(Heard {
                    track: one.track,
                    windows: a.windows,
                    overlaps: a.overlaps,
                    key: hearing_key(one.track),
                    dim: a.embeddings.first().map_or(0, Vec::len),
                    embeddings,
                }),
                known: Vec::new(),
                pins: pins.to_vec(),
            };
            return Ok(label(&unlabelled, &a.embeddings, voices));
        }
    };
    Ok(found)
}

/// The cached result in `dir` when it still holds for `speech` and the tracks the speakers
/// come from (`voice`, as [`find`] takes it) with this build's models and settings (hearing need
/// not run again), with its embeddings when speakers are told apart by voice.
pub fn cached(
    found: Found,
    speech: &Speech,
    voice: &[usize],
    dir: &Path,
) -> Option<(Found, Vec<Vec<f32>>)> {
    if found.speech != speech_key(speech) {
        return None;
    }
    // The plan follows the roles too: a track that became a voice since changes it.
    let same_plan = match (plan(speech, voice), found.method) {
        (Plan::Nothing, Method::None) => true,
        (Plan::ByTrack(t), Method::Tracks) => {
            t.iter().map(|x| Some(x.track)).collect::<Vec<_>>() == found.tracks
        }
        (Plan::Diarize(t), Method::Voices) => {
            found.heard.as_ref().is_some_and(|h| h.track == t.track)
        }
        _ => false,
    };
    if !same_plan {
        return None;
    }
    if found.method != Method::Voices {
        return Some((found, Vec::new()));
    }
    let heard = found.heard.as_ref()?;
    let e = heard_embeddings(heard, dir)?;
    Some((found, e))
}

/// One speaker per voice track, in track order; turns are the tracks' speech regions.
fn by_track(tracks: &[&SpeechTrack]) -> Found {
    let mut turns: Vec<Turn> = tracks
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            t.regions.iter().map(move |r| Turn {
                start: r.start,
                end: r.end,
                speaker: i as u32 + 1,
            })
        })
        .collect();
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    Found {
        method: Method::Tracks,
        tracks: tracks.iter().map(|t| Some(t.track)).collect(),
        people: Vec::new(),
        turns,
        speech: String::new(),
        heard: None,
        known: Vec::new(),
        pins: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
