//! "The picture does not move": how much each fifth of a second differs from the one before, on
//! 48x27 grey frames, kept in the project folder as one byte per step ([`FILE`]) so a stretch is read
//! from the video only once; a recipe over a selection reads only the selection. Rejected: ffmpeg's
//! `freezedetect`, which answers only "frozen from..to" for one threshold, so another threshold or a
//! stretch read twice decodes again; and the encoder's packet sizes (no decode, 3 s for 1h45), which
//! on the sample followed the picture's change too loosely to tell a still picture (r = 0.23).

use std::path::{Path, PathBuf};

use crate::ffmpeg::frames;
use crate::Result;

/// The file in the project folder.
pub const FILE: &str = "still.u8";
/// Steps a second.
pub const RATE: u32 = 5;
const SIZE: (u32, u32) = (48, 27);
/// A step not read yet.
const UNREAD: u8 = 255;
/// Mean absolute difference (0-255 grey) times this is the stored byte.
const SCALE: f64 = 8.0;
/// Below this mean difference between two steps the picture is still: a game's idle shimmer stays
/// under it, a turn of the camera or a menu opening goes far over (on the 1h45 sample half the
/// steps are over 2.1, a tenth over 6.5).
pub const STILL_BELOW: f64 = 1.0;

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// The steps kept for a recording `duration` long: as read so far, [`UNREAD`] elsewhere; a file
/// of another length (another recording's, cut short) is read again.
fn load(dir: &Path, steps: usize) -> Vec<u8> {
    match std::fs::read(path(dir)) {
        Ok(b) if b.len() == steps => b,
        _ => vec![UNREAD; steps],
    }
}

fn steps(duration: f64) -> usize {
    (duration * RATE as f64).ceil() as usize + 1
}

/// Each step's difference from the one before over `a..b` of `file` (its length `duration`),
/// reading from the video only the steps not kept yet, and keeping what it read.
pub fn differences(dir: &Path, file: &Path, duration: f64, (a, b): (f64, f64)) -> Result<Vec<u8>> {
    let n = steps(duration);
    let mut kept = load(dir, n);
    let from = ((a * RATE as f64).floor().max(1.0) as usize).min(n);
    let to = ((b * RATE as f64).ceil() as usize).min(n);
    let mut i = from;
    let mut changed = false;
    while i < to {
        if kept[i] != UNREAD {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < to && kept[j] == UNREAD {
            j += 1;
        }
        // From the step before the first unread one, which only gives the first difference.
        let start = (i - 1) as f64 / RATE as f64;
        let len = (j - i + 1) as f64 / RATE as f64;
        let mut prev: Option<Vec<u8>> = None;
        let mut k = i;
        frames::gray(file, start, len, RATE, SIZE, &mut |px| {
            if let Some(p) = &prev {
                if k < j {
                    kept[k] = byte(mad(p, px));
                    k += 1;
                }
            }
            prev = Some(px.to_vec());
            k < j
        })?;
        // Past the video's end (or a frame ffmpeg did not give): moving, so never cut as still.
        for s in &mut kept[k..j] {
            *s = UNREAD - 1;
        }
        changed = true;
        i = j;
    }
    if changed {
        crate::output::write_replacing(&path(dir), &kept)?;
    }
    Ok(kept[from..to].to_vec())
}

fn mad(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    sum as f64 / a.len().max(1) as f64
}

fn byte(mad: f64) -> u8 {
    (mad * SCALE).round().min((UNREAD - 1) as f64) as u8
}

/// The still stretches among `diffs`, the differences of steps `first..`: a run of still steps
/// `i..=j` is the picture unchanged from step `i - 1` to `j`.
pub fn spans(diffs: &[u8], first: usize) -> Vec<(f64, f64)> {
    let line = byte(STILL_BELOW);
    let t = |i: usize| i as f64 / RATE as f64;
    let mut out = Vec::new();
    let mut run: Option<usize> = None;
    for (k, &d) in diffs.iter().enumerate() {
        match (d < line, run) {
            (true, None) => run = Some(k),
            (false, Some(s)) => {
                out.push((t(first + s - 1), t(first + k - 1)));
                run = None;
            }
            _ => {}
        }
    }
    if let Some(s) = run {
        out.push((t(first + s - 1), t(first + diffs.len() - 1)));
    }
    out
}

/// The still stretches of `a..b`.
pub fn still(dir: &Path, file: &Path, duration: f64, range: (f64, f64)) -> Result<Vec<(f64, f64)>> {
    let first = ((range.0 * RATE as f64).floor().max(1.0) as usize).min(steps(duration));
    let d = differences(dir, file, duration, range)?;
    Ok(spans(&d, first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_of_still_steps_is_the_stretch_it_spans() {
        let line = byte(STILL_BELOW);
        let d = [50, line - 1, 0, 50, 0, 0];
        // Steps 10..16: still from 10 to 12 (steps 11 and 12), and from 13 to the end (14, 15).
        assert_eq!(spans(&d, 10), [(2.0, 2.4), (2.6, 3.0)]);
        assert!(spans(&[], 3).is_empty());
        assert_eq!(byte(1000.0), UNREAD - 1);
        assert_eq!(mad(&[0, 10], &[4, 0]), 7.0);
    }
}
