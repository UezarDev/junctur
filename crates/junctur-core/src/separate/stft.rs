//! STFT and inverse STFT matching `torch.stft`/`torch.istft` with `center=True`, reflect padding and
//! a periodic Hann window: the MDX-Net models were trained on exactly that framing.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub struct Stft {
    pub n_fft: usize,
    pub hop: usize,
    window: Vec<f32>,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
}

impl Stft {
    pub fn new(n_fft: usize, hop: usize) -> Self {
        let mut p = FftPlanner::new();
        let window = (0..n_fft)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / n_fft as f32).cos())
            .collect();
        Self {
            n_fft,
            hop,
            window,
            fwd: p.plan_fft_forward(n_fft),
            inv: p.plan_fft_inverse(n_fft),
        }
    }

    pub fn frames(&self, len: usize) -> usize {
        1 + len / self.hop
    }

    /// Bins per frame of the half spectrum: `n_fft/2 + 1`.
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Spectrum of `x`, frame-major and flat: bin `k` of frame `t` at `t * bins() + k`. One buffer
    /// and one FFT scratch for every frame.
    pub fn forward(&self, x: &[f32]) -> Vec<Complex32> {
        let (half, bins) = (self.n_fft / 2, self.bins());
        let padded = reflect_pad(x, half);
        let frames = self.frames(x.len());
        let mut out = Vec::with_capacity(frames * bins);
        let mut buf = vec![Complex32::default(); self.n_fft];
        let mut scratch = vec![Complex32::default(); self.fwd.get_inplace_scratch_len()];
        for t in 0..frames {
            let s = &padded[t * self.hop..t * self.hop + self.n_fft];
            for ((b, &v), &w) in buf.iter_mut().zip(s).zip(&self.window) {
                *b = Complex32::new(v * w, 0.0);
            }
            self.fwd.process_with_scratch(&mut buf, &mut scratch);
            out.extend_from_slice(&buf[..bins]);
        }
        out
    }

    /// Signal of length `len` from a flat frame-major half spectrum as [`Self::forward`] gives
    /// (overlap-add, divided by the summed squared window, centre padding trimmed).
    pub fn inverse(&self, spec: &[Complex32], len: usize) -> Vec<f32> {
        let (n, half, bins) = (self.n_fft, self.n_fft / 2, self.bins());
        debug_assert!(
            !spec.is_empty() && spec.len().is_multiple_of(bins),
            "whole frames of a half spectrum"
        );
        let frames = spec.len() / bins;
        let total = (frames - 1) * self.hop + n;
        let mut out = vec![0.0f32; total];
        let mut norm = vec![0.0f32; total];
        let mut buf = vec![Complex32::default(); n];
        let mut scratch = vec![Complex32::default(); self.inv.get_inplace_scratch_len()];
        for (t, frame) in spec.chunks_exact(bins).enumerate() {
            buf[..=half].copy_from_slice(&frame[..=half]);
            for k in 1..half {
                buf[n - k] = frame[k].conj();
            }
            self.inv.process_with_scratch(&mut buf, &mut scratch);
            let o = t * self.hop;
            for i in 0..n {
                out[o + i] += buf[i].re / n as f32 * self.window[i];
                norm[o + i] += self.window[i] * self.window[i];
            }
        }
        (0..len)
            .map(|i| {
                let j = i + half;
                if norm[j] > 1e-8 {
                    out[j] / norm[j]
                } else {
                    0.0
                }
            })
            .collect()
    }
}

fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    let at = |i: isize| -> f32 {
        // Reflect without repeating the edge sample, as torch does; fold until in range.
        let mut i = i;
        let m = (n as isize - 1).max(1);
        loop {
            if i < 0 {
                i = -i;
            } else if i > m {
                i = 2 * m - i;
            } else {
                return x.get(i as usize).copied().unwrap_or(0.0);
            }
        }
    };
    (0..n + 2 * pad)
        .map(|i| at(i as isize - pad as isize))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_identity() {
        let s = Stft::new(64, 16);
        let x: Vec<f32> = (0..1024)
            .map(|i| ((i * 7919) % 101) as f32 / 50.0 - 1.0)
            .collect();
        let spec = s.forward(&x);
        assert_eq!(spec.len(), (1 + 1024 / 16) * s.bins());
        let y = s.inverse(&spec, x.len());
        let err = x
            .iter()
            .zip(&y)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    /// The forward transform as written before the flat spectrum and the reused scratch: a Vec
    /// per frame, rustfft's own scratch per call.
    fn per_frame_forward(s: &Stft, x: &[f32]) -> Vec<Vec<Complex32>> {
        let half = s.n_fft / 2;
        let padded = reflect_pad(x, half);
        let mut buf = vec![Complex32::default(); s.n_fft];
        (0..s.frames(x.len()))
            .map(|t| {
                let w = &padded[t * s.hop..t * s.hop + s.n_fft];
                for ((b, &v), &h) in buf.iter_mut().zip(w).zip(&s.window) {
                    *b = Complex32::new(v * h, 0.0);
                }
                s.fwd.process(&mut buf);
                buf[..=half].to_vec()
            })
            .collect()
    }

    /// The inverse as written before, over a Vec per frame.
    fn per_frame_inverse(s: &Stft, spec: &[Vec<Complex32>], len: usize) -> Vec<f32> {
        let (n, half) = (s.n_fft, s.n_fft / 2);
        let total = (spec.len() - 1) * s.hop + n;
        let (mut out, mut norm) = (vec![0.0f32; total], vec![0.0f32; total]);
        let mut buf = vec![Complex32::default(); n];
        for (t, frame) in spec.iter().enumerate() {
            buf[..=half].copy_from_slice(&frame[..=half]);
            for k in 1..half {
                buf[n - k] = frame[k].conj();
            }
            s.inv.process(&mut buf);
            for i in 0..n {
                out[t * s.hop + i] += buf[i].re / n as f32 * s.window[i];
                norm[t * s.hop + i] += s.window[i] * s.window[i];
            }
        }
        (0..len)
            .map(|i| {
                let j = i + half;
                if norm[j] > 1e-8 {
                    out[j] / norm[j]
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// Forward and inverse give bit for bit what the per-frame versions gave.
    #[test]
    fn transforms_match_the_per_frame_version_bit_for_bit() {
        let s = Stft::new(7680, 1024);
        let x: Vec<f32> = (0..40_000)
            .map(|i| (i as f32 * 0.021).sin() * 0.4 + ((i * 7919) % 101) as f32 * 1e-3)
            .collect();
        let old = per_frame_forward(&s, &x);
        let new = s.forward(&x);
        let flat: Vec<Complex32> = old.iter().flatten().copied().collect();
        let bits = |v: &[Complex32]| -> Vec<(u32, u32)> {
            v.iter().map(|c| (c.re.to_bits(), c.im.to_bits())).collect()
        };
        assert_eq!(bits(&new), bits(&flat));
        let f32_bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            f32_bits(&s.inverse(&new, x.len())),
            f32_bits(&per_frame_inverse(&s, &old, x.len()))
        );
    }

    #[test]
    fn reflect_matches_torch() {
        assert_eq!(
            reflect_pad(&[1.0, 2.0, 3.0], 2),
            vec![3.0, 2.0, 1.0, 2.0, 3.0, 2.0, 1.0]
        );
    }
}
