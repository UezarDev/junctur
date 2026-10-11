//! Transcription, the engine's second stage: find the speech tracks, cut them into short speech
//! chunks with the VAD, and run Whisper on the chunks.

mod pick;
mod render;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ffmpeg::{self, ANALYSIS_RATE};
use crate::tracks::{self, AnalyzeOptions, Role};
use crate::vad::{self, Span};
use crate::whisper::{self, Model, Segment};
use crate::{cache, Result};

pub use pick::speech_tracks;

/// Chunks per whisper-cli run: enough to amortise loading the model, few enough that the command
/// line stays well under Windows' 32k character limit.
pub const BATCH: usize = 64;

#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    pub model: Model,
    /// None means [`whisper::default_models_dir`].
    pub models_dir: Option<PathBuf>,
    /// A Whisper language code; "auto" detects it.
    pub language: String,
    /// Shared with track analysis.
    pub cache_dir: Option<PathBuf>,
    pub use_cache: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            model: Model::default(),
            models_dir: None,
            language: "auto".into(),
            cache_dir: None,
            use_cache: true,
        }
    }
}

impl TranscribeOptions {
    pub fn model_file(&self) -> PathBuf {
        let dir = self
            .models_dir
            .clone()
            .unwrap_or_else(whisper::default_models_dir);
        self.model.path_in(&dir)
    }

    fn settings_key(&self) -> String {
        format!("transcribe|{}|{}", self.model, self.language)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackTranscript {
    /// Audio ordinal (0-based) of the track that was transcribed.
    pub track: usize,
    pub role: Role,
    /// The chunks Whisper heard, in seconds from the start of the file.
    pub chunks: Vec<Span>,
    /// Lines with times in seconds from the start of the file.
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub file: PathBuf,
    pub model: Model,
    pub language: String,
    pub tracks: Vec<TrackTranscript>,
    #[serde(skip)]
    pub from_cache: bool,
}

/// What [`transcribe`] is doing, for a caller that wants to show progress on a long file.
#[derive(Debug, Clone, Copy)]
pub enum Progress {
    /// Measuring levels of a track (audio ordinal).
    Scanning { track: usize },
    /// `done` of `total` chunks of a track transcribed.
    Chunks {
        track: usize,
        done: usize,
        total: usize,
    },
}

pub fn transcribe(
    file: &Path,
    opts: &TranscribeOptions,
    mut progress: impl FnMut(Progress),
) -> Result<Transcript> {
    let dir = opts.cache_dir.clone().unwrap_or_else(cache::default_dir);
    let entry = cache::entry_path(
        &dir,
        "transcripts",
        &cache::key(file, &opts.settings_key())?,
    );
    if opts.use_cache {
        if let Some(mut t) = cache::read::<Transcript>(&entry) {
            t.from_cache = true;
            return Ok(t);
        }
    }

    whisper::check_model(&opts.model_file())?;
    let report = tracks::analyze(
        file,
        &AnalyzeOptions {
            cache_dir: opts.cache_dir.clone(),
            use_cache: opts.use_cache,
            ..AnalyzeOptions::default()
        },
    )?;
    let settings = whisper::Settings {
        model_file: opts.model_file(),
        language: opts.language.clone(),
        threads: std::thread::available_parallelism().map_or(4, |n| n.get()),
    };
    let work = scratch_dir()?;
    let picked = speech_tracks(&report.logical);
    let heard = picked
        .iter()
        .map(|t| {
            let track = t.members[0];
            progress(Progress::Scanning { track });
            let chunks = speech_chunks(file, track, report.duration)?;
            let segments =
                transcribe_chunks(file, track, &chunks, &settings, &work, &mut progress)?;
            Ok(TrackTranscript {
                track,
                role: t.role,
                chunks,
                segments,
            })
        })
        .collect::<Result<Vec<_>>>();
    let _ = std::fs::remove_dir_all(&work);

    let transcript = Transcript {
        file: file.to_path_buf(),
        model: opts.model,
        language: opts.language.clone(),
        tracks: heard?,
        from_cache: false,
    };
    if opts.use_cache {
        cache::write(&entry, &transcript)?;
    }
    Ok(transcript)
}

/// The speech chunks of audio track `track`, from one streaming pass over it.
pub fn speech_chunks(file: &Path, track: usize, duration: f64) -> Result<Vec<Span>> {
    let frame = (ANALYSIS_RATE as f64 * vad::FRAME_SECONDS) as usize;
    let mut levels = Vec::new();
    ffmpeg::stream_track(file, track, frame * 1000, |block| {
        levels.extend(vad::frame_levels(block, frame))
    })?;
    Ok(vad::speech(&levels, duration))
}

fn transcribe_chunks(
    file: &Path,
    track: usize,
    chunks: &[Span],
    settings: &whisper::Settings,
    dir: &Path,
    progress: &mut impl FnMut(Progress),
) -> Result<Vec<Segment>> {
    let mut segments = Vec::new();
    for (b, batch) in chunks.chunks(BATCH).enumerate() {
        let wavs: Vec<PathBuf> = batch
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let wav = dir.join(format!("{track}-{}.wav", b * BATCH + i));
                ffmpeg::extract_wav(file, track, c.start, c.len(), &wav).map(|()| wav)
            })
            .collect::<Result<_>>()?;
        let heard = whisper::transcribe(settings, &wavs)?;
        for (chunk, lines) in batch.iter().zip(heard) {
            segments.extend(lines.into_iter().map(|s| place(s, chunk)));
        }
        for w in &wavs {
            let _ = std::fs::remove_file(w);
        }
        progress(Progress::Chunks {
            track,
            done: b * BATCH + batch.len(),
            total: chunks.len(),
        });
    }
    Ok(segments)
}

/// Move a line from chunk time to file time, keeping it inside its chunk.
fn place(s: Segment, chunk: &Span) -> Segment {
    Segment {
        start: (chunk.start + s.start).min(chunk.end),
        end: (chunk.start + s.end).min(chunk.end),
        text: s.text,
    }
}

/// A fresh folder for chunk WAVs, outside the input's folder (sources are read-only).
fn scratch_dir() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let d = std::env::temp_dir().join(format!("junctur-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_move_to_file_time_and_stay_in_their_chunk() {
        let chunk = Span {
            start: 100.0,
            end: 104.0,
        };
        let s = place(
            Segment {
                start: 1.5,
                end: 4.4,
                text: "hi".into(),
            },
            &chunk,
        );
        assert_eq!((s.start, s.end), (101.5, 104.0));
    }

    #[test]
    fn model_choice_changes_the_file_and_cache_key() {
        let mut o = TranscribeOptions {
            models_dir: Some("/m".into()),
            ..TranscribeOptions::default()
        };
        assert_eq!(o.model_file(), PathBuf::from("/m/ggml-large-v3-turbo.bin"));
        let turbo = o.settings_key();
        o.model = Model::LargeV3;
        assert_eq!(o.model_file(), PathBuf::from("/m/ggml-large-v3.bin"));
        assert_ne!(o.settings_key(), turbo);
    }
}
