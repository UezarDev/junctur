//! The `speakers` stage: who talks when. Two or more voice tracks are a speaker each; otherwise
//! voices are told apart on one track by slow **hearing**, kept ([`Heard`], `windows.f32`), then
//! cheap **labelling** ([`relabel`]), rerun whenever a voice is enrolled, with [`Pin`]s overruling.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Speech, SpeechTrack};
use crate::diarize::{
    self, Embedder, Overlap, Segmenter, CENTROID_MATCH, MINOR_SHARE, VOICE_MATCH,
};
use crate::edit::{Person, Turn};
use crate::ffmpeg::{PcmReader, Span};
use crate::hash::{fnv, Fnv};
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

/// Who talks when in `file`, from the speech stage's regions, labelled with `voices`. `voice` names
/// the single-voice speech tracks. Hearing by voice writes the embeddings to `dir` for [`relabel`]
/// and cuts the windows at `pins` ([`seeded`]); `heard` is told the track and how far it has got.
#[allow(clippy::too_many_arguments)]
pub fn find(
    file: &Path,
    speech: &Speech,
    voice: &[usize],
    voices: &[Voice],
    pins: &[Pin],
    dir: &Path,
    cache_dir: &Path,
    heard: &mut dyn FnMut(usize, f64),
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

/// Why a recording heard by voice has no windows to label or sample from: heard by an older Junctur
/// that did not keep them, or whose windows may be the GPU's and not CAM++'s.
const HEARD_BY_AN_OLDER_JUNCTUR: &str = "this recording's speakers were found by an older Junctur: analyse them again first (`junctur analyze`, or \"Analyse speakers again\" in the app)";

/// `windows.f32` in `dir` when it is the one `heard` was written with, by this build's hearing
/// ([`hearing_key`]): windows from another one, such as GPU-made ones, are never labelled
/// with or taken as a voice sample (a voice remembered from them would match nobody).
fn heard_embeddings(heard: &Heard, dir: &Path) -> Option<Vec<Vec<f32>>> {
    if !heard.is_current() {
        return None;
    }
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
        .ok_or_else(|| crate::Error::Edit(HEARD_BY_AN_OLDER_JUNCTUR.into()))?;
    Ok(label(found, &e, voices))
}

/// A voice sample from what was heard: the cached embeddings of the windows whose middle is in one
/// of `stretches` (source seconds), and the seconds they cover, leaving out windows touching an
/// overlap. For a short pick, the windows reaching into it, weighted by the seconds covered: CAM++
/// on one word alone is too noisy. Empty when none: the caller then uses [`embed_stretches`].
pub fn voice_sample(
    found: &Found,
    dir: &Path,
    stretches: &[(f64, f64)],
) -> Result<(Vec<Vec<f32>>, f64)> {
    let not_heard = || {
        crate::Error::Edit(match found.method {
            Method::Tracks => {
                "the speakers here are their own tracks: there is no voice to tell apart".into()
            }
            Method::None => "no track of this recording carries speech".into(),
            Method::Voices => HEARD_BY_AN_OLDER_JUNCTUR.into(),
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
    // Overlaps are sorted by start and none is longer than a segmentation window, so only those
    // starting within that much before a window can touch it.
    let longest = diarize::segment::WINDOW_S;
    let shared = |w: &Region| {
        let upto = heard.overlaps.partition_point(|o| o.start < w.end);
        heard.overlaps[..upto]
            .iter()
            .rev()
            .take_while(|o| o.start > w.start - longest)
            .any(|o| o.end > w.start)
    };
    for (w, emb) in heard.windows.iter().zip(&e) {
        let mid = (w.start + w.end) / 2.0;
        let chosen = stretches.iter().any(|&(a, b)| a <= mid && mid <= b);
        if chosen && !shared(w) {
            seconds += w.end - w.start;
            out.push(emb.clone());
        }
    }
    if out.is_empty() {
        for (w, emb) in heard.windows.iter().zip(&e) {
            let covered: f64 = stretches
                .iter()
                .map(|&(a, b)| (w.end.min(b) - w.start.max(a)).max(0.0))
                .sum();
            if covered > 0.0 && !shared(w) {
                seconds += covered;
                out.push(emb.clone());
            }
        }
    }
    Ok((out, seconds))
}

/// Embeddings of `stretches` of `track` read straight from `file`, and their seconds, on the CPU:
/// the voice sample only when no cached window covers the selection. Each stretch is read at least
/// [`MIN_CLIP_S`] long, centred on it.
pub fn embed_stretches(
    file: &Path,
    track: usize,
    stretches: &[(f64, f64)],
    cache_dir: &Path,
) -> Result<(Vec<Vec<f32>>, f64)> {
    let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
    let mut emb = Embedder::open(&model)?;
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

/// Labels the heard windows ([`diarize::label`]); turns from the windows, plus a turn for each of
/// the two people in every overlap stretch. Pins of a voice in `voices` win: inside a pin there is
/// exactly one speaker, the pinned one, who gets a speaker even if no window came out as them.
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
            // nearest other speaker.
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
mod tests;
