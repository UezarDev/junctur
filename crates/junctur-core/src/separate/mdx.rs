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
        let plane = DIM_F * DIM_T;
        let mut input = vec![0.0f32; 4 * plane];
        for (c, x) in [left, right].into_iter().enumerate() {
            for (t, frame) in self.stft.forward(x).iter().enumerate() {
                for (f, v) in frame[..DIM_F].iter().enumerate() {
                    input[(2 * c) * plane + f * DIM_T + t] = v.re;
                    input[(2 * c + 1) * plane + f * DIM_T + t] = v.im;
                }
            }
        }
        let tensor = Tensor::from_array(([1usize, 4, DIM_F, DIM_T], input))?;
        let res = self.session.run(ort::inputs!["input" => tensor])?;
        let out = res["output"].try_extract_tensor::<f32>()?.1;
        let bins = N_FFT / 2 + 1;
        let mut stems = [Vec::new(), Vec::new()];
        for (c, stem) in stems.iter_mut().enumerate() {
            let spec: Vec<Vec<Complex32>> = (0..DIM_T)
                .map(|t| {
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
            *stem = self
                .stft
                .inverse(&spec, CHUNK)
                .into_iter()
                .map(|x| x * COMPENSATE)
                .collect();
        }
        Ok(stems)
    }
}
