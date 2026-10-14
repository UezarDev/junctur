//! Voice isolation for mix-only recordings: split a mix track into a vocal stem and the rest, then
//! find speech and speakers on it. Everything streams, and a finished run in the cache is reused,
//! since separating a long stream is expensive.

mod mdx;
mod render;
mod stft;

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::diarize::{self, Diarization, Embedder};
use crate::ffmpeg::{PcmReader, PcmWriter, Span};
use crate::runtime::{Device, DevicePref};
use crate::vad::{self, Region, Vad};
use crate::{cache, models, Result};

pub use stft::Stft;

#[derive(Debug, Clone, Default)]
pub struct SeparateOptions {
    pub cache_dir: Option<PathBuf>,
    pub device: DevicePref,
    pub span: Span,
    /// Ignore a finished earlier run and separate again.
    pub force: bool,
    /// Skip speaker diarization.
    pub no_speakers: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    pub separate_s: f64,
    pub vad_s: f64,
    pub diarize_s: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stems {
    pub file: PathBuf,
    /// 0-based audio ordinal of the separated track.
    pub track: usize,
    pub start: f64,
    /// Seconds of audio separated.
    pub seconds: f64,
    pub vocals: PathBuf,
    pub background: PathBuf,
    pub device: Device,
    pub speech: Vec<Region>,
    pub speech_share: f64,
    pub speakers: Option<Diarization>,
    pub timings: Timings,
    #[serde(skip)]
    pub from_cache: bool,
}

/// Bump when stems or their analysis change shape.
const STEMS_VERSION: u32 = 1;

pub fn separate(file: &Path, track: usize, opts: &SeparateOptions) -> Result<Stems> {
    let cache_dir = opts.cache_dir.clone().unwrap_or_else(cache::default_dir);
    let settings = format!(
        "stems{STEMS_VERSION}|t{track}|{}|{:?}",
        opts.span.start, opts.span.len
    );
    let dir = cache_dir.join("stems").join(cache::key(file, &settings)?);
    // The speakers' file is named for the embeddings they were found from (an older `stems.json`
    // holds ones made on the GPU), so new ones are found while the separated stems are reused.
    let done = dir.join(if opts.no_speakers {
        "stems-no-speakers.json".to_string()
    } else {
        format!("stems-{}-embeddings.json", diarize::EMBEDDINGS_MARK)
    });
    if !opts.force {
        if let Some(s) = finished(&done) {
            return Ok(s);
        }
    }
    std::fs::create_dir_all(&dir)?;
    let (vocals, background) = (dir.join("vocals.flac"), dir.join("background.flac"));

    let Separated {
        seconds,
        device,
        separate_s,
    } = stems_for(file, track, opts, &cache_dir, &dir)?;

    let t1 = Instant::now();
    let speech = speech_regions(&vocals, &cache_dir)?;
    let vad_s = t1.elapsed().as_secs_f64();

    let t2 = Instant::now();
    let speakers = if opts.no_speakers {
        None
    } else {
        Some(speakers(&vocals, &speech, &cache_dir)?)
    };
    let diarize_s = t2.elapsed().as_secs_f64();

    let stems = Stems {
        file: file.to_path_buf(),
        track,
        start: opts.span.start,
        seconds,
        vocals,
        background,
        device,
        speech_share: vad::share(&speech, seconds),
        speech,
        speakers,
        timings: Timings {
            separate_s,
            vad_s,
            diarize_s,
        },
        from_cache: false,
    };
    std::fs::write(&done, serde_json::to_vec_pretty(&stems)?)?;
    Ok(stems)
}

/// A finished earlier run whose stems are still there.
fn finished(done: &Path) -> Option<Stems> {
    let mut s: Stems = serde_json::from_slice(&std::fs::read(done).ok()?).ok()?;
    if !(s.vocals.is_file() && s.background.is_file()) {
        return None;
    }
    s.from_cache = true;
    Some(s)
}

/// Who speaks when, over the vocal stem's speech regions.
fn speakers(vocals: &Path, speech: &[Region], cache_dir: &Path) -> Result<Diarization> {
    let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
    let mut emb = Embedder::open(&model)?;
    let mut r = PcmReader::open(vocals, 0, vad::RATE, 1, Span::default())?;
    diarize::diarize(&mut r, speech, &mut emb)
}

/// What the model pass produced; saved on its own so a changed analysis does not re-separate.
#[derive(Serialize, Deserialize)]
struct Separated {
    seconds: f64,
    device: Device,
    separate_s: f64,
}

fn stems_for(
    file: &Path,
    track: usize,
    opts: &SeparateOptions,
    cache_dir: &Path,
    dir: &Path,
) -> Result<Separated> {
    let (vocals, background) = (dir.join("vocals.flac"), dir.join("background.flac"));
    let marker = dir.join("separated.json");
    if !opts.force && vocals.is_file() && background.is_file() {
        if let Ok(s) = serde_json::from_slice(&std::fs::read(&marker).unwrap_or_default()) {
            return Ok(s);
        }
    }
    let t0 = Instant::now();
    let model = models::ensure(&models::MDX_VOCALS, cache_dir)?;
    let mut mdx = mdx::Mdx::open(&model, opts.device)?;
    let seconds = run_mdx(file, track, opts.span, &mut mdx, &vocals, &background)?;
    let s = Separated {
        seconds,
        device: mdx.device,
        separate_s: t0.elapsed().as_secs_f64(),
    };
    std::fs::write(&marker, serde_json::to_vec(&s)?)?;
    Ok(s)
}

/// Stream the track through the model in overlapping chunks; returns the seconds written.
fn run_mdx(
    file: &Path,
    track: usize,
    span: Span,
    mdx: &mut mdx::Mdx,
    vocals: &Path,
    background: &Path,
) -> Result<f64> {
    let mut reader = PcmReader::open(file, track, mdx::RATE, 2, span)?;
    let mut voc_out = PcmWriter::create(vocals, mdx::RATE, 2)?;
    let mut bg_out = PcmWriter::create(background, mdx::RATE, 2)?;
    // Chunk i covers padded samples [i*STEP, i*STEP + CHUNK) and keeps its middle STEP samples,
    // which are input samples [i*STEP, (i+1)*STEP): the leading TRIM zeros line them up.
    let mut l = vec![0.0f32; mdx::TRIM];
    let mut r = vec![0.0f32; mdx::TRIM];
    let (mut total, mut written, mut eof) = (0usize, 0usize, false);
    loop {
        while l.len() < mdx::CHUNK && !eof {
            let x = reader.read((mdx::CHUNK - l.len()) * 2)?;
            eof = x.is_empty();
            total += x.len() / 2;
            for f in x.chunks_exact(2) {
                l.push(f[0]);
                r.push(f[1]);
            }
        }
        if written >= total && eof {
            break;
        }
        l.resize(mdx::CHUNK, 0.0);
        r.resize(mdx::CHUNK, 0.0);
        let [vl, vr] = mdx.vocals(&l, &r)?;
        let keep = mdx::STEP.min(total - written);
        let mut voc = Vec::with_capacity(keep * 2);
        let mut bg = Vec::with_capacity(keep * 2);
        for i in mdx::TRIM..mdx::TRIM + keep {
            voc.extend([vl[i], vr[i]]);
            bg.extend([l[i] - vl[i], r[i] - vr[i]]);
        }
        voc_out.write(&voc)?;
        bg_out.write(&bg)?;
        written += keep;
        l.drain(..mdx::STEP);
        r.drain(..mdx::STEP);
    }
    reader.finish()?;
    voc_out.finish()?;
    bg_out.finish()?;
    Ok(total as f64 / mdx::RATE as f64)
}

/// Speech regions of a whole stem, streamed through the VAD a minute at a time.
pub fn speech_regions(audio: &Path, cache_dir: &Path) -> Result<Vec<Region>> {
    let mut vad = Vad::open(&models::ensure(&models::SILERO_VAD, cache_dir)?)?;
    let mut reader = PcmReader::open(audio, 0, vad::RATE, 1, Span::default())?;
    let mut probs = Vec::new();
    loop {
        let x = reader.read(vad::MINUTE)?;
        if x.is_empty() {
            break;
        }
        probs.extend(vad.probs(&x)?);
    }
    reader.finish()?;
    Ok(vad::regions(&probs, 0.0))
}
