//! Kaldi-style log-mel filterbank (80 bins, 25 ms / 10 ms), the features the 3D-Speaker models take.
//!
//! Matches Kaldi's defaults that matter for an embedding: DC removal, 0.97 pre-emphasis, Povey
//! window, power spectrum, mel bins from 20 Hz to Nyquist, natural log, no dither.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub const BINS: usize = 80;
const RATE: f32 = 16_000.0;
const WIN: usize = 400;
const SHIFT: usize = 160;
const N_FFT: usize = 512;

pub struct Fbank {
    window: Vec<f32>,
    /// Per mel bin: first FFT bin and its weights.
    banks: Vec<(usize, Vec<f32>)>,
    fft: Arc<dyn Fft<f32>>,
}

fn mel(f: f32) -> f32 {
    1127.0 * (1.0 + f / 700.0).ln()
}

impl Default for Fbank {
    fn default() -> Self {
        Self::new()
    }
}

impl Fbank {
    pub fn new() -> Self {
        let window = (0..WIN)
            .map(|n| {
                let h = 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / (WIN - 1) as f32).cos();
                h.powf(0.85)
            })
            .collect();
        let (lo, hi) = (mel(20.0), mel(RATE / 2.0));
        let delta = (hi - lo) / (BINS + 1) as f32;
        let fft_bins = N_FFT / 2;
        let banks = (0..BINS)
            .map(|b| {
                let (l, c, r) = (
                    lo + b as f32 * delta,
                    lo + (b + 1) as f32 * delta,
                    lo + (b + 2) as f32 * delta,
                );
                let mut first = None;
                let mut w = Vec::new();
                for k in 0..fft_bins {
                    let m = mel(k as f32 * RATE / N_FFT as f32);
                    let v = if m > l && m < c {
                        (m - l) / (c - l)
                    } else if m >= c && m < r {
                        (r - m) / (r - c)
                    } else {
                        0.0
                    };
                    if v > 0.0 {
                        first.get_or_insert(k);
                        w.push(v);
                    } else if first.is_some() {
                        break;
                    }
                }
                (first.unwrap_or(0), w)
            })
            .collect();
        Self {
            window,
            banks,
            fft: FftPlanner::new().plan_fft_forward(N_FFT),
        }
    }

    /// Frame-major features `[frames][BINS]`, mean-normalised over time (the models' "global-mean").
    pub fn compute(&self, x: &[f32]) -> Vec<[f32; BINS]> {
        if x.len() < WIN {
            return Vec::new();
        }
        let n = 1 + (x.len() - WIN) / SHIFT;
        let mut buf = vec![Complex32::default(); N_FFT];
        // One frame and one FFT scratch per call, not per frame: a 30 h track is ~10 M frames.
        let mut scratch = vec![Complex32::default(); self.fft.get_inplace_scratch_len()];
        let mut f = [0.0f32; WIN];
        let mut feats: Vec<[f32; BINS]> = (0..n)
            .map(|t| {
                f.copy_from_slice(&x[t * SHIFT..t * SHIFT + WIN]);
                let mean = f.iter().sum::<f32>() / WIN as f32;
                f.iter_mut().for_each(|v| *v -= mean);
                for i in (1..WIN).rev() {
                    f[i] -= 0.97 * f[i - 1];
                }
                f[0] -= 0.97 * f[0];
                buf.fill(Complex32::default());
                for i in 0..WIN {
                    buf[i].re = f[i] * self.window[i];
                }
                self.fft.process_with_scratch(&mut buf, &mut scratch);
                let mut row = [0.0f32; BINS];
                for (b, (first, w)) in self.banks.iter().enumerate() {
                    let e: f32 = w
                        .iter()
                        .enumerate()
                        .map(|(i, wt)| wt * buf[first + i].norm_sqr())
                        .sum();
                    row[b] = e.max(f32::EPSILON).ln();
                }
                row
            })
            .collect();
        let mut mean = [0.0f32; BINS];
        for r in &feats {
            for (m, v) in mean.iter_mut().zip(r) {
                *m += v / n as f32;
            }
        }
        for r in &mut feats {
            for (v, m) in r.iter_mut().zip(&mean) {
                *v -= m;
            }
        }
        feats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_and_mean_normalised() {
        let x: Vec<f32> = (0..16_000)
            .map(|i| (i as f32 * 0.05).sin() * 0.3 + (i as f32 * 0.31).sin() * 0.1)
            .collect();
        let f = Fbank::new().compute(&x);
        assert_eq!(f.len(), 1 + (16_000 - 400) / 160);
        let m: f32 = f.iter().map(|r| r[10]).sum::<f32>() / f.len() as f32;
        assert!(m.abs() < 1e-3);
        assert!(Fbank::new().compute(&[0.0; 100]).is_empty());
    }

    /// The features as computed before the reused frame and scratch.
    fn allocating(fb: &Fbank, x: &[f32]) -> Vec<[f32; BINS]> {
        let n = 1 + (x.len() - WIN) / SHIFT;
        let mut buf = vec![Complex32::default(); N_FFT];
        let mut feats: Vec<[f32; BINS]> = (0..n)
            .map(|t| {
                let mut f: Vec<f32> = x[t * SHIFT..t * SHIFT + WIN].to_vec();
                let mean = f.iter().sum::<f32>() / WIN as f32;
                f.iter_mut().for_each(|v| *v -= mean);
                for i in (1..WIN).rev() {
                    f[i] -= 0.97 * f[i - 1];
                }
                f[0] -= 0.97 * f[0];
                buf.fill(Complex32::default());
                for i in 0..WIN {
                    buf[i].re = f[i] * fb.window[i];
                }
                fb.fft.process(&mut buf);
                let mut row = [0.0f32; BINS];
                for (b, (first, w)) in fb.banks.iter().enumerate() {
                    let e: f32 = w
                        .iter()
                        .enumerate()
                        .map(|(i, wt)| wt * buf[first + i].norm_sqr())
                        .sum();
                    row[b] = e.max(f32::EPSILON).ln();
                }
                row
            })
            .collect();
        let mut mean = [0.0f32; BINS];
        for r in &feats {
            for (m, v) in mean.iter_mut().zip(r) {
                *m += v / n as f32;
            }
        }
        for r in &mut feats {
            for (v, m) in r.iter_mut().zip(&mean) {
                *v -= m;
            }
        }
        feats
    }

    /// Bit for bit what [`allocating`] gave.
    #[test]
    fn features_match_the_allocating_version_bit_for_bit() {
        let fb = Fbank::new();
        let x: Vec<f32> = (0..24_123)
            .map(|i| {
                let t = i as f32;
                0.3 * (t * 0.031).sin() + 0.1 * (t * 0.77).sin() + ((i * 7919) % 97) as f32 * 1e-3
            })
            .collect();
        let bits =
            |f: &[[f32; BINS]]| -> Vec<u32> { f.iter().flatten().map(|v| v.to_bits()).collect() };
        assert_eq!(bits(&fb.compute(&x)), bits(&allocating(&fb, &x)));
        assert_eq!(
            bits(&fb.compute(&x[..400])),
            bits(&allocating(&fb, &x[..400]))
        );
    }

    #[test]
    fn every_bank_has_weights() {
        assert!(Fbank::new().banks.iter().all(|(_, w)| !w.is_empty()));
    }
}
