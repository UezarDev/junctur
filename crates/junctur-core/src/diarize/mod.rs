//! Who speaks when, on a single voice track: speaker embeddings (CAM++) over short speech segments,
//! clustered into speakers. This is how mic and Discord are told apart when they share one track.

mod cluster;
mod fbank;

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

use crate::ffmpeg::PcmReader;
use crate::runtime::{self, Device, DevicePref};
use crate::vad::{Region, RATE};
use crate::Result;

pub use cluster::{two_pass, FIRST, MERGE};

/// Segment length for one embedding. Long enough for a stable voiceprint, short enough that one
/// segment rarely spans two speakers.
const SEGMENT_S: f64 = 1.5;
/// Speech shorter than this gives an unreliable embedding and is left unlabelled.
const MIN_SEGMENT_S: f64 = 0.5;
/// Speakers under this share of talk time are reported as minor (laughs, bleed, clustering noise).
const MINOR_SHARE: f64 = 0.03;
/// Embeddings per model call.
const BATCH: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: usize,
    pub seconds: f64,
    pub share: f64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diarization {
    pub speakers: Vec<Speaker>,
    pub turns: Vec<Turn>,
}

pub struct Embedder {
    session: Session,
    pub device: Device,
    fbank: fbank::Fbank,
}

impl Embedder {
    pub fn open(model: &Path, pref: DevicePref) -> Result<Self> {
        let (session, device) = runtime::open(model, pref)?;
        Ok(Self {
            session,
            device,
            fbank: fbank::Fbank::new(),
        })
    }

    /// Unit-length speaker embedding of 16 kHz mono audio; None when too short for one frame.
    pub fn embed(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>> {
        Ok(self.embed_batch(&[samples])?.pop())
    }

    /// Embeddings of equal-length clips in one model call; empty when they are too short.
    pub fn embed_batch(&mut self, clips: &[&[f32]]) -> Result<Vec<Vec<f32>>> {
        let feats: Vec<_> = clips.iter().map(|c| self.fbank.compute(c)).collect();
        let t = feats.iter().map(Vec::len).min().unwrap_or(0);
        if t == 0 {
            return Ok(Vec::new());
        }
        let flat: Vec<f32> = feats
            .iter()
            .flat_map(|f| f[..t].iter().flatten().copied())
            .collect();
        let x = Tensor::from_array(([clips.len(), t, fbank::BINS], flat))?;
        let res = self.session.run(ort::inputs!["x" => x])?;
        let out = res["embedding"].try_extract_tensor::<f32>()?.1;
        let dim = out.len() / clips.len();
        Ok(out
            .chunks_exact(dim)
            .map(|e| {
                let mut e = e.to_vec();
                cluster::normalize(&mut e);
                e
            })
            .collect())
    }
}

/// Fixed-length embedding windows over the speech regions. Every window is exactly [`SEGMENT_S`]
/// long (a short region gets one window centred on it), because on CUDA every new input shape
/// costs a fresh kernel search: variable lengths made diarization 10x slower than separation.
pub fn segments(regions: &[Region]) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    for r in regions {
        let len = r.end - r.start;
        if len < MIN_SEGMENT_S {
            continue;
        }
        let n = (len / SEGMENT_S).floor().max(1.0) as usize;
        let spare = len - n as f64 * SEGMENT_S;
        let first = (r.start + spare / 2.0).max(0.0);
        out.extend((0..n).map(|i| Region {
            start: first + i as f64 * SEGMENT_S,
            end: first + (i + 1) as f64 * SEGMENT_S,
        }));
    }
    out
}

