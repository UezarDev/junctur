//! Pure signal measurements on decoded samples: loudness, sameness, and "is this the sum of those".
//!
//! Kept apart from the ffmpeg and report code so every threshold here is unit-tested on synthetic
//! buffers, with no decoding involved.

use serde::{Deserialize, Serialize};

/// Loudness is measured per 100 ms frame.
pub const FRAME_SECONDS: f64 = 0.1;
/// dBFS reported for digital silence instead of -inf.
pub const FLOOR_DB: f64 = -120.0;
/// A frame below this is "near-silent". -45 dB is the line the 2026-10-09 file survey used.
pub const NEAR_SILENT_DB: f64 = -45.0;
/// A track whose loudest frame stays below this is treated as silent and dropped. The silent
/// third track of the 3-track recording measured -91 dB; real faint audio peaks far above this.
pub const SILENT_PEAK_DB: f64 = -80.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    /// Energy mean over all sampled frames, in dBFS.
    pub mean_db: f64,
    /// Loudest 100 ms frame, in dBFS.
    pub peak_db: f64,
    /// Share of sampled frames below [`NEAR_SILENT_DB`], 0..=1.
    pub near_silent_share: f64,
    pub silent: bool,
}

fn db(power: f64) -> f64 {
    if power <= 0.0 {
        FLOOR_DB
    } else {
        (10.0 * power.log10()).max(FLOOR_DB)
    }
}

pub fn activity(samples: &[f32], rate: u32) -> Activity {
    let frame = ((rate as f64 * FRAME_SECONDS) as usize).max(1);
    let powers: Vec<f64> = samples
        .chunks(frame)
        .map(|c| c.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / c.len() as f64)
        .collect();
    if powers.is_empty() {
        return Activity {
            mean_db: FLOOR_DB,
            peak_db: FLOOR_DB,
            near_silent_share: 1.0,
            silent: true,
        };
    }
    let mean_db = db(powers.iter().sum::<f64>() / powers.len() as f64);
    let peak_db = db(powers.iter().cloned().fold(0.0, f64::max));
    let quiet = powers.iter().filter(|&&p| db(p) < NEAR_SILENT_DB).count();
    Activity {
        mean_db,
        peak_db,
        near_silent_share: quiet as f64 / powers.len() as f64,
        silent: peak_db < SILENT_PEAK_DB,
    }
}

