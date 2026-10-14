//! Speech regions on every track that carries speech, read as-is (no voice isolation: stems cost
//! ~19 GB per 30h stream, and Silero holds up on the raw mix), plus the energy envelope the cut
//! snapping uses. One decode pass per track feeds both.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::edit::{Envelope, ENVELOPE_HOP};
use crate::ffmpeg::{PcmReader, Span};
use crate::vad::{self, Region, Vad};
use crate::{models, Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechTrack {
    /// 0-based audio ordinal.
    pub track: usize,
    pub regions: Vec<Region>,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Speech {
    pub tracks: Vec<SpeechTrack>,
    pub seconds: f64,
}

impl Speech {
    pub fn all_regions(&self) -> Vec<Region> {
        self.tracks.iter().flat_map(|t| t.regions.clone()).collect()
    }
}

/// 60 s = 1875 whole VAD frames, so frames never straddle reads.
const READ: usize = vad::RATE as usize * 60;
const HOP: usize = (vad::RATE as f64 * ENVELOPE_HOP) as usize;

/// Speech regions on each of `tracks` and the loudest-track envelope. Each track runs on a thread
/// of its own (its own decode and Silero session), and the first to fail stops the others at their
/// next read. `read` is told each track's seconds decoded so far, from that track's thread.
pub fn detect(
    file: &Path,
    tracks: &[usize],
    cache_dir: &Path,
    read: &(dyn Fn(usize, f64) + Sync),
) -> Result<(Speech, Envelope)> {
    let model = models::ensure(&models::SILERO_VAD, cache_dir)?;
    let failed = AtomicBool::new(false);
    let results: Vec<Result<Option<Track>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = tracks
            .iter()
            .map(|&track| {
                let (model, failed) = (&model, &failed);
                scope.spawn(move || {
                    let r = one_track(file, track, model, failed, read);
                    if r.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    r
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(Error::Model("a speech thread panicked".into())))
            })
            .collect()
    });
    let mut env: Vec<f32> = Vec::new();
    let mut out = Vec::new();
    let mut seconds: f64 = 0.0;
    // A failure is reported, not the tracks it stopped (those come back as None).
    if let Some(e) = results.iter().position(Result::is_err) {
        return Err(results
            .into_iter()
            .nth(e)
            .and_then(Result::err)
            .expect("an error"));
    }
    for t in results.into_iter().flatten().flatten() {
        // The loudest speech track at each instant: a cut must be quiet on all of them.
        if env.len() < t.env.len() {
            env.resize(t.env.len(), 0.0);
        }
        for (e, &x) in env.iter_mut().zip(&t.env) {
            *e = e.max(x);
        }
        seconds = seconds.max(t.seconds);
        out.push(t.speech);
    }
    Ok((
        Speech {
            tracks: out,
            seconds,
        },
        Envelope(env),
    ))
}

/// What one track's pass found.
struct Track {
    speech: SpeechTrack,
    /// RMS per envelope hop.
    env: Vec<f32>,
    seconds: f64,
}

/// One decode of `track` feeds both the VAD and the envelope. None when `stop` was set (another
/// track failed) before it finished.
fn one_track(
    file: &Path,
    track: usize,
    model: &Path,
    stop: &AtomicBool,
    read: &(dyn Fn(usize, f64) + Sync),
) -> Result<Option<Track>> {
    let mut vad = Vad::open(model)?;
    let mut r = PcmReader::open(file, track, vad::RATE, 1, Span::default())?;
    let (mut probs, mut env, mut n) = (Vec::new(), Vec::new(), 0usize);
    let mut x = Vec::with_capacity(READ);
    read(track, 0.0);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        r.read_into(&mut x, READ)?;
        if x.is_empty() {
            break;
        }
        probs.extend(vad.probs(&x)?);
        // READ is a whole number of hops, so hops never straddle reads.
        env.extend(
            x.chunks(HOP)
                .map(|hop| (hop.iter().map(|s| s * s).sum::<f32>() / hop.len() as f32).sqrt()),
        );
        n += x.len();
        read(track, n as f64 / vad::RATE as f64);
    }
    r.finish()?;
    let len = n as f64 / vad::RATE as f64;
    let regions = vad::regions(&probs, 0.0);
    Ok(Some(Track {
        speech: SpeechTrack {
            track,
            share: vad::share(&regions, len),
            regions,
        },
        env,
        seconds: len,
    }))
}

pub fn save_envelope(path: &Path, env: &Envelope) -> Result<()> {
    let bytes: Vec<u8> = env.0.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(path, bytes)?;
    Ok(())
}

pub fn load_envelope(path: &Path) -> Result<Envelope> {
    let b = std::fs::read(path)?;
    Ok(Envelope(
        b.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    ))
}
