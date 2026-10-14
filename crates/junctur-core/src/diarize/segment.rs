//! Where two people talk at once, which CAM++ windows cannot see: pyannote's segmentation-3.0 model
//! over 10 s windows. Its "local" speakers are named later from the labelled turns they coincide
//! with, so this only finds the stretches and keeps that evidence, and relabelling never reruns it.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

use crate::runtime;
use crate::vad::{Region, RATE};
use crate::Result;

/// The model's input: 10 s of 16 kHz mono.
pub const WINDOW_S: f64 = 10.0;
/// Windows start this far apart, so every instant is in the middle half of exactly one window, the
/// one that decides it: the model is least sure near a window's edges.
pub const STEP_S: f64 = 5.0;
/// The model's frame step and receptive field, in samples (its export metadata).
const FRAME_STEP: usize = 270;
const FRAME_FIELD: usize = 991;
/// Windows per model call.
pub const BATCH: usize = 16;
/// A pair class must hold this long to count: shorter is a crossfade at a turn change.
const MIN_OVERLAP_S: f64 = 0.2;
/// A frame is two people at once when the three pair classes together have at least this
/// probability (the most likely pair is taken). Lower than plain argmax (about 0.5), which misses
/// much overlap; 0.2 keeps the same precision, 0.1 loses some.
const PAIR_P: f32 = 0.2;
/// Gaps shorter than this inside one pair's stretch are bridged (a syllable's dip).
const BRIDGE_S: f64 = 0.3;
/// The powerset classes: 0 none, 1-3 one local speaker, then the pairs 1+2, 1+3, 2+3.
const CLASSES: usize = 7;
const FIRST_PAIR: u8 = 4;
/// A speaker's solo runs bridge gaps under this many frames.
const SOLO_BRIDGE_FRAMES: f64 = 1.5;

/// A stretch where two local speakers of one window talk at once, with where each of them talks
/// alone in that window (source seconds), the evidence for who they are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overlap {
    pub start: f64,
    pub end: f64,
    pub a: Vec<[f64; 2]>,
    pub b: Vec<[f64; 2]>,
}

pub struct Segmenter {
    session: Session,
}

impl Segmenter {
    /// On the CPU, as everything in the Tracks, Speech and Speakers stages is: only transcription
    /// uses the GPU.
    pub fn open(model: &Path) -> Result<Self> {
        Ok(Self {
            session: runtime::open_on_cpu(model, None)?,
        })
    }

    /// The most likely powerset class of every frame of each 10 s clip (0 none, 1-3 one local
    /// speaker, 4-6 the pairs 1+2, 1+3, 2+3).
    pub fn classes(&mut self, clips: &[Vec<f32>]) -> Result<Vec<Vec<u8>>> {
        if clips.is_empty() {
            return Ok(Vec::new());
        }
        let len = window_samples();
        let mut flat = Vec::with_capacity(clips.len() * len);
        for c in clips {
            flat.extend(c.iter().copied().take(len));
            flat.resize(flat.len() + len.saturating_sub(c.len()), 0.0);
        }
        let x = Tensor::from_array(([clips.len(), 1, len], flat))?;
        let res = self.session.run(ort::inputs!["x" => x])?;
        let (shape, y) = res["y"].try_extract_tensor::<f32>()?;
        let classes = *shape.last().unwrap_or(&(CLASSES as i64)) as usize;
        let frames = y.len() / clips.len() / classes.max(1);
        Ok(y.chunks_exact(frames * classes)
            .map(|w| {
                w.chunks_exact(classes)
                    .map(|f| {
                        let best = |range: std::ops::Range<usize>| {
                            range
                                .max_by(|&a, &b| f[a].total_cmp(&f[b]))
                                .map_or(0, |i| i as u8)
                        };
                        // The output is log-probabilities: a frame is two people at once when
                        // the pairs together are likely enough, even if one single class wins.
                        let first_pair = FIRST_PAIR as usize;
                        let pairs: f32 = f.iter().skip(first_pair).map(|x| x.exp()).sum();
                        // Under it, nobody or one person: never a pair that won narrowly.
                        if f.len() == CLASSES && pairs >= PAIR_P {
                            best(first_pair..CLASSES)
                        } else {
                            best(0..f.len().min(first_pair))
                        }
                    })
                    .collect()
            })
            .collect())
    }
}

/// Every setting that shapes the overlap stretches, for the hearing key.
pub fn settings() -> String {
    format!("{WINDOW_S} {STEP_S} {PAIR_P} {MIN_OVERLAP_S} {BRIDGE_S}")
}

pub fn window_samples() -> usize {
    (WINDOW_S * RATE as f64) as usize
}

/// The span of source time window `k` (starting at `k * STEP_S`) decides: its middle half, and for
/// the first window everything before that too.
fn decides(k: usize) -> (f64, f64) {
    let s = k as f64 * STEP_S;
    let lo = if k == 0 { 0.0 } else { s + STEP_S / 2.0 };
    (lo, s + STEP_S / 2.0 + STEP_S)
}

/// The windows (by index `k`, starting at `k * STEP_S`) worth running: those whose deciding span
/// holds some speech. Sorted.
pub fn windows(regions: &[Region]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for r in regions {
        let first = ((r.start - STEP_S / 2.0) / STEP_S).floor().max(0.0) as usize;
        let last = ((r.end - STEP_S / 2.0) / STEP_S).floor().max(0.0) as usize;
        for k in first..=last {
            let (lo, hi) = decides(k);
            if r.start < hi && r.end > lo && out.last().is_none_or(|&l| l < k) {
                out.push(k);
            }
        }
    }
    out
}