/// Bit-for-bit equality of two decodes. Decoding is deterministic, so copies of one stream decode
/// to identical samples; this is the cheap first test before any correlation.
pub fn identical(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

/// Pearson correlation at zero lag. None when either side has no variance (e.g. silence).
pub fn correlation(a: &[f32], b: &[f32]) -> Option<f64> {
    let n = a.len().min(b.len());
    if n == 0 {
        return None;
    }
    let (ma, mb) = (mean(&a[..n]), mean(&b[..n]));
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (x, y) = (a[i] as f64 - ma, b[i] as f64 - mb);
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    if saa <= 0.0 || sbb <= 0.0 {
        return None;
    }
    Some(sab / (saa * sbb).sqrt())
}

fn mean(a: &[f32]) -> f64 {
    a.iter().map(|&x| x as f64).sum::<f64>() / a.len() as f64
}

/// How well `target` is explained as a weighted sum of `parts` (least squares, zero lag).
/// Returns the residual energy as a share of the target's energy (near 0 means the fit is exact)
/// and the fitted weights. None when the system is degenerate (silent target or parts).
///
/// The weights matter: when A = B + C, B = A - C fits just as exactly, so a caller deciding which
/// track is the mix must also require non-negative weights.
pub fn sum_fit(target: &[f32], parts: &[&[f32]]) -> Option<(f64, Vec<f64>)> {
    let k = parts.len();
    let n = parts.iter().map(|p| p.len()).chain([target.len()]).min()?;
    if k == 0 || n == 0 {
        return None;
    }
    // Normal equations: (X^T X) w = X^T y.
    let mut m = vec![vec![0.0f64; k + 1]; k];
    for i in 0..k {
        for j in 0..k {
            m[i][j] = (0..n)
                .map(|t| parts[i][t] as f64 * parts[j][t] as f64)
                .sum();
        }
        m[i][k] = (0..n).map(|t| parts[i][t] as f64 * target[t] as f64).sum();
    }
    let w = solve(m)?;
    let energy: f64 = target[..n].iter().map(|&y| (y as f64).powi(2)).sum();
    if energy <= 0.0 {
        return None;
    }
    let resid: f64 = (0..n)
        .map(|t| {
            let fit: f64 = (0..k).map(|i| w[i] * parts[i][t] as f64).sum();
            (target[t] as f64 - fit).powi(2)
        })
        .sum();
    Some((resid / energy, w))
}

/// Gaussian elimination with partial pivoting on an augmented k x (k+1) matrix.
fn solve(mut m: Vec<Vec<f64>>) -> Option<Vec<f64>> {
    let k = m.len();
    for c in 0..k {
        let p = (c..k).max_by(|&a, &b| m[a][c].abs().total_cmp(&m[b][c].abs()))?;
        if m[p][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, p);
        for r in 0..k {
            if r != c {
                let f = m[r][c] / m[c][c];
                let pivot = m[c].clone();
                for (x, p) in m[r].iter_mut().zip(&pivot).skip(c) {
                    *x -= f * p;
                }
            }
        }
    }
    Some((0..k).map(|i| m[i][k] / m[i][i]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (i as f32 * freq * std::f32::consts::TAU / 16_000.0).sin())
            .collect()
    }

    #[test]
    fn digital_silence_is_silent() {
        let a = activity(&vec![0.0; 16_000], 16_000);
        assert!(a.silent);
        assert_eq!(a.near_silent_share, 1.0);
        assert_eq!(a.mean_db, FLOOR_DB);
    }

    #[test]
    fn empty_input_is_silent() {
        assert!(activity(&[], 16_000).silent);
    }

    #[test]
    fn faint_tone_is_quiet_but_not_silent() {
        // amplitude 0.001 ~ -63 dBFS RMS.
        let a = activity(&sine(440.0, 0.001, 16_000), 16_000);
        assert!(!a.silent);
        assert!(a.mean_db < -60.0 && a.mean_db > -66.0, "{}", a.mean_db);
        assert_eq!(a.near_silent_share, 1.0);
    }

    #[test]
    fn half_on_tone_reports_half_near_silent() {
        let mut s = sine(440.0, 0.5, 8_000);
        s.extend(vec![0.0; 8_000]);
        let a = activity(&s, 16_000);
        assert!((a.near_silent_share - 0.5).abs() < 0.01);
    }

    #[test]
    fn identity_is_bitwise() {
        let a = sine(440.0, 0.5, 100);
        let mut b = a.clone();
        assert!(identical(&a, &b));
        b[3] += 1e-6;
        assert!(!identical(&a, &b));
        assert!(correlation(&a, &b).unwrap() > 0.9999);
    }

    #[test]
    fn unrelated_signals_do_not_correlate() {
        let c = correlation(&sine(440.0, 0.5, 16_000), &sine(1234.0, 0.5, 16_000)).unwrap();
        assert!(c.abs() < 0.05, "{c}");
        assert_eq!(correlation(&[0.0; 10], &[1.0; 10]), None);
    }

    #[test]
    fn mix_is_explained_by_its_parts() {
        let a = sine(300.0, 0.3, 16_000);
        let b = sine(1700.0, 0.2, 16_000);
        let mix: Vec<f32> = a.iter().zip(&b).map(|(x, y)| 0.8 * x + 0.5 * y).collect();
        let (r, w) = sum_fit(&mix, &[&a, &b]).unwrap();
        assert!(r < 1e-6);
        assert!((w[0] - 0.8).abs() < 1e-3 && (w[1] - 0.5).abs() < 1e-3);
        // A part is also an exact fit of the mix and the other part, but with a negative weight.
        let (r, w) = sum_fit(&a, &[&mix, &b]).unwrap();
        assert!(r < 1e-6 && w[1] < 0.0);
        let other = sine(5000.0, 0.3, 16_000);
        assert!(sum_fit(&other, &[&a, &b]).unwrap().0 > 0.9);
    }
}
