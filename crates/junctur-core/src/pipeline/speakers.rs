//! The `speakers` stage: who talks when, for a recording, in one of two ways.
//!
//! When OBS recorded the voices on their own tracks (a mic, a Discord) there are several voice
//! tracks, and each one is a speaker: exact and free, no model. A mix track is never one speaker:
//! it holds everyone, so with voice tracks beside it the speakers are the voice tracks (words
//! heard on the mix go to whichever voice track talks then). When there are fewer than two voice
//! tracks, the speakers are told apart by voice on one track (the mix if there is one) (VAD regions from the `speech` stage, CAM++
//! embeddings, two-pass clustering) straight on the mix: 5a measured the vocal stem buying nothing
//! for this (stem and mix embeddings of one window at cosine 0.94 median, the same three speakers)
//! while separation costs ~19 GB per 30 h. Its own stage and file (`speakers.json`) because it is
//! analysis the edit list is made from, like `speech.json`; the names the user gives the speakers
//! live in the edit list ([`crate::edit::Speakers`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Speech, SpeechTrack};
use crate::diarize::{self, Embedder};
use crate::edit::Turn;
use crate::ffmpeg::{PcmReader, Span};
use crate::runtime::DevicePref;
use crate::vad::RATE;
use crate::{models, Result};

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
    /// Sorted by start; speaker ids from 1.
    pub turns: Vec<Turn>,
    /// [`speech_key`] of the speech regions this was found from: when the speech stage runs again
    /// and finds other regions, this result is stale and the stage runs again too.
    #[serde(default)]
    pub speech: String,
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

/// Who talks when in `file`, from the speech stage's regions. `voice` names the speech tracks
/// whose role is a single voice; the others are mixes.
pub fn find(
    file: &Path,
    speech: &Speech,
    voice: &[usize],
    cache_dir: &Path,
    device: DevicePref,
) -> Result<Found> {
    let mut found = match plan(speech, voice) {
        Plan::Nothing => Found {
            method: Method::None,
            tracks: Vec::new(),
            turns: Vec::new(),
            speech: String::new(),
        },
        Plan::ByTrack(tracks) => by_track(&tracks),
        Plan::Diarize(one) => {
            let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
            let mut emb = Embedder::open(&model, device)?;
            let mut r = PcmReader::open(file, one.track, RATE, 1, Span::default())?;
            // Not `finish`ed: diarization stops reading after the last speech window, and the
            // decoder is dropped (killed) rather than waited for.
            let d = diarize::diarize(&mut r, &one.regions, &mut emb)?;
            Found {
                method: Method::Voices,
                tracks: vec![None; d.speakers.len()],
                turns: d
                    .turns
                    .iter()
                    .map(|t| Turn {
                        start: t.start,
                        end: t.end,
                        speaker: t.speaker as u32 + 1,
                    })
                    .collect(),
                speech: String::new(),
            }
        }
    };
    found.speech = speech_key(speech);
    Ok(found)
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
        turns,
        speech: String::new(),
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
}
