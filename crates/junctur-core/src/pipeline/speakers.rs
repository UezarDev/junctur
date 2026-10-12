//! The `speakers` stage: who talks when, for a recording, in one of two ways.
//!
//! When OBS recorded the voices on their own tracks (a mic, a Discord) there are several speech
//! tracks, and each one is a speaker: exact and free, no model. When everything is on one mix
//! track, the speakers are told apart by voice (VAD regions from the `speech` stage, CAM++
//! embeddings, two-pass clustering) straight on the mix: 5a measured the vocal stem buying nothing
//! for this (stem and mix embeddings of one window at cosine 0.94 median, the same three speakers)
//! while separation costs ~19 GB per 30 h. Its own stage and file (`speakers.json`) because it is
//! analysis the edit list is made from, like `speech.json`; the names the user gives the speakers
//! live in the edit list ([`crate::edit::Speakers`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Speech;
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
}

/// Who talks when in `file`, from the speech stage's regions.
pub fn find(file: &Path, speech: &Speech, cache_dir: &Path, device: DevicePref) -> Result<Found> {
    match speech.tracks.as_slice() {
        [] => Ok(Found {
            method: Method::None,
            tracks: Vec::new(),
            turns: Vec::new(),
        }),
        [one] => {
            let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
            let mut emb = Embedder::open(&model, device)?;
            let mut r = PcmReader::open(file, one.track, RATE, 1, Span::default())?;
            // Not `finish`ed: diarization stops reading after the last speech window, and the
            // decoder is dropped (killed) rather than waited for.
            let d = diarize::diarize(&mut r, &one.regions, &mut emb)?;
            Ok(Found {
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
            })
        }
        many => Ok(by_track(many)),
    }
}

/// One speaker per speech track, in track order; turns are the tracks' speech regions.
fn by_track(tracks: &[super::SpeechTrack]) -> Found {
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
        let f = by_track(&[
            track(0, vec![(0.0, 2.0), (5.0, 6.0)]),
            track(2, vec![(1.0, 3.0)]),
        ]);
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
            Path::new("."),
            DevicePref::Cpu,
        )
        .unwrap();
        assert_eq!(f.method, Method::None);
        assert!(f.turns.is_empty());
    }
}
