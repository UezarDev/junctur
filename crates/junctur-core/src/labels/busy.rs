//! How busy a silent piece is, from the game's own audio: the game/PC track when the track roles
//! found one, else the mix, which is game audio wherever nobody talks. As a share of the recording's
//! seconds quieter than the piece, so it reads the same whatever the game's volume was set to.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use super::pieces::Piece;
use crate::tracks::{Role, TrackReport};
use crate::{waves, Result};

/// The track the score is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusyTrack {
    /// 0-based audio ordinal.
    pub track: usize,
    pub role: Role,
}

/// The first game/PC track that is not silent, else the first such mix; `None` when there is neither
/// (voices only).
pub fn track(report: &TrackReport) -> Option<BusyTrack> {
    [Role::GamePc, Role::Mix].into_iter().find_map(|role| {
        report
            .logical
            .iter()
            .find(|l| l.role == role && !l.activity.silent)
            .map(|l| BusyTrack {
                track: l.members[0],
                role,
            })
    })
}

/// The track's peak per [`waves::HOP`] (bytes on the -60..0 dBFS scale): from the app's `waves.u8` in
/// project folder `dir` when it reaches the end of the recording, else decoded for that track alone.
pub fn peaks(dir: &Path, file: &Path, report: &TrackReport, track: usize) -> Result<Vec<u8>> {
    let n = report.tracks.len();
    if let Some(w) = waves::load(dir, n).filter(|w| w.seconds() + 1.0 >= report.duration) {
        return Ok(w.data().iter().skip(track).step_by(n).copied().collect());
    }
    let mut out = Vec::new();
    let duration = (report.duration > 0.0).then_some(report.duration);
    waves::read_tracks(
        file,
        &[track],
        duration,
        &AtomicBool::new(false),
        &mut |more| out.extend_from_slice(more),
    )?;
    Ok(out)
}

/// A peak byte in dBFS.
fn db(byte: f64) -> f64 {
    waves::FLOOR_DB as f64 * (1.0 - byte / 255.0)
}

/// Each piece's `(busy, level in dBFS)` from `peaks`: its level is the mean of its peaks, and busy
/// the share of the recording's whole seconds whose mean is below it. `None` for a piece past the
/// peaks.
pub fn scores(peaks: &[u8], pieces: &[Piece]) -> Vec<Option<(f64, f64)>> {
    let per = waves::RATE as usize;
    let mut seconds: Vec<f64> = peaks
        .chunks_exact(per)
        .map(|s| s.iter().map(|&b| b as f64).sum::<f64>() / per as f64)
        .collect();
    seconds.sort_by(f64::total_cmp);
    pieces
        .iter()
        .map(|p| {
            let a = ((p.start / waves::HOP).floor().max(0.0) as usize).min(peaks.len());
            let b = ((p.end / waves::HOP).ceil() as usize).min(peaks.len());
            if b <= a || seconds.is_empty() {
                return None;
            }
            let mean = peaks[a..b].iter().map(|&x| x as f64).sum::<f64>() / (b - a) as f64;
            let below = seconds.partition_point(|&s| s < mean);
            Some((below as f64 / seconds.len() as f64, db(mean)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::{LogicalTrack, Sameness};

    #[test]
    fn busy_ranks_a_piece_among_the_recordings_seconds() {
        // Ten seconds: quiet (byte 10) but for seconds 6..8 (byte 200).
        let mut peaks = vec![10u8; 500];
        peaks[300..400].fill(200);
        let quiet = Piece {
            start: 0.0,
            end: 4.0,
        };
        let loud = Piece {
            start: 6.0,
            end: 8.0,
        };
        let past = Piece {
            start: 20.0,
            end: 30.0,
        };
        let s = scores(&peaks, &[quiet, loud, past]);
        let (b, level) = s[0].unwrap();
        assert_eq!(b, 0.0);
        assert!((level - db(10.0)).abs() < 1e-9);
        assert_eq!(s[1].unwrap().0, 0.8, "louder than the eight quiet seconds");
        assert_eq!(s[2], None);
        assert_eq!(scores(&[], &[quiet]), vec![None]);
    }

    #[test]
    fn bytes_read_as_the_waves_decibels() {
        assert_eq!(db(255.0), 0.0);
        assert_eq!(db(0.0), -60.0);
    }

    fn report(roles: &[Role]) -> TrackReport {
        quiet(roles, &[])
    }

    /// `roles`, the tracks in `silent` silent.
    fn quiet(roles: &[Role], silent: &[usize]) -> TrackReport {
        let logical = roles
            .iter()
            .enumerate()
            .map(|(i, &role)| LogicalTrack {
                members: vec![i],
                sameness: Sameness::Single,
                activity: crate::signal::Activity {
                    mean_db: -30.0,
                    peak_db: -10.0,
                    near_silent_share: 0.0,
                    silent: silent.contains(&i),
                },
                speech: None,
                role,
                confidence: 1.0,
                reasons: vec![],
            })
            .collect();
        TrackReport {
            file: "x.mp4".into(),
            duration: 10.0,
            windows: vec![],
            window_seconds: 1.0,
            tracks: vec![],
            logical,
            speech_error: None,
            from_cache: false,
        }
    }

    #[test]
    fn the_game_track_first_then_the_mix_then_none() {
        let t = track(&report(&[Role::Mix, Role::Voice, Role::GamePc])).unwrap();
        assert_eq!((t.track, t.role), (2, Role::GamePc));
        let t = track(&report(&[Role::Voice, Role::Mix])).unwrap();
        assert_eq!((t.track, t.role), (1, Role::Mix));
        assert_eq!(track(&report(&[Role::Voice, Role::Voice])), None);
        // A game track with nothing on it: the mix.
        let t = track(&quiet(&[Role::GamePc, Role::Mix], &[0])).unwrap();
        assert_eq!((t.track, t.role), (1, Role::Mix));
    }
}
