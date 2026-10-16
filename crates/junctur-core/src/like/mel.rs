//! A track's log-mel spectrogram, what "find all like this" compares sounds on (7b): 64 mel bands
//! every 10 ms, a byte each. Built once per track by streaming it through ffmpeg (a 30 h track never
//! sits in memory as samples) and kept in the project folder, so every later search only reads it.
//! Not `diarize::fbank` as it is: that one is 80 Kaldi bins normalised over each call's own mean, so
//! two chunks of one stream would not agree; the mel filters (`signal::mel_banks`) are shared instead.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::ffmpeg::{PcmReader, Span, ANALYSIS_RATE};
use crate::signal::mel_banks;
use crate::{output, Error, Result};

/// Mel bands per frame.
pub const BANDS: usize = 64;
/// Seconds per frame.
pub const HOP: f64 = SHIFT as f64 / ANALYSIS_RATE as f64;
/// 25 ms windows every 10 ms at 16 kHz.
const WIN: usize = 400;
const SHIFT: usize = 160;
const N_FFT: usize = 512;
/// Below this the game's rumble and the mic's hum are all there is.
const LOW_HZ: f32 = 50.0;
/// Byte 0 is this many dB (a full-scale sine is about -6 dB); each step is [`STEP_DB`].
const FLOOR_DB: f32 = -100.0;
const STEP_DB: f32 = 0.4;
const MAGIC: &[u8; 8] = b"JLIKEM1\n";
/// Samples read from ffmpeg at a time: 10 s.
const READ: usize = ANALYSIS_RATE as usize * 10;

/// Frames of [`BANDS`] bytes, frame-major: `data()[frame * BANDS + band]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spectrogram {
    data: Vec<u8>,
}

impl Spectrogram {
    /// From frame-major bytes (a whole number of frames).
    pub fn from_bytes(data: Vec<u8>) -> Self {
        debug_assert_eq!(data.len() % BANDS, 0);
        Self { data }
    }

    /// The spectrogram of 16 kHz mono `samples`.
    pub fn of_samples(samples: &[f32]) -> Self {
        let mut m = Mel::new();
        m.push(samples);
        m.finish()
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn frames(&self) -> usize {
        self.data.len() / BANDS
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 * HOP
    }
}

/// The streaming builder: samples in, whole frames out as they complete.
pub struct Mel {
    window: Vec<f32>,
    /// The window's energy: power is divided by it, so a full-scale sine reads the same whatever
    /// the window.
    norm: f32,
    banks: Vec<(usize, Vec<f32>)>,
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    buf: Vec<Complex32>,
    /// Samples not yet in a frame (fewer than a window, or the overlap with the next one).
    tail: Vec<f32>,
    data: Vec<u8>,
}

impl Default for Mel {
    fn default() -> Self {
        Self::new()
    }
}

impl Mel {
    pub fn new() -> Self {
        let window: Vec<f32> = (0..WIN)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / WIN as f32).cos())
            .collect();
        let sum: f32 = window.iter().sum();
        let fft = FftPlanner::new().plan_fft_forward(N_FFT);
        Self {
            norm: sum * sum,
            window,
            banks: mel_banks(BANDS, N_FFT, ANALYSIS_RATE as f32, LOW_HZ),
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            buf: vec![Complex32::default(); N_FFT],
            tail: Vec::new(),
            data: Vec::new(),
        }
    }

    /// Takes the next samples; every frame they complete is added.
    pub fn push(&mut self, x: &[f32]) {
        self.tail.extend_from_slice(x);
        let mut at = 0;
        while at + WIN <= self.tail.len() {
            self.buf.fill(Complex32::default());
            for i in 0..WIN {
                self.buf[i].re = self.tail[at + i] * self.window[i];
            }
            self.fft
                .process_with_scratch(&mut self.buf, &mut self.scratch);
            for (first, w) in &self.banks {
                let e: f32 = w
                    .iter()
                    .enumerate()
                    .map(|(i, wt)| wt * self.buf[first + i].norm_sqr())
                    .sum();
                let db = 10.0 * (e / self.norm).max(1e-12).log10();
                self.data
                    .push(((db - FLOOR_DB) / STEP_DB).round().clamp(0.0, 255.0) as u8);
            }
            at += SHIFT;
        }
        self.tail.drain(..at);
    }

    pub fn finish(self) -> Spectrogram {
        Spectrogram { data: self.data }
    }
}

/// The cached spectrogram of 0-based audio track `track` in project folder `dir`.
pub fn path(dir: &Path, track: usize) -> PathBuf {
    dir.join(format!("like.{track}.mel"))
}

/// The cached spectrogram, when there is one that reads whole.
pub fn load(dir: &Path, track: usize) -> Option<Spectrogram> {
    let mut f = std::fs::File::open(path(dir, track)).ok()?;
    let mut head = [0u8; 20];
    f.read_exact(&mut head).ok()?;
    if &head[..8] != MAGIC || u32::from_le_bytes(head[8..12].try_into().ok()?) != BANDS as u32 {
        return None;
    }
    // The header's length against the file's before anything is allocated for it.
    let frames = u64::from_le_bytes(head[12..20].try_into().ok()?);
    let len = frames.checked_mul(BANDS as u64)?;
    if f.metadata().ok()?.len() != 20 + len {
        return None;
    }
    let mut data = Vec::with_capacity(len as usize);
    f.read_to_end(&mut data).ok()?;
    (data.len() as u64 == len).then_some(Spectrogram { data })
}

/// Writes the header, then the bytes as they are (a 30 h track's are 700 MB: never copied).
pub fn save(dir: &Path, track: usize, s: &Spectrogram) -> Result<()> {
    output::write_replacing_with(&path(dir, track), |f| {
        f.write_all(MAGIC)?;
        f.write_all(&(BANDS as u32).to_le_bytes())?;
        f.write_all(&(s.frames() as u64).to_le_bytes())?;
        f.write_all(&s.data)
    })?;
    Ok(())
}

/// The spectrogram of `file`'s track `track`, decoded from the start to its end.
pub fn build(file: &Path, track: usize) -> Result<Spectrogram> {
    let mut r = PcmReader::open(file, track, ANALYSIS_RATE, 1, Span::default())?;
    let mut m = Mel::new();
    let mut buf = Vec::new();
    loop {
        r.read_into(&mut buf, READ)?;
        if buf.is_empty() {
            break;
        }
        m.push(&buf);
    }
    r.finish()?;
    let s = m.finish();
    if s.frames() == 0 {
        return Err(Error::Like(format!(
            "audio track {} has no sound to read",
            track + 1
        )));
    }
    Ok(s)
}

/// The track's spectrogram from the project folder, or built and kept there when there is none, or
/// the one there stops more than 2 s short of `duration` (a recording still being written when it
/// was read). The bool says whether it was built now.
pub fn of_track(
    dir: &Path,
    file: &Path,
    track: usize,
    duration: f64,
) -> Result<(Spectrogram, bool)> {
    if let Some(s) = load(dir, track).filter(|s| s.seconds() + 2.0 >= duration) {
        return Ok((s, false));
    }
    let s = build(file, track)?;
    save(dir, track, &s)?;
    Ok((s, true))
}
