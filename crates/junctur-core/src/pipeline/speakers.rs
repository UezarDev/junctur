//! The `speakers` stage: who talks when, for a recording, in one of two ways.
//!
//! When OBS recorded the voices on their own tracks (a mic, a Discord) there are several voice
//! tracks, and each one is a speaker: exact and free, no model. A mix track is never one speaker:
//! it holds everyone, so with voice tracks beside it the speakers are the voice tracks (words
//! heard on the mix go to whichever voice track talks then). When there are fewer than two voice
//! tracks, the speakers are told apart by voice on one track (the mix if there is one): VAD regions
//! from the `speech` stage, CAM++ embeddings of 1.5 s windows, straight on the mix (5a measured the
//! vocal stem buying nothing for this while separation costs ~19 GB per 30 h).
//!
//! Telling them apart by voice is two steps since 5b. **Hearing** reads the track once: the windows'
//! embeddings and, with pyannote's segmentation model, where two people talk at once
//! ([`crate::diarize::analyze`]). It is the slow part and is kept: the embeddings in `windows.f32`,
//! the rest in `speakers.json` ([`Heard`]). **Labelling** is cheap and runs on every stage run and
//! whenever a voice is enrolled or forgotten ([`relabel`]): windows like an enrolled voice are that
//! person, the rest are clustered as in 5a, and each overlap stretch gives its two people a turn
//! each, so turns of different people overlap there. Its own stage and file because it is analysis
//! the edit list is made from, like `speech.json`; the names the user gives the speakers live in the
//! edit list ([`crate::edit::Speakers`]).
//!
//! Since 5c the seconds a user picks to remember a voice are **pins** ([`Pin`]): those seconds are
//! that person whatever the windows say, at every labelling, and hearing again cuts its windows at
//! them ([`seeded`]), so the user's picks shape both the cheap pass and the slow one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Speech, SpeechTrack};
use crate::diarize::{
    self, Embedder, Overlap, Segmenter, CENTROID_MATCH, MINOR_SHARE, VOICE_MATCH,
};
use crate::edit::{Person, Turn};
use crate::ffmpeg::{PcmReader, Span};
use crate::runtime::DevicePref;
use crate::vad::{Region, RATE};
use crate::voices::Voice;
use crate::{models, Result};

/// Where hearing keeps the window embeddings, in the project folder.
const EMBEDDINGS_FILE: &str = "windows.f32";
/// An overlap's local speaker is named by the speaker whose turns cover most of where it talks
/// alone in its window, when they cover at least this much.
const MIN_EVIDENCE_S: f64 = 0.5;
/// When only one of an overlap's two people is known, the other is the nearest other speaker
/// within this many seconds of it.
const NEIGHBOUR_S: f64 = 3.0;
/// A stretch embedded straight from the audio ([`embed_stretches`]) is read at least this long,
/// centred on it, so CAM++ has frames to work on even for one short word.
const MIN_CLIP_S: f64 = 0.5;
/// Turn pieces shorter than this, left between two pins, are dropped (turns are held to the ms).
const SLIVER_S: f64 = 0.001;

/// Seconds the user said are one person's (5c): the stretch picked to remember their voice. At
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
    /// Per speaker: the enrolled voice it was recognised as (5b), whose name and colour it takes.
    #[serde(default)]
    pub people: Vec<Option<Person>>,
    /// Sorted by start; speaker ids from 1. Two people's turns overlap where both talk at once.
    pub turns: Vec<Turn>,
    /// [`speech_key`] of the speech regions this was found from: when the speech stage runs again
    /// and finds other regions, this result is stale and the stage runs again too.
    #[serde(default)]
    pub speech: String,
    /// What was heard on the track, for labelling again without reading it (5b); `None` when the
    /// speakers are tracks, and in a 5a file (which is then heard again once).
    #[serde(default)]
    pub heard: Option<Heard>,
    /// Every name an enrolled voice has had while labelling this recording, current or since
    /// forgotten: a speaker's name that is one of these is the voice's, never carried onto a
    /// cluster ([`crate::edit::Speakers::found`]).
    #[serde(default)]
    pub known: Vec<String>,
    /// The seconds the user pinned to a person (5c), sorted by start, never overlapping. The
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
            .filter(|(a, b)| b > a)
            .chain(
                self.pins
                    .iter()
                    .filter(|p| same(p))
                    .map(|p| (p.start, p.end)),
            )
            .collect();
        mine.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut joined: Vec<(f64, f64)> = Vec::new();
        for (a, b) in mine {
            match joined.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => joined.push((a, b)),
            }
        }
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

