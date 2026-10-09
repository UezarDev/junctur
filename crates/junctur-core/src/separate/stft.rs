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

    /// Spectrum of `x`, frame-major: `out[t][k]` for bins `k < n_fft/2 + 1`.
    pub fn forward(&self, x: &[f32]) -> Vec<Vec<Complex32>> {
        let half = self.n_fft / 2;
        let padded = reflect_pad(x, half);
        let mut buf = vec![Complex32::default(); self.n_fft];
        (0..self.frames(x.len()))
            .map(|t| {
                let s = &padded[t * self.hop..t * self.hop + self.n_fft];
                for ((b, &v), &w) in buf.iter_mut().zip(s).zip(&self.window) {
                    *b = Complex32::new(v * w, 0.0);
                }
                self.fwd.process(&mut buf);
                buf[..=half].to_vec()
            })
            .collect()
    }

    /// Signal of length `len` from a frame-major half spectrum (overlap-add, divided by the summed
    /// squared window, centre padding trimmed).
    pub fn inverse(&self, spec: &[Vec<Complex32>], len: usize) -> Vec<f32> {
        let (n, half) = (self.n_fft, self.n_fft / 2);
        let total = (spec.len() - 1) * self.hop + n;
        let mut out = vec![0.0f32; total];
        let mut norm = vec![0.0f32; total];
        let mut buf = vec![Complex32::default(); n];
        for (t, frame) in spec.iter().enumerate() {
            buf[..=half].copy_from_slice(&frame[..=half]);
            for k in 1..half {
                buf[n - k] = frame[k].conj();
            }
            self.inv.process(&mut buf);
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
        assert_eq!(spec.len(), 1 + 1024 / 16);
        let y = s.inverse(&spec, x.len());
        let err = x
            .iter()
            .zip(&y)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    #[test]
    fn reflect_matches_torch() {
        assert_eq!(
            reflect_pad(&[1.0, 2.0, 3.0], 2),
            vec![3.0, 2.0, 1.0, 2.0, 3.0, 2.0, 1.0]
        );
    }
}
