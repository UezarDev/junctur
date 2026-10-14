//! MDX-Net vocal model: one fixed-size stereo segment in, its vocal part out.
//!
//! The model sees a spectrogram, not samples: `[batch, 4, dim_f, dim_t]` with channels ordered
//! left-real, left-imag, right-real, right-imag, and only the lowest `dim_f` bins (the rest are zero
//! on the way back). Parameters are the ones UVR ships for Kim Vocal 2.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use rustfft::num_complex::Complex32;

use super::stft::Stft;
use crate::runtime::{self, Device, DevicePref};
use crate::Result;

pub const RATE: u32 = 44_100;
const N_FFT: usize = 7680;
const HOP: usize = 1024;
const DIM_F: usize = 3072;
const DIM_T: usize = 256;
/// UVR's level correction for this model's vocal output.
const COMPENSATE: f32 = 1.009;
/// Samples per model call: exactly `DIM_T` STFT frames.
pub const CHUNK: usize = HOP * (DIM_T - 1);
/// Edge samples of each chunk thrown away, where the STFT sees padding instead of signal.
pub const TRIM: usize = N_FFT / 2;
/// Fresh samples each chunk contributes.
pub const STEP: usize = CHUNK - 2 * TRIM;

pub struct Mdx {
    session: Session,
    pub device: Device,
    stft: Stft,
}

impl Mdx {
    pub fn open(model: &Path, pref: DevicePref) -> Result<Self> {
        let (session, device) = runtime::open(model, pref)?;
        Ok(Self {
            session,
            device,
            stft: Stft::new(N_FFT, HOP),
        })
    }

    /// Vocals of one stereo chunk (`left`, `right`, each [`CHUNK`] samples).
    pub fn vocals(&mut self, left: &[f32], right: &[f32]) -> Result<[Vec<f32>; 2]> {
        let input = model_input(&self.stft, left, right);
        let tensor = Tensor::from_array(([1usize, 4, DIM_F, DIM_T], input))?;
        let res = self.session.run(ort::inputs!["input" => tensor])?;
        let out = res["output"].try_extract_tensor::<f32>()?.1;
        Ok(stems_from(&self.stft, out))
    }
}

/// The model's input for one stereo chunk: `[4][DIM_F][DIM_T]`, written in that order (reading
/// the frame-major spectrum across frames) so the writes run straight through memory.
fn model_input(stft: &Stft, left: &[f32], right: &[f32]) -> Vec<f32> {
    let bins = stft.bins();
    let mut input = Vec::with_capacity(4 * DIM_F * DIM_T);
    for x in [left, right] {
        let spec = stft.forward(x);
        let parts: [fn(&Complex32) -> f32; 2] = [|v| v.re, |v| v.im];
        for part in parts {
            for f in 0..DIM_F {
                input.extend((0..DIM_T).map(|t| part(&spec[t * bins + f])));
            }
        }
    }
    input
}

/// Both channels' vocals from the model's output, the bins past `DIM_F` zero.
fn stems_from(stft: &Stft, out: &[f32]) -> [Vec<f32>; 2] {
    let (plane, bins) = (DIM_F * DIM_T, stft.bins());
    [0, 1].map(|c| {
        let (re, im) = (&out[2 * c * plane..], &out[(2 * c + 1) * plane..]);
        let mut spec = vec![Complex32::default(); DIM_T * bins];
        for f in 0..DIM_F {
            for t in 0..DIM_T {
                spec[t * bins + f] = Complex32::new(re[f * DIM_T + t], im[f * DIM_T + t]);
            }
        }
        stft.inverse(&spec, CHUNK)
            .into_iter()
            .map(|x| x * COMPENSATE)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic stereo: two tones and a little hash noise per channel.
    fn chunk(seed: usize) -> Vec<f32> {
        (0..CHUNK)
            .map(|i| {
                let t = i as f32;
                0.3 * (t * 0.013 * (seed + 1) as f32).sin()
                    + 0.1 * (t * 0.41).sin()
                    + ((i * 7919 + seed) % 89) as f32 * 1e-3
            })
            .collect()
    }

    /// The input and the stems as built before the flat spectrum (a Vec per frame, written
    /// across the planes), bit for bit.
    #[test]
    fn layout_matches_the_per_frame_version_bit_for_bit() {
        let stft = Stft::new(N_FFT, HOP);
        let bins = stft.bins();
        let frames_of = |x: &[f32]| -> Vec<Vec<Complex32>> {
            stft.forward(x)
                .chunks_exact(bins)
                .map(<[_]>::to_vec)
                .collect()
        };
        let (left, right) = (chunk(0), chunk(1));
        let plane = DIM_F * DIM_T;
        let mut old = vec![0.0f32; 4 * plane];
        for (c, x) in [&left, &right].into_iter().enumerate() {
            for (t, frame) in frames_of(x).iter().enumerate() {
                for (f, v) in frame[..DIM_F].iter().enumerate() {
                    old[(2 * c) * plane + f * DIM_T + t] = v.re;
                    old[(2 * c + 1) * plane + f * DIM_T + t] = v.im;
                }
            }
        }
        let new = model_input(&stft, &left, &right);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&new), bits(&old));

        // Any output tensor: the input itself, scaled, stands in for the model's.
        let out: Vec<f32> = old.iter().map(|v| v * 0.5).collect();
        let stems = stems_from(&stft, &out);
        for (c, stem) in stems.iter().enumerate() {
            let spec: Vec<Complex32> = (0..DIM_T)
                .flat_map(|t| {
                    let mut frame = vec![Complex32::default(); bins];
                    for (f, v) in frame[..DIM_F].iter_mut().enumerate() {
                        *v = Complex32::new(
                            out[(2 * c) * plane + f * DIM_T + t],
                            out[(2 * c + 1) * plane + f * DIM_T + t],
                        );
                    }
                    frame
                })
                .collect();
            let want: Vec<f32> = stft
                .inverse(&spec, CHUNK)
                .into_iter()
                .map(|x| x * COMPENSATE)
                .collect();
            assert_eq!(bits(stem), bits(&want));
        }
    }
}
