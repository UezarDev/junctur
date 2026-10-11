//! Voice activity detection: per-frame levels in, speech spans out. Pure, so every threshold is
//! tested on synthetic levels.
//!
//! Energy based on purpose. Junctur's voice tracks are a mic or Discord feed, where the line
//! between speech and the noise floor is clear; the spans only decide what Whisper hears, and the
//! padding keeps word edges.

use serde::{Deserialize, Serialize};

use crate::signal;

/// Levels are measured per 30 ms frame, fine enough that padding, not the frame, sets the edges.
pub const FRAME_SECONDS: f64 = 0.03;
/// Speech must be this far above the track's noise floor.
pub const ABOVE_FLOOR_DB: f64 = 12.0;
/// The threshold never drops below the near-silent line, so a dead-quiet track does not turn
/// breaths and fan noise into speech.
pub const MIN_THRESHOLD_DB: f64 = signal::NEAR_SILENT_DB;
/// Pauses shorter than this stay inside one span.
pub const MERGE_GAP_SECONDS: f64 = 0.5;
/// Bursts shorter than this (clicks, key presses) are dropped.
pub const MIN_SPEECH_SECONDS: f64 = 0.25;
/// Kept on each side of a span so first and last syllables are not clipped.
pub const PAD_SECONDS: f64 = 0.2;
/// Longest chunk sent to Whisper: its window is 30 s, and short chunks with no carried context keep
/// large-v3-turbo from repeating lines.
pub const MAX_CHUNK_SECONDS: f64 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

impl Span {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Level of each frame in dBFS.
pub fn frame_levels(samples: &[f32], frame: usize) -> Vec<f64> {
    samples
        .chunks(frame.max(1))
        .map(|c| signal::db(signal::power(c)))
        .collect()
}

/// The noise floor: the 10th-percentile frame level, ignoring digital silence, which would
/// otherwise pull the floor to -120 dB on tracks that gate their mic.
pub fn noise_floor(levels: &[f64]) -> f64 {
    let mut live: Vec<f64> = levels
        .iter()
        .copied()
        .filter(|&l| l > signal::FLOOR_DB)
        .collect();
    if live.is_empty() {
        return signal::FLOOR_DB;
    }
    live.sort_by(f64::total_cmp);
    live[live.len() / 10]
}

/// Speech spans in seconds, padded, merged across short pauses and cut to at most
/// [`MAX_CHUNK_SECONDS`]. `duration` clamps the padding at the end of the track.
pub fn speech(levels: &[f64], duration: f64) -> Vec<Span> {
    let threshold = (noise_floor(levels) + ABOVE_FLOOR_DB).max(MIN_THRESHOLD_DB);
    let frames = |s: f64| (s / FRAME_SECONDS).round() as usize;

    // Runs of loud frames, as frame ranges.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, &l) in levels.iter().enumerate() {
        if l < threshold {
            continue;
        }
        match runs.last_mut() {
            Some(r) if i - r.1 <= frames(MERGE_GAP_SECONDS) => r.1 = i + 1,
            _ => runs.push((i, i + 1)),
        }
    }

    let mut spans: Vec<Span> = Vec::new();
    for (a, b) in runs {
        if b - a < frames(MIN_SPEECH_SECONDS) {
            continue;
        }
        let start = (a as f64 * FRAME_SECONDS - PAD_SECONDS).max(0.0);
        let end = (b as f64 * FRAME_SECONDS + PAD_SECONDS).min(duration);
        match spans.last_mut() {
            // Padding can make neighbours touch; one chunk is better than two overlapping ones.
            Some(s) if start <= s.end => s.end = end,
            _ => spans.push(Span { start, end }),
        }
    }
    spans
        .into_iter()
        .flat_map(|s| split(s, levels))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Cut a long span at its quietest frame in the second half of each chunk, so cuts fall in pauses.
fn split(mut span: Span, levels: &[f64]) -> Vec<Span> {
    let mut out = Vec::new();
    while span.len() > MAX_CHUNK_SECONDS {
        let lo = ((span.start + MAX_CHUNK_SECONDS / 2.0) / FRAME_SECONDS) as usize;
        let hi = ((span.start + MAX_CHUNK_SECONDS) / FRAME_SECONDS) as usize;
        let cut = (lo..hi.min(levels.len()))
            .min_by(|&a, &b| levels[a].total_cmp(&levels[b]))
            .map(|f| f as f64 * FRAME_SECONDS)
            .unwrap_or(span.start + MAX_CHUNK_SECONDS);
        out.push(Span {
            start: span.start,
            end: cut,
        });
        span.start = cut;
    }
    out.push(span);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Levels from (seconds, dB) pieces.
    fn levels(pieces: &[(f64, f64)]) -> Vec<f64> {
        pieces
            .iter()
            .flat_map(|&(s, db)| std::iter::repeat_n(db, (s / FRAME_SECONDS).round() as usize))
            .collect()
    }

    #[test]
    fn finds_padded_speech_in_noise() {
        let l = levels(&[(2.0, -60.0), (1.5, -25.0), (2.0, -60.0)]);
        let s = speech(&l, 5.5);
        assert_eq!(s.len(), 1);
        assert!((s[0].start - 1.8).abs() <= FRAME_SECONDS, "{s:?}");
        assert!((s[0].end - 3.7).abs() <= FRAME_SECONDS, "{s:?}");
    }

    #[test]
    fn short_pauses_merge_and_long_ones_split() {
        let l = levels(&[
            (1.0, -60.0),
            (1.0, -25.0),
            (0.3, -60.0),
            (1.0, -25.0),
            (2.0, -60.0),
            (1.0, -25.0),
            (1.0, -60.0),
        ]);
        let s = speech(&l, 7.3);
        assert_eq!(s.len(), 2, "{s:?}");
    }

    #[test]
    fn clicks_and_silence_are_not_speech() {
        let l = levels(&[(2.0, -60.0), (0.09, -10.0), (2.0, -60.0)]);
        assert!(speech(&l, 4.1).is_empty());
        assert!(speech(&levels(&[(5.0, signal::FLOOR_DB)]), 5.0).is_empty());
        assert!(speech(&[], 0.0).is_empty());
    }

    #[test]
    fn quiet_track_keeps_threshold_at_near_silent() {
        // Floor -100 dB would put the threshold at -88; breaths at -70 must still not count.
        let l = levels(&[(3.0, -100.0), (1.0, -70.0), (3.0, -100.0)]);
        assert!(speech(&l, 7.0).is_empty());
    }

    #[test]
    fn long_speech_is_cut_in_a_pause() {
        // The -40 dB dip is above the threshold, so this is one 40 s run that must be cut there.
        let l = levels(&[(10.0, -60.0), (20.0, -25.0), (0.3, -40.0), (20.0, -25.0)]);
        let s = speech(&l, 50.3);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s.iter().all(|x| x.len() <= MAX_CHUNK_SECONDS + 1e-9));
        assert!(s[0].end >= 30.0 && s[0].end < 30.3, "{s:?}");
        assert_eq!(s[0].end, s[1].start);
    }

    #[test]
    fn noise_floor_skips_digital_silence() {
        let l = levels(&[(5.0, signal::FLOOR_DB), (5.0, -62.0), (1.0, -20.0)]);
        assert_eq!(noise_floor(&l), -62.0);
    }
}
