//! Role decision for one distinct track, from its loudness profile and, when available, speech.
//!
//! Without speech this is the loudness heuristic. With it, the questions become "does it carry
//! speech" and "what is under the speech": a voice track goes near-silent between phrases, a mix
//! keeps game or music going.

use serde::{Deserialize, Serialize};

use super::Role;
use crate::signal::{Activity, NEAR_SILENT_DB};
use crate::vad;

/// Per-track speech measurements over the sampled windows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Speech {
    /// Share of sampled time inside speech regions.
    pub share: f64,
    /// Share of the non-speech time that is near-silent: high for a voice track, low for a mix.
    pub quiet_gaps: f64,
}

/// A track with less speech than this is treated as carrying none (VAD false positives on music
/// and game sound stay under a few percent).
pub const SPEECH_MIN: f64 = 0.05;
/// A faint track needs more: amplified noise trips the VAD more easily.
pub const FAINT_SPEECH_MIN: f64 = 0.1;
/// Non-speech time at least this quiet means nothing else plays under the voice.
pub const QUIET_GAPS: f64 = 0.6;

/// Speech stats from VAD probabilities and the un-normalised samples they came from.
pub fn measure(probs: &[f32], samples: &[f32]) -> Speech {
    let regions = vad::regions(probs, 0.0);
    let total = probs.len() as f64 * vad::FRAME_SECONDS;
    let in_speech = |i: usize| {
        let t = (i as f64 + 0.5) * vad::FRAME_SECONDS;
        regions.iter().any(|r| t >= r.start && t < r.end)
    };
    let (mut gaps, mut quiet) = (0usize, 0usize);
    for (i, c) in samples.chunks(vad::FRAME).enumerate() {
        if in_speech(i) {
            continue;
        }
        gaps += 1;
        let p = c.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / c.len() as f64;
        if p <= 0.0 || 10.0 * p.log10() < NEAR_SILENT_DB {
            quiet += 1;
        }
    }
    Speech {
        share: vad::share(&regions, total),
        quiet_gaps: if gaps == 0 {
            1.0
        } else {
            quiet as f64 / gaps as f64
        },
    }
}

/// What the caller already knows about the track's place among the others.
pub struct Context {
    pub sum_of_others: Option<f64>,
    pub faint: bool,
    /// Non-silent, non-faint tracks in the recording.
    pub substantial: usize,
}

pub fn decide(a: &Activity, speech: Option<Speech>, cx: &Context) -> (Role, f64, String) {
    if a.silent {
        return (
            Role::Silent,
            0.99,
            format!("silent: loudest frame {:.0} dB", a.peak_db),
        );
    }
    if let Some(r) = cx.sum_of_others {
        let mut why = format!(
            "is the sum of the other tracks (residual {:.1} dB)",
            10.0 * r.max(1e-12).log10()
        );
        if let Some(s) = speech {
            why += &format!(", speech {:.0}%", pct(s.share));
        }
        return (Role::Mix, 0.8, why);
    }
    match (speech, cx.faint) {
        (s, true) => faint(a, s),
        (None, false) => loudness_only(a, cx.substantial),
        (Some(s), false) => with_speech(s, cx.substantial),
    }
}

fn pct(x: f64) -> f64 {
    x * 100.0
}

/// A faint track: named by its speech, if speech detection ran.
fn faint(a: &Activity, speech: Option<Speech>) -> (Role, f64, String) {
    match speech {
        None => (
            Role::Unknown,
            0.3,
            format!(
                "faint: mean {:.0} dB, near-silent {:.0}% of the time; a low-level source \
                 (PC or Discord audio turned down, or bleed), not enough to tell which",
                a.mean_db,
                pct(a.near_silent_share)
            ),
        ),
        Some(s) if s.share >= FAINT_SPEECH_MIN => (
            Role::Voice,
            0.6,
            format!(
                "faint (mean {:.0} dB) but carries speech {:.0}% of the time: likely Discord \
                 turned down, or mic bleed",
                a.mean_db,
                pct(s.share)
            ),
        ),
        Some(s) => (
            Role::GamePc,
            0.5,
            format!(
                "faint (mean {:.0} dB) with no speech ({:.0}%): likely PC or game audio turned down",
                a.mean_db,
                pct(s.share)
            ),
        ),
    }
}

/// A substantial track with speech measured: does it carry speech, and what plays under it.
fn with_speech(s: Speech, substantial: usize) -> (Role, f64, String) {
    if s.share < SPEECH_MIN {
        (
            Role::GamePc,
            0.6,
            format!(
                "no speech ({:.0}%): game, PC or music audio (music is not told apart yet)",
                pct(s.share)
            ),
        )
    } else if s.quiet_gaps >= QUIET_GAPS {
        (
            Role::Voice,
            0.75,
            format!(
                "speech {:.0}% of the time, near-silent between phrases ({:.0}% of the gaps)",
                pct(s.share),
                pct(s.quiet_gaps)
            ),
        )
    } else if substantial == 1 {
        (
            Role::Mix,
            0.8,
            format!(
                "the only substantial track: speech {:.0}% of the time over a background that keeps \
                 playing (only {:.0}% of the gaps near-silent)",
                pct(s.share),
                pct(s.quiet_gaps)
            ),
        )
    } else {
        (
            Role::Voice,
            0.5,
            format!(
                "speech {:.0}% of the time over continuous sound: a noisy voice track, or game \
                 audio with voices in it",
                pct(s.share)
            ),
        )
    }
}

/// The loudness-only guess, used when speech detection is off or unavailable.
fn loudness_only(a: &Activity, substantial: usize) -> (Role, f64, String) {
    if substantial == 1 {
        (
            Role::Mix,
            0.6,
            "the only track with substantial sound; other tracks are silent or faint".into(),
        )
    } else if a.near_silent_share >= 0.3 {
        (
            Role::Voice,
            0.5,
            format!(
                "intermittent like speech: near-silent {:.0}% of the time",
                a.near_silent_share * 100.0
            ),
        )
    } else {
        (
            Role::GamePc,
            0.4,
            format!(
                "continuous sound: near-silent only {:.0}% of the time",
                a.near_silent_share * 100.0
            ),
        )
    }
}

#[cfg(test)]
mod tests;