/// Speech regions for hearing, cut at every pin's edges (5c), with the pins that lie outside the
/// regions added (the user says someone talks there): no embedding window straddles a pin's edge,
/// and the pinned seconds get windows of their own. A piece under 0.5 s still gets none and one
/// under 1.5 s one centred window ([`diarize::segments`]), which reaches past it.
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

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// FNV-1a over the speech tracks and their regions.
pub fn speech_key(speech: &Speech) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        for b in x.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    for t in &speech.tracks {
        eat(t.track as u64);
        for r in &t.regions {
            eat(r.start.to_bits());
            eat(r.end.to_bits());
        }
    }
    format!("{h:016x}")
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

/// Who talks when in `file`, from the speech stage's regions, labelled with `voices`. `voice` names
/// the speech tracks whose role is a single voice; the others are mixes. When the speakers are told
/// apart by voice, the window embeddings are written to project folder `dir` for [`relabel`], and
/// the windows are cut at `pins` (5c: [`seeded`]), which the result keeps.
#[allow(clippy::too_many_arguments)]
pub fn find(
    file: &Path,
    speech: &Speech,
    voice: &[usize],
    voices: &[Voice],
    pins: &[Pin],
    dir: &Path,
    cache_dir: &Path,
    device: DevicePref,
) -> Result<Found> {
    let found = match plan(speech, voice) {
        Plan::Nothing => Found {
            method: Method::None,
            tracks: Vec::new(),
            people: Vec::new(),
            turns: Vec::new(),
            speech: speech_key(speech),
            heard: None,
            known: Vec::new(),
            pins: Vec::new(),
        },
        Plan::ByTrack(tracks) => Found {
            speech: speech_key(speech),
            ..by_track(&tracks)
        },
        Plan::Diarize(one) => {
            let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
            let mut emb = Embedder::open(&model, device)?;
            let seg_model = models::ensure(&models::SEGMENTATION, cache_dir)?;
            let mut seg = Segmenter::open(&seg_model, device)?;
            let mut r = PcmReader::open(file, one.track, RATE, 1, Span::default())?;
            // Not `finish`ed: hearing stops reading after the last speech window, and the decoder
            // is dropped (killed) rather than waited for.
            let regions = seeded(&one.regions, pins);
            let a = diarize::analyze(&mut r, &regions, &mut emb, Some(&mut seg))?;
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
    if heard.key != hearing_key(heard.track) {
        return None;
    }
    let e = heard_embeddings(heard, dir)?;
    Some((found, e))
}

/// `windows.f32` in `dir` when it is the one `heard` was written with.
fn heard_embeddings(heard: &Heard, dir: &Path) -> Option<Vec<Vec<f32>>> {
    let b = std::fs::read(embeddings_path(dir)).ok()?;
    if format!("{:016x}", fnv(&b)) != heard.embeddings {
        return None;
    }
    let e = decode_embeddings(&b, heard.dim)?;
    (e.len() == heard.windows.len()).then_some(e)
}

/// `found` labelled again with `voices` from what was heard (no audio, no model): what enrolling,
/// forgetting or renaming a voice calls. Speakers that are tracks, or none, are returned as they
/// are.
pub fn relabel(found: &Found, dir: &Path, voices: &[Voice]) -> Result<Found> {
    if found.method != Method::Voices {
        return Ok(found.clone());
    }
    let e = found
        .heard
        .as_ref()
        .and_then(|h| heard_embeddings(h, dir))
        .ok_or_else(|| {
            crate::Error::Edit(
                "this recording's speakers were found before Junctur kept voiceprints: run `junctur analyze` on it once"
                    .into(),
            )
        })?;
    Ok(label(found, &e, voices))
}

/// A voice sample from what was heard (5b): the cached embeddings of the windows whose middle is
/// in one of `stretches` (source seconds), and the seconds of speech they cover. Windows touching a
/// stretch where two people talk at once are left out: their voiceprint is two people's.
///
/// A short selection (5c: one word is allowed) often has no such window: windows are 1.5 s. Then
/// every window reaching into a stretch at all, overlaps included, each counted by the seconds of
/// the stretches it covers, so a word moves a voice built from more only a little. Empty when no
/// window reaches the stretches either (the caller embeds the audio there: [`embed_stretches`]).
pub fn voice_sample(
    found: &Found,
    dir: &Path,
    stretches: &[(f64, f64)],
) -> Result<(Vec<Vec<f32>>, f64)> {
    let not_heard = || {
        crate::Error::Edit(match found.method {
            Method::Tracks => "the speakers here are their own tracks: there is no voice to tell apart".into(),
            Method::None => "no track of this recording carries speech".into(),
            Method::Voices => "this recording's speakers were found before Junctur kept voiceprints: run `junctur analyze` on it once".into(),
        })
    };
    let heard = found
        .heard
        .as_ref()
        .filter(|_| found.method == Method::Voices)
        .ok_or_else(not_heard)?;
    let e = heard_embeddings(heard, dir).ok_or_else(not_heard)?;
    let mut out = Vec::new();
    let mut seconds = 0.0;
    let covered = |w: &Region| -> f64 {
        stretches
            .iter()
            .map(|&(a, b)| (w.end.min(b) - w.start.max(a)).max(0.0))
            .sum()
    };
    // Overlaps are sorted by start and none is longer than a segmentation window, so only those
    // starting within that much before a window can touch it.
    let longest = diarize::segment::WINDOW_S;
    for (w, emb) in heard.windows.iter().zip(&e) {
        let mid = (w.start + w.end) / 2.0;
        let chosen = stretches.iter().any(|&(a, b)| a <= mid && mid <= b);
        let upto = heard.overlaps.partition_point(|o| o.start < w.end);
        let shared = heard.overlaps[..upto]
            .iter()
            .rev()
            .take_while(|o| o.start > w.start - longest)
            .any(|o| o.end > w.start);
        if chosen && !shared {
            seconds += w.end - w.start;
            out.push(emb.clone());
        }
    }
    if out.is_empty() {
        for (w, emb) in heard.windows.iter().zip(&e) {
            let c = covered(w);
            if c > 0.0 {
                seconds += c;
                out.push(emb.clone());
            }
        }
    }
    Ok((out, seconds))
}

/// Embeddings of `stretches` of `track` read straight from `file`, and their seconds: the last
/// resort for a voice sample (5c) where hearing made no window (a region under 0.5 s, a word
/// outside the speech regions). Each stretch is read at least [`MIN_CLIP_S`] long, centred on it.
/// CPU: one short clip is not worth starting the GPU for. 5b rejected decoding at enrollment as
/// the way to get a sample, since the stage already has the numbers; this is only for what it
/// never saw.
pub fn embed_stretches(
    file: &Path,
    track: usize,
    stretches: &[(f64, f64)],
    cache_dir: &Path,
) -> Result<(Vec<Vec<f32>>, f64)> {
    let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
    let mut emb = Embedder::open(&model, DevicePref::Cpu)?;
    let mut out = Vec::new();
    let mut seconds = 0.0;
    for &(a, b) in stretches {
        let len = (b - a).max(MIN_CLIP_S);
        let start = ((a + b) / 2.0 - len / 2.0).max(0.0);
        let mut r = PcmReader::open(
            file,
            track,
            RATE,
            1,
            Span {
                start,
                len: Some(len),
            },
        )?;
        let mut samples = Vec::new();
        loop {
            let more = r.read(RATE as usize)?;
            if more.is_empty() {
                break;
            }
            samples.extend(more);
        }
        r.finish()?;
        if let Some(e) = emb.embed(&samples)? {
            out.push(e);
            seconds += b - a;
        }
    }
    Ok((out, seconds))
}

/// Labels the heard windows: enrolled voices first, then clusters ([`diarize::label`]); turns from
/// the windows, plus a turn for each of the two people in every overlap stretch.
///
/// Pins (5c) of a voice in `voices` win over all of it: a window whose middle is in one is that
/// voice outright ([`diarize::label_fixed`]), every turn is cut out of the pins, and each pin is a
/// turn of its person, so inside a pin there is exactly one speaker, the pinned one. A pinned voice
/// no window came out as still gets a speaker, after the others.
pub fn label(found: &Found, embeddings: &[Vec<f32>], voices: &[Voice]) -> Found {
    let Some(heard) = &found.heard else {
        return found.clone();
    };
    let prints: Vec<Vec<f32>> = voices.iter().map(|v| v.print.clone()).collect();
    // Pins of the voices here, by voice index; pins are sorted and never overlap.
    let pins: Vec<(f64, f64, usize)> = found
        .pins
        .iter()
        .filter_map(|p| {
            let name = p.name.to_lowercase();
            voices
                .iter()
                .position(|v| v.name.to_lowercase() == name)
                .map(|v| (p.start, p.end, v))
        })
        .collect();
    let spans: Vec<(f64, f64)> = pins.iter().map(|&(a, b, _)| (a, b)).collect();
    let fixed: Vec<Option<usize>> = heard
        .windows
        .iter()
        .map(|w| {
            let mid = (w.start + w.end) / 2.0;
            let i = pins.partition_point(|p| p.1 < mid);
            pins.get(i).filter(|p| p.0 <= mid).map(|p| p.2)
        })
        .collect();
    let mut labels = diarize::label_fixed(
        embeddings,
        &prints,
        &fixed,
        VOICE_MATCH,
        CENTROID_MATCH,
        MINOR_SHARE,
    );
    for &(_, _, v) in &pins {
        if !labels.voices.contains(&Some(v)) {
            labels.voices.push(Some(v));
        }
    }
    let id_of = |v: usize| {
        labels
            .voices
            .iter()
            .position(|&x| x == Some(v))
            .expect("every pinned voice has a speaker") as u32
            + 1
    };
    let windows = diarize::turns(&heard.windows, &labels.windows);
    // Window turns outside the pins, and the pins: still sorted and never overlapping, which
    // `who`, `covering` and `neighbour` rely on.
    let mut turns: Vec<Turn> = windows
        .iter()
        .flat_map(|t| {
            outside(t.start, t.end, &spans)
                .into_iter()
                .map(move |(start, end)| Turn {
                    start,
                    end,
                    speaker: t.speaker as u32 + 1,
                })
        })
        .chain(pins.iter().map(|&(start, end, v)| Turn {
            start,
            end,
            speaker: id_of(v),
        }))
        .collect();
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    let base = turns.clone();
    for o in &heard.overlaps {
        let pair = match (who(&o.a, &base), who(&o.b, &base)) {
            (Some(a), Some(b)) if a != b => Some((a, b)),
            // One of the two is known (or both came out as one person): the other is whoever
            // else talks nearest to the stretch, as the one cutting in or being cut in on.
            (Some(x), _) | (_, Some(x)) => neighbour(o, x, &base).map(|y| (x, y)),
            // Neither talks alone in the window: whoever the windows say talks there, and the
            // nearest other speaker. Measured in the repair pass: 13 s more of two people on
            // the sample at the same 91% against the avatars.
            (None, None) => covering(o, &base).and_then(|x| neighbour(o, x, &base).map(|y| (x, y))),
        };
        for speaker in pair.into_iter().flat_map(|(a, b)| [a, b]) {
            for (start, end) in outside(o.start, o.end, &spans) {
                turns.push(Turn {
                    start,
                    end,
                    speaker,
                });
            }
        }
    }
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut known = found.known.clone();
    add_known(&mut known, found.people.iter().flatten().map(|p| &p.name));
    add_known(&mut known, voices.iter().map(|v| &v.name));
    Found {
        known,
        method: Method::Voices,
        tracks: vec![None; labels.voices.len()],
        people: labels
            .voices
            .iter()
            .map(|v| {
                v.map(|i| Person {
                    name: voices[i].name.clone(),
                    color: voices[i].color.clone(),
                })
            })
            .collect(),
        turns,
        speech: found.speech.clone(),
        heard: found.heard.clone(),
        pins: found.pins.clone(),
    }
}

/// The speaker whose `turns` (from windows: sorted, never overlapping) cover most of `spans`, when
/// they cover at least [`MIN_EVIDENCE_S`].
fn who(spans: &[[f64; 2]], turns: &[Turn]) -> Option<u32> {
    let mut time: Vec<(u32, f64)> = Vec::new();
    for &[s, e] in spans {
        let from = turns.partition_point(|t| t.end <= s);
        for t in turns[from..].iter().take_while(|t| t.start < e) {
            let o = t.end.min(e) - t.start.max(s);
            if o <= 0.0 {
                continue;
            }
            match time.iter_mut().find(|(id, _)| *id == t.speaker) {
                Some((_, x)) => *x += o,
                None => time.push((t.speaker, o)),
            }
        }
    }
    time.into_iter()
        .filter(|&(_, x)| x >= MIN_EVIDENCE_S)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

fn add_known<'a>(known: &mut Vec<String>, names: impl Iterator<Item = &'a String>) {
    for n in names {
        if !known.iter().any(|k| k.to_lowercase() == n.to_lowercase()) {
            known.push(n.clone());
        }
    }
}

/// Adds to `found`'s [`Found::known`] the voice names `old` (the result it replaces) knew.
pub fn remember_known(found: &mut Found, old: &Found) {
    let mut known = std::mem::take(&mut found.known);
    add_known(&mut known, old.known.iter());
    add_known(&mut known, old.people.iter().flatten().map(|p| &p.name));
    found.known = known;
}

/// The speaker whose turn (from windows) holds the middle of overlap `o`.
fn covering(o: &Overlap, turns: &[Turn]) -> Option<u32> {
    let mid = (o.start + o.end) / 2.0;
    let i = turns.partition_point(|t| t.end < mid);
    turns.get(i).filter(|t| t.start <= mid).map(|t| t.speaker)
}

/// The speaker other than `x` whose turn (from windows) is nearest to overlap `o`, within
/// [`NEIGHBOUR_S`].
fn neighbour(o: &Overlap, x: u32, turns: &[Turn]) -> Option<u32> {
    let from = turns.partition_point(|t| t.end < o.start - NEIGHBOUR_S);
    turns[from..]
        .iter()
        .take_while(|t| t.start <= o.end + NEIGHBOUR_S)
        .filter(|t| t.speaker != x)
        .map(|t| {
            let gap = (t.start - o.end).max(o.start - t.end).max(0.0);
            (gap, t.speaker)
        })
        .filter(|&(gap, _)| gap <= NEIGHBOUR_S)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

fn embeddings_path(dir: &Path) -> PathBuf {
    dir.join(EMBEDDINGS_FILE)
}

/// Raw little-endian f32, [`Heard::dim`] per window, like `envelope.f32`. Returns the file's
/// [`Heard::embeddings`] key.
fn save_embeddings(dir: &Path, e: &[Vec<f32>]) -> Result<String> {
    let bytes: Vec<u8> = e.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(embeddings_path(dir), &bytes)?;
    Ok(format!("{:016x}", fnv(&bytes)))
}

/// Embeddings of `dim` numbers from raw bytes; `None` when not a whole number of them.
fn decode_embeddings(b: &[u8], dim: usize) -> Option<Vec<Vec<f32>>> {
    if dim == 0 {
        return b.is_empty().then(Vec::new);
    }
    if !b.len().is_multiple_of(dim * 4) {
        return None;
    }
    Some(
        b.chunks_exact(dim * 4)
            .map(|w| {
                w.chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect()
            })
            .collect(),
    )
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
mod tests {
    use super::*;
    use crate::pipeline::SpeechTrack;
    use crate::vad::Region;

    #[test]
    fn several_speech_tracks_are_one_speaker_each() {
        let track = |track, regions: Vec<(f64, f64)>| SpeechTrack {
            track,
            regions: regions
                .into_iter()
                .map(|(start, end)| Region { start, end })
                .collect(),
            share: 0.5,
        };
        let (a, b) = (
            track(0, vec![(0.0, 2.0), (5.0, 6.0)]),
            track(2, vec![(1.0, 3.0)]),
        );
        let f = by_track(&[&a, &b]);
        assert_eq!(f.method, Method::Tracks);
        assert_eq!(f.tracks, vec![Some(0), Some(2)]);
        let ids: Vec<u32> = f.turns.iter().map(|t| t.speaker).collect();
        assert_eq!(ids, vec![1, 2, 1]);
    }

    #[test]
    fn no_speech_no_speakers() {
        let speech = Speech {
            tracks: vec![],
            seconds: 10.0,
        };
        let f = find(
            Path::new("missing.mp4"),
            &speech,
            &[],
            &[],
            &[],
            Path::new("."),
            Path::new("."),
            DevicePref::Cpu,
        )
        .unwrap();
        assert_eq!(f.method, Method::None);
        assert!(f.turns.is_empty());
        assert_eq!(f.speech, speech_key(&speech));
    }

    #[test]
    fn a_mix_is_never_a_speaker_beside_voice_tracks() {
        let track = |track| SpeechTrack {
            track,
            regions: vec![Region {
                start: track as f64,
                end: track as f64 + 1.0,
            }],
            share: 0.5,
        };
        let speech = |ts: &[usize]| Speech {
            tracks: ts.iter().map(|&t| track(t)).collect(),
            seconds: 10.0,
        };
        // OBS: mix on 0, mic on 1, Discord on 2. The voices are the speakers.
        let s = speech(&[0, 1, 2]);
        let Plan::ByTrack(v) = plan(&s, &[1, 2]) else {
            panic!("by track")
        };
        assert_eq!(v.iter().map(|t| t.track).collect::<Vec<_>>(), vec![1, 2]);
        // Mix plus one voice: told apart by voice on the mix, which holds everyone.
        let s = speech(&[0, 1]);
        assert_eq!(plan(&s, &[1]), Plan::Diarize(&s.tracks[0]));
        // One voice track alone, or only a mix: that track.
        let s = speech(&[3]);
        assert_eq!(plan(&s, &[3]), Plan::Diarize(&s.tracks[0]));
        assert_eq!(plan(&s, &[]), Plan::Diarize(&s.tracks[0]));
        assert_eq!(plan(&speech(&[]), &[]), Plan::Nothing);
        // A changed region makes another key.
        let mut moved = speech(&[0, 1]);
        moved.tracks[1].regions[0].end += 0.5;
        assert_ne!(speech_key(&speech(&[0, 1])), speech_key(&moved));
    }

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("junctur-speakers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn voice(name: &str, print: Vec<f32>) -> Voice {
        Voice {
            name: name.into(),
            color: "#123456".into(),
            seconds: 30.0,
            model: models::SPEAKER_EMBEDDING.sha256.into(),
            print,
        }
    }

    /// Ana (axis 0) talks 0-15 s, Bea (axis 1) 15-30 s, in 1.5 s windows; overlap stretches given.
    fn heard(overlaps: Vec<Overlap>) -> (Found, Vec<Vec<f32>>) {
        let windows: Vec<Region> = (0..20)
            .map(|i| Region {
                start: i as f64 * 1.5,
                end: i as f64 * 1.5 + 1.5,
            })
            .collect();
        let e: Vec<Vec<f32>> = (0..20)
            .map(|i| {
                unit(if i < 10 {
                    &[1.0, 0.1, 0.0]
                } else {
                    &[0.1, 1.0, 0.0]
                })
            })
            .collect();
        let bytes: Vec<u8> = e.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        let found = Found {
            method: Method::Voices,
            tracks: vec![],
            people: vec![],
            turns: vec![],
            speech: "k".into(),
            heard: Some(Heard {
                track: 0,
                windows,
                overlaps,
                key: hearing_key(0),
                dim: 3,
                embeddings: format!("{:016x}", fnv(&bytes)),
            }),
            known: vec![],
            pins: vec![],
        };
        (found, e)
    }

    #[test]
    fn voices_name_speakers_and_an_overlap_gives_both_people_a_turn() {
        // 5-6 s both talk: local a talks alone 3-5 s (Ana's time), local b 16-18 s (Bea's).
        let o = Overlap {
            start: 5.0,
            end: 6.0,
            a: vec![[3.0, 5.0]],
            b: vec![[16.0, 18.0]],
        };
        let (found, e) = heard(vec![o.clone()]);
        let voices = [
            voice("Bea", unit(&[0.0, 1.0, 0.0])),
            voice("Ana", unit(&[1.0, 0.0, 0.0])),
        ];
        let f = label(&found, &e, &voices);
        // Bea is speaker 1 (the voices' order), Ana speaker 2.
        let names: Vec<_> = f
            .people
            .iter()
            .map(|p| p.as_ref().map(|p| p.name.as_str()))
            .collect();
        assert_eq!(names, vec![Some("Bea"), Some("Ana")]);
        assert_eq!(f.tracks, vec![None, None]);
        let at = |t: f64| -> Vec<u32> {
            f.turns
                .iter()
                .filter(|x| x.start <= t && t < x.end)
                .map(|x| x.speaker)
                .collect()
        };
        assert_eq!(at(2.0), vec![2]);
        let mut both = at(5.5);
        both.sort();
        both.dedup();
        assert_eq!(both, vec![1, 2]);
        assert_eq!(at(20.0), vec![1]);
        // No voices: 5a's numbering, and the same overlap turns.
        let plain = label(&found, &e, &[]);
        assert_eq!(plain.people, vec![None, None]);
        assert_eq!(plain.turns.len(), f.turns.len());
        // One local speaker with no evidence: the other is the nearest other speaker. None
        // near: no overlap turn.
        let lone = Overlap {
            start: 13.5,
            end: 14.5,
            a: vec![[10.0, 13.0]],
            b: vec![],
        };
        let (found, e) = heard(vec![lone]);
        let l = label(&found, &e, &voices);
        assert_eq!(l.turns.len(), 4, "{:?}", l.turns);
        assert!(l.turns.iter().any(|t| t.start == 13.5 && t.speaker == 1));
        let far = Overlap {
            start: 2.0,
            end: 3.0,
            a: vec![[0.0, 2.0]],
            b: vec![],
        };
        let (found, e) = heard(vec![far]);
        assert_eq!(
            label(&found, &e, &voices).turns.len(),
            2,
            "Ana, then Bea: no overlap turn"
        );
        // Neither local speaker talks alone: the windows' speaker there and the nearest other.
        let blind = Overlap {
            start: 14.0,
            end: 14.6,
            a: vec![],
            b: vec![],
        };
        let (found, e) = heard(vec![blind]);
        let l = label(&found, &e, &voices);
        let mut at: Vec<u32> = l
            .turns
            .iter()
            .filter(|t| t.start == 14.0)
            .map(|t| t.speaker)
            .collect();
        at.sort();
        assert_eq!(at, vec![1, 2]);
    }

    #[test]
    fn a_voice_sample_is_the_chosen_windows_without_overlapping_talk() {
        let d = scratch("sample");
        let o = Overlap {
            start: 3.2,
            end: 3.4,
            a: vec![],
            b: vec![],
        };
        let (found, e) = heard(vec![o]);
        save_embeddings(&d, &e).unwrap();
        // Windows 0-1.5, 1.5-3, 3-4.5 (touches the overlap), 4.5-6.
        let (sample, secs) = voice_sample(&found, &d, &[(0.0, 6.0)]).unwrap();
        assert_eq!(sample.len(), 3);
        assert!((secs - 4.5).abs() < 1e-9);
        // Relabelling reads the same embeddings back.
        let again = relabel(&found, &d, &[]).unwrap();
        assert_eq!(again, label(&found, &e, &[]));
        assert_eq!(
            cached(
                found.clone(),
                &Speech {
                    tracks: vec![],
                    seconds: 0.0
                },
                &[],
                &d
            ),
            None
        );
        // The same windows in a cached result for this speech: reused with their embeddings.
        let speech = Speech {
            tracks: vec![SpeechTrack {
                track: 0,
                regions: vec![Region {
                    start: 0.0,
                    end: 30.0,
                }],
                share: 0.5,
            }],
            seconds: 30.0,
        };
        let mine = Found {
            speech: speech_key(&speech),
            ..found.clone()
        };
        assert!(cached(mine.clone(), &speech, &[], &d).is_some());
        // Not when the track is now a voice track of its own (another plan), or after other
        // settings or models.
        assert!(
            cached(mine.clone(), &speech, &[0, 1], &d).is_some(),
            "one voice: still diarized"
        );
        let mut other = mine.clone();
        other.heard.as_mut().unwrap().key = "old".into();
        assert!(cached(other, &speech, &[], &d).is_none());
        // A windows.f32 of the same size from another run is not these windows.
        let mut swapped = e.clone();
        swapped.swap(0, 19);
        save_embeddings(&d, &swapped).unwrap();
        assert!(cached(mine, &speech, &[], &d).is_none());
        assert!(relabel(&found, &d, &[]).is_err());
        save_embeddings(&d, &e).unwrap();
        // Speakers that are tracks have no voice to sample.
        let tracks = Found {
            method: Method::Tracks,
            heard: None,
            ..found.clone()
        };
        assert!(voice_sample(&tracks, &d, &[(0.0, 6.0)]).is_err());
        // A file of another size is not these windows.
        std::fs::write(d.join(EMBEDDINGS_FILE), [0u8; 8]).unwrap();
        assert!(relabel(&found, &d, &[]).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn speakers_at(f: &Found, t: f64) -> Vec<u32> {
        let mut s: Vec<u32> = f
            .turns
            .iter()
            .filter(|x| x.start <= t && t < x.end)
            .map(|x| x.speaker)
            .collect();
        s.sort();
        s.dedup();
        s
    }

    fn pins(f: &Found) -> Vec<(f64, f64, &str)> {
        f.pins
            .iter()
            .map(|p| (p.start, p.end, p.name.as_str()))
            .collect()
    }

    #[test]
    fn a_newer_pin_takes_its_seconds_and_one_persons_pins_join() {
        let (mut f, _) = heard(vec![]);
        f.pin("Ana", &[(2.0, 5.0)]);
        f.pin("Bea", &[(4.0, 6.0), (10.0, 11.0)]);
        assert_eq!(
            pins(&f),
            vec![(2.0, 4.0, "Ana"), (4.0, 6.0, "Bea"), (10.0, 11.0, "Bea")]
        );
        // Ana again over the middle of Bea's: Bea keeps what is left either side; Ana's pins
        // are found whatever the case of the name, and take the name as given (enrolling gives
        // the voice's own).
        f.pin("ana", &[(4.5, 5.5)]);
        assert_eq!(
            pins(&f),
            vec![
                (2.0, 4.0, "ana"),
                (4.0, 4.5, "Bea"),
                (4.5, 5.5, "ana"),
                (5.5, 6.0, "Bea"),
                (10.0, 11.0, "Bea")
            ]
        );
        f.pin("Ana", &[(3.5, 4.5)]);
        assert_eq!(pins(&f)[0], (2.0, 5.5, "Ana"));
        assert_eq!(f.pins.len(), 3);
        f.rename_pins("ANA", "Anita");
        assert_eq!(pins(&f)[0], (2.0, 5.5, "Anita"));
        // An empty stretch pins nothing.
        let (mut g, _) = heard(vec![]);
        g.pin("Ana", &[(3.0, 3.0)]);
        assert!(g.pins.is_empty());
    }

    #[test]
    fn pinned_seconds_are_the_pinned_person_and_nobody_else() {
        // Bea is pinned over 3-6 s, where the windows hear Ana, and where an overlap stretch
        // would give both a turn.
        let o = Overlap {
            start: 4.0,
            end: 5.0,
            a: vec![[0.0, 3.0]],
            b: vec![[16.0, 18.0]],
        };
        let (mut found, e) = heard(vec![o]);
        let voices = [
            voice("Ana", unit(&[1.0, 0.0, 0.0])),
            voice("Bea", unit(&[0.0, 1.0, 0.0])),
        ];
        assert_eq!(speakers_at(&label(&found, &e, &voices), 4.5), vec![1, 2]);
        found.pin("Bea", &[(3.0, 6.0)]);
        let f = label(&found, &e, &voices);
        assert_eq!(f.pins, found.pins, "the pins are kept");
        for t in [3.0, 3.5, 4.5, 5.9] {
            assert_eq!(speakers_at(&f, t), vec![2], "at {t}: {:?}", f.turns);
        }
        assert_eq!(speakers_at(&f, 2.9), vec![1]);
        assert_eq!(speakers_at(&f, 6.0), vec![1]);
        assert_eq!(speakers_at(&f, 20.0), vec![2]);
        // Turns of window labels never overlap each other, pins included.
        let mut solo: Vec<&Turn> = f.turns.iter().collect();
        solo.sort_by(|a, b| a.start.total_cmp(&b.start));
        assert!(solo.windows(2).all(|w| w[0].end <= w[1].start + 1e-9));
        // A pin of a name no voice has is ignored, and kept.
        let mut odd = found.clone();
        odd.pin("Nobody", &[(20.0, 21.0)]);
        let f = label(&odd, &e, &voices);
        assert_eq!(speakers_at(&f, 20.5), vec![2]);
        assert_eq!(f.pins.len(), 2);
        // A pinned voice no window sounds like still gets a speaker, after the others.
        let cid = voice("Cid", unit(&[0.0, 0.0, 1.0]));
        let three = [voices[0].clone(), voices[1].clone(), cid];
        let mut c = found.clone();
        c.pin("Cid", &[(20.0, 21.0)]);
        let f = label(&c, &e, &three);
        let names: Vec<_> = f
            .people
            .iter()
            .map(|p| p.as_ref().map(|p| p.name.as_str()))
            .collect();
        assert_eq!(names, vec![Some("Ana"), Some("Bea"), Some("Cid")]);
        assert_eq!(speakers_at(&f, 20.5), vec![3]);
        assert_eq!(speakers_at(&f, 21.5), vec![2]);
    }

    #[test]
    fn a_short_selection_still_gives_a_voice_sample() {
        let d = scratch("short");
        let o = Overlap {
            start: 3.2,
            end: 3.4,
            a: vec![],
            b: vec![],
        };
        let (found, e) = heard(vec![o]);
        save_embeddings(&d, &e).unwrap();
        // One word, 3.3-3.6: the window 3-4.5 has its middle outside it and touches the overlap,
        // so no clean window; it is taken anyway, weighing the 0.3 s it covers.
        let (sample, secs) = voice_sample(&found, &d, &[(3.3, 3.6)]).unwrap();
        assert_eq!(sample, vec![e[2].clone()]);
        assert!((secs - 0.3).abs() < 1e-9, "{secs}");
        // A word across two windows: both, by what each covers.
        let (sample, secs) = voice_sample(&found, &d, &[(5.8, 6.2)]).unwrap();
        assert_eq!(sample.len(), 2);
        assert!((secs - 0.4).abs() < 1e-9, "{secs}");
        // Where hearing made no window: nothing here (the caller reads the audio).
        let (sample, _) = voice_sample(&found, &d, &[(31.0, 31.4)]).unwrap();
        assert!(sample.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hearing_regions_are_cut_at_the_pins() {
        let r = |start, end| Region { start, end };
        let pin = |start, end| Pin {
            start,
            end,
            name: "Ana".into(),
        };
        let regions = [r(0.0, 10.0), r(20.0, 22.0)];
        let got = seeded(&regions, &[pin(2.0, 3.0), pin(9.0, 12.0), pin(15.0, 16.0)]);
        assert_eq!(
            got,
            vec![
                r(0.0, 2.0),
                r(2.0, 3.0),
                r(3.0, 9.0),
                r(9.0, 10.0),
                r(10.0, 12.0),
                r(15.0, 16.0),
                r(20.0, 22.0)
            ]
        );
        assert_eq!(seeded(&regions, &[]), regions.to_vec());
    }
}
