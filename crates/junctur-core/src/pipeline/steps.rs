//! How far each track has got through the running stage, with the track's length beside it, for a
//! caller that draws progress bars. Its own channel rather than log lines: one line per track per
//! read would flood the CLI. A caller that wants none passes [`none`].

use serde::Serialize;

use crate::tracks::TrackReport;

/// One track's place in one stage: `at` seconds of `of` are behind it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Step {
    /// A name from [`super::STAGES`].
    pub stage: &'static str,
    /// 0-based audio ordinal.
    pub track: usize,
    pub at: f64,
    pub of: f64,
}

impl Step {
    /// The stage's share of the track that is done, 0 to 100, whole (a bar that never claims
    /// 100 before it is).
    pub fn percent(&self) -> u32 {
        if self.of <= 0.0 {
            return 100;
        }
        ((self.at / self.of).clamp(0.0, 1.0) * 100.0).floor() as u32
    }
}

/// Where steps go. `Sync`: the speech stage reports from a thread per track.
pub type Steps<'a> = &'a (dyn Fn(Step) + Sync);

/// Steps thrown away (the CLI, which logs instead).
pub fn none() -> Steps<'static> {
    &|_| {}
}

/// A track's length: ffprobe's for its stream when it gave one, else the recording's (a track can
/// end before the video does).
pub fn track_length(report: &TrackReport, track: usize) -> f64 {
    report
        .tracks
        .get(track)
        .and_then(|a| a.duration)
        .filter(|d| *d > 0.0)
        .unwrap_or(report.duration)
}

/// `at` seconds into `track` for `stage`, clamped to the track's length (so `f64::INFINITY` is
/// the whole track).
pub fn step(report: &TrackReport, stage: &'static str, track: usize, at: f64) -> Step {
    let of = track_length(report, track);
    Step {
        stage,
        track,
        at: at.clamp(0.0, of.max(0.0)),
        of,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_whole_and_never_past_the_end() {
        let s = |at, of| Step {
            stage: "transcribe",
            track: 0,
            at,
            of,
        };
        assert_eq!(s(0.0, 100.0).percent(), 0);
        assert_eq!(s(99.9, 100.0).percent(), 99);
        assert_eq!(s(100.0, 100.0).percent(), 100);
        assert_eq!(s(250.0, 100.0).percent(), 100);
        // A track with no length has nothing left to do.
        assert_eq!(s(0.0, 0.0).percent(), 100);
    }
}
