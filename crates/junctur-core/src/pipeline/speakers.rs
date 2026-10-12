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
}

/// What hearing found on the diarized track, besides the embeddings (kept in `windows.f32`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    pub track: usize,
    /// The embedding windows, in the order of the embeddings.
    pub windows: Vec<Region>,
    /// Where two people talk at once.
    pub overlaps: Vec<Overlap>,
    /// The embedding and segmentation models' files: another model means hearing again.
    pub models: Vec<String>,
    /// Numbers per embedding in `windows.f32`.
    pub dim: usize,
}

fn heard_models() -> Vec<String> {
    vec![
        models::SPEAKER_EMBEDDING.file.to_string(),
        models::SEGMENTATION.file.to_string(),
    ]
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
/// apart by voice, the window embeddings are written to project folder `dir` for [`relabel`].
pub fn find(
    file: &Path,
    speech: &Speech,
    voice: &[usize],
    voices: &[Voice],
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
            let a = diarize::analyze(&mut r, &one.regions, &mut emb, Some(&mut seg))?;
            save_embeddings(dir, &a.embeddings)?;
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
                    models: heard_models(),
                    dim: a.embeddings.first().map_or(0, Vec::len),
                }),
            };
            return Ok(label(&unlabelled, &a.embeddings, voices));
        }
    };
    Ok(found)
}

/// The cached result in `dir` when it still holds for `speech` with this build's models (hearing
/// need not run again), with its embeddings when speakers are told apart by voice.
pub fn cached(found: Found, speech: &Speech, dir: &Path) -> Option<(Found, Vec<Vec<f32>>)> {
    if found.speech != speech_key(speech) {
        return None;
    }
    if found.method != Method::Voices {
        return Some((found, Vec::new()));
    }
    let heard = found.heard.as_ref()?;
    if heard.models != heard_models() {
        return None;
    }
    let e = load_embeddings(dir, heard.dim)?;
    (e.len() == heard.windows.len()).then_some((found, e))
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
        .and_then(|h| load_embeddings(dir, h.dim).filter(|e| e.len() == h.windows.len()))
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
    let e = load_embeddings(dir, heard.dim)
        .filter(|e| e.len() == heard.windows.len())
        .ok_or_else(not_heard)?;
    let mut out = Vec::new();
    let mut seconds = 0.0;
    for (w, emb) in heard.windows.iter().zip(e) {
        let mid = (w.start + w.end) / 2.0;
        let chosen = stretches.iter().any(|&(a, b)| a <= mid && mid <= b);
        let shared = heard
            .overlaps
            .iter()
            .any(|o| o.start < w.end && o.end > w.start);
        if chosen && !shared {
            seconds += w.end - w.start;
            out.push(emb);
        }
    }
    Ok((out, seconds))
}

/// Labels the heard windows: enrolled voices first, then clusters ([`diarize::label`]); turns from
/// the windows, plus a turn for each of the two people in every overlap stretch.
pub fn label(found: &Found, embeddings: &[Vec<f32>], voices: &[Voice]) -> Found {
    let Some(heard) = &found.heard else {
        return found.clone();
    };
    let prints: Vec<Vec<f32>> = voices.iter().map(|v| v.print.clone()).collect();
    let labels = diarize::label(
        embeddings,
        &prints,
        VOICE_MATCH,
        CENTROID_MATCH,
        MINOR_SHARE,
    );
    let windows = diarize::turns(&heard.windows, &labels.windows);
    let mut turns: Vec<Turn> = windows
        .iter()
        .map(|t| Turn {
            start: t.start,
            end: t.end,
            speaker: t.speaker as u32 + 1,
        })
        .collect();
    let base = turns.clone();
    for o in &heard.overlaps {
        let pair = match (who(&o.a, &base), who(&o.b, &base)) {
            (Some(a), Some(b)) if a != b => Some((a, b)),
            // One of the two is known (or both came out as one person): the other is whoever
            // else talks nearest to the stretch, as the one cutting in or being cut in on.
            (Some(x), _) | (_, Some(x)) => neighbour(o, x, &base).map(|y| (x, y)),
            (None, None) => None,
        };
        for speaker in pair.into_iter().flat_map(|(a, b)| [a, b]) {
            turns.push(Turn {
                start: o.start,
                end: o.end,
                speaker,
            });
        }
    }
    turns.sort_by(|a, b| a.start.total_cmp(&b.start));
    Found {
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

/// Raw little-endian f32, [`Heard::dim`] per window, like `envelope.f32`.
fn save_embeddings(dir: &Path, e: &[Vec<f32>]) -> Result<()> {
    let bytes: Vec<u8> = e.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(embeddings_path(dir), bytes)?;
    Ok(())
}

/// The cached window embeddings; `None` when missing or not a whole number of embeddings.
pub fn load_embeddings(dir: &Path, dim: usize) -> Option<Vec<Vec<f32>>> {
    let b = std::fs::read(embeddings_path(dir)).ok()?;
    if dim == 0 {
        return b.is_empty().then(Vec::new);
    }
    if b.len() % (dim * 4) != 0 {
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
            model: models::SPEAKER_EMBEDDING.file.into(),
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
        let e = (0..20)
            .map(|i| {
                unit(if i < 10 {
                    &[1.0, 0.1, 0.0]
                } else {
                    &[0.1, 1.0, 0.0]
                })
            })
            .collect();
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
                models: heard_models(),
                dim: 3,
            }),
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
                &d
            ),
            None
        );
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
}