/// Diarize a 16 kHz mono stream (which starts at time 0) over its speech `regions`, reading it once.
pub fn diarize(
    reader: &mut PcmReader,
    regions: &[Region],
    embedder: &mut Embedder,
) -> Result<Diarization> {
    let segs = segments(regions);
    let mut buf: Vec<f32> = Vec::new();
    let mut buf_start = 0usize;
    let mut clips: Vec<Vec<f32>> = Vec::new();
    let mut embs: Vec<Vec<f32>> = Vec::new();
    let at = |t: f64| (t * RATE as f64) as usize;
    let len = at(SEGMENT_S);
    for s in &segs {
        let a = at(s.start);
        // Drop what lies before this segment, then read up to its end.
        let drop = a.saturating_sub(buf_start).min(buf.len());
        buf.drain(..drop);
        buf_start += drop;
        while buf_start < a {
            let skip = reader.read((a - buf_start).min(RATE as usize * 60))?;
            if skip.is_empty() {
                break;
            }
            buf_start += skip.len();
        }
        while buf_start + buf.len() < a + len {
            let more = reader.read(a + len - buf_start - buf.len())?;
            if more.is_empty() {
                break;
            }
            buf.extend(more);
        }
        // Segments are sorted but may overlap the previous one, so slice from the buffer.
        let lo = a.saturating_sub(buf_start).min(buf.len());
        let mut clip = buf[lo..(lo + len).min(buf.len())].to_vec();
        clip.resize(len, 0.0);
        clips.push(clip);
        if clips.len() == BATCH {
            embs.extend(embed_all(embedder, &mut clips)?);
        }
    }
    embs.extend(embed_all(embedder, &mut clips)?);
    let labels = two_pass(&embs, FIRST, MERGE, MINOR_SHARE);
    Ok(summarize(&segs, &labels))
}

fn embed_all(embedder: &mut Embedder, clips: &mut Vec<Vec<f32>>) -> Result<Vec<Vec<f32>>> {
    if clips.is_empty() {
        return Ok(Vec::new());
    }
    let refs: Vec<&[f32]> = clips.iter().map(Vec::as_slice).collect();
    let out = embedder.embed_batch(&refs)?;
    clips.clear();
    Ok(out)
}

/// Turns (adjacent same-speaker segments joined) and per-speaker talk time, largest first.
pub fn summarize(segs: &[Region], labels: &[usize]) -> Diarization {
    let mut turns: Vec<Turn> = Vec::new();
    for (s, &l) in segs.iter().zip(labels) {
        match turns.last_mut() {
            Some(t) if t.speaker == l && s.start - t.end < 0.05 => t.end = s.end,
            _ => turns.push(Turn {
                start: s.start,
                end: s.end,
                speaker: l,
            }),
        }
    }
    let n = labels.iter().max().map_or(0, |m| m + 1);
    let mut secs = vec![0.0; n];
    for t in &turns {
        secs[t.speaker] += t.end - t.start;
    }
    let total: f64 = secs.iter().sum();
    // Defensive: a label with no talk time (none today; `two_pass` labels are dense) is not a
    // speaker.
    let mut order: Vec<usize> = (0..n).filter(|&l| secs[l] > 0.0).collect();
    order.sort_by(|&a, &b| secs[b].total_cmp(&secs[a]));
    // Renumber so speaker 0 talks most.
    let mut rank = vec![usize::MAX; n];
    for (r, &o) in order.iter().enumerate() {
        rank[o] = r;
    }
    turns.iter_mut().for_each(|t| t.speaker = rank[t.speaker]);
    let speakers = order
        .iter()
        .enumerate()
        .map(|(r, &o)| {
            let share = if total > 0.0 { secs[o] / total } else { 0.0 };
            let label = if r == 0 {
                "main voice: most talk time, likely the streamer's mic"
            } else if share < MINOR_SHARE {
                "minor: too little talk time to call a person"
            } else {
                "other voice: likely Discord or a guest"
            };
            Speaker {
                id: r,
                seconds: secs[o],
                share,
                label: label.to_string(),
            }
        })
        .collect();
    Diarization { speakers, turns }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Region {
        Region { start, end }
    }

    #[test]
    fn segments_split_long_and_skip_short() {
        let s = segments(&[r(0.0, 0.3), r(1.0, 4.2), r(10.0, 10.8)]);
        assert_eq!(s.len(), 3);
        assert!((s[0].start - 1.1).abs() < 1e-9 && (s[1].end - 4.1).abs() < 1e-9);
        // A short region gets one full-length window centred on it.
        assert!((s[2].start - 9.65).abs() < 1e-9 && (s[2].end - 11.15).abs() < 1e-9);
    }

    #[test]
    fn summary_ranks_by_talk_time_and_joins_turns() {
        let segs = [r(0.0, 1.0), r(1.0, 2.0), r(5.0, 10.0), r(20.0, 21.0)];
        let d = summarize(&segs, &[1, 1, 0, 1]);
        assert_eq!(d.speakers[0].seconds, 5.0);
        assert_eq!(d.turns.len(), 3);
        assert_eq!(d.turns[0].speaker, 1);
        assert_eq!(d.turns[1].speaker, 0);
        assert!(summarize(&[], &[]).speakers.is_empty());
    }
}