/// Centre of frame `f` of a window, in seconds from the window's start.
fn frame_time(f: usize) -> f64 {
    (f * FRAME_STEP) as f64 / RATE as f64 + FRAME_FIELD as f64 / 2.0 / RATE as f64
}

fn frame_len() -> f64 {
    FRAME_STEP as f64 / RATE as f64
}

/// Which two local speakers (0-2) a pair class is.
fn pair(class: u8) -> Option<(u8, u8)> {
    match class {
        4 => Some((0, 1)),
        5 => Some((0, 2)),
        6 => Some((1, 2)),
        _ => None,
    }
}

/// Source spans, `[start, end]` seconds.
type Spans = Vec<[f64; 2]>;

/// Runs of frames matching `is`, as source spans, bridging gaps under `bridge` seconds.
fn runs(start: f64, classes: &[u8], bridge: f64, is: impl Fn(u8) -> bool) -> Spans {
    let mut out: Vec<[f64; 2]> = Vec::new();
    for (f, &c) in classes.iter().enumerate() {
        if !is(c) {
            continue;
        }
        let t = start + frame_time(f) - frame_len() / 2.0;
        match out.last_mut() {
            Some(r) if t - r[1] < bridge => r[1] = t + frame_len(),
            _ => out.push([t, t + frame_len()]),
        }
    }
    out
}

/// The overlap stretches window `k` decides, from its frames' classes.
pub fn overlaps(k: usize, classes: &[u8]) -> Vec<Overlap> {
    let start = k as f64 * STEP_S;
    let (lo, hi) = decides(k);
    let mut out = Vec::new();
    for class in FIRST_PAIR..CLASSES as u8 {
        let (a, b) = pair(class).expect("a pair class");
        let alone = |who: u8| {
            runs(start, classes, frame_len() * SOLO_BRIDGE_FRAMES, |c| {
                c == who + 1
            })
        };
        let mut solo: Option<(Spans, Spans)> = None;
        for [s, e] in runs(start, classes, BRIDGE_S, |c| c == class) {
            // Only the part this window decides; another window decides the rest.
            let (s, e) = (s.max(lo), e.min(hi));
            if e - s < MIN_OVERLAP_S {
                continue;
            }
            let (sa, sb) = solo.get_or_insert_with(|| (alone(a), alone(b))).clone();
            out.push(Overlap {
                start: s,
                end: e,
                a: sa,
                b: sb,
            });
        }
    }
    out.sort_by(|x, y| x.start.total_cmp(&y.start));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Region {
        Region { start, end }
    }

    #[test]
    fn windows_cover_the_speech_once() {
        // Speech 3-4 s: decided by window 0 (0-7.5). 12-13: window 1 (7.5-12.5) and 2 (12.5-17.5).
        assert_eq!(windows(&[r(3.0, 4.0)]), vec![0]);
        assert_eq!(windows(&[r(3.0, 4.0), r(12.0, 13.0)]), vec![0, 1, 2]);
        assert_eq!(windows(&[r(30.0, 31.0), r(31.5, 32.0)]), vec![5]);
        assert!(windows(&[]).is_empty());
        assert_eq!(decides(0), (0.0, 7.5));
        assert_eq!(decides(3), (17.5, 22.5));
    }

    #[test]
    fn a_pair_class_long_enough_is_an_overlap_with_each_ones_solo_time() {
        let n = 589;
        let at =
            |t: f64| ((t - FRAME_FIELD as f64 / 2.0 / RATE as f64) / frame_len()).round() as usize;
        let mut c = vec![0u8; n];
        // Window 1 (5-15 s): local 1 alone 6-7, local 2 alone 7-8, both 8-9 (inside 7.5-12.5).
        c[at(1.0)..at(2.0)].fill(1);
        c[at(2.0)..at(3.0)].fill(2);
        c[at(3.0)..at(4.0)].fill(4);
        // A blip of pair 2+3 too short to count.
        c[at(5.0)..at(5.1)].fill(6);
        let o = overlaps(1, &c);
        assert_eq!(o.len(), 1, "{o:?}");
        assert!(
            (o[0].start - 8.0).abs() < 0.03 && (o[0].end - 9.0).abs() < 0.03,
            "{o:?}"
        );
        assert_eq!(o[0].a.len(), 1);
        assert!((o[0].a[0][0] - 6.0).abs() < 0.03 && (o[0].b[0][1] - 8.0).abs() < 0.03);
        // The same frames in window 2 (10-20 s, deciding 12.5-17.5): its 13-14 s overlap counts,
        // nothing outside the middle half does.
        let o2 = overlaps(2, &c);
        assert_eq!(o2.len(), 1);
        assert!((o2[0].start - 13.0).abs() < 0.03);
        let mut edge = vec![0u8; n];
        edge[at(0.5)..at(2.0)].fill(5); // 10.5-12 s of window 2: not its to decide
        assert!(overlaps(2, &edge).is_empty());
    }

    #[test]
    fn short_gaps_inside_an_overlap_are_bridged() {
        let at =
            |t: f64| ((t - FRAME_FIELD as f64 / 2.0 / RATE as f64) / frame_len()).round() as usize;
        let mut c = vec![0u8; 589];
        c[at(3.0)..at(3.5)].fill(4);
        c[at(3.5)..at(3.7)].fill(1);
        c[at(3.7)..at(4.2)].fill(4);
        let o = overlaps(0, &c);
        assert_eq!(o.len(), 1, "{o:?}");
        assert!((o[0].end - o[0].start - 1.2).abs() < 0.05);
    }
}
