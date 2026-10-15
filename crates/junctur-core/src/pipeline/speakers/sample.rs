//! A voice sample to remember: embeddings of the stretches the user picked, from the windows the
//! speakers stage kept or decoded afresh.

use std::path::Path;

use super::embeddings::{heard_embeddings, HEARD_BY_AN_OLDER_JUNCTUR};
use super::{Found, Method};
use crate::diarize::{self, Embedder};
use crate::ffmpeg::{PcmReader, Span};
use crate::vad::{Region, RATE};
use crate::{models, Result};

/// A stretch embedded straight from the audio ([`embed_stretches`]) is read at least this long,
/// centred on it, so CAM++ has frames to work on even for one short word.
const MIN_CLIP_S: f64 = 0.5;

/// A voice sample from what was heard: the cached embeddings of the windows whose middle is in one
/// of `stretches` (source seconds), and the seconds they cover, leaving out windows touching an
/// overlap. For a short pick, the windows reaching into it, weighted by the seconds covered: CAM++
/// on one word alone is too noisy. Empty when none: the caller then uses [`embed_stretches`].
pub fn voice_sample(
    found: &Found,
    dir: &Path,
    stretches: &[(f64, f64)],
) -> Result<(Vec<Vec<f32>>, f64)> {
    let not_heard = || {
        crate::Error::Edit(match found.method {
            Method::Tracks => {
                "the speakers here are their own tracks: there is no voice to tell apart".into()
            }
            Method::None => "no track of this recording carries speech".into(),
            Method::Voices => HEARD_BY_AN_OLDER_JUNCTUR.into(),
        })
    };
    let heard = found
        .heard
        .as_ref()
        .filter(|_| found.method == Method::Voices)
        .ok_or_else(not_heard)?;
    let e = heard_embeddings(heard, dir).ok_or_else(not_heard)?;
    let mut out = Vec::new();
    let mut seconds = 0.0;
    // Overlaps are sorted by start and none is longer than a segmentation window, so only those
    // starting within that much before a window can touch it.
    let longest = diarize::segment::WINDOW_S;
    let shared = |w: &Region| {
        let upto = heard.overlaps.partition_point(|o| o.start < w.end);
        heard.overlaps[..upto]
            .iter()
            .rev()
            .take_while(|o| o.start > w.start - longest)
            .any(|o| o.end > w.start)
    };
    for (w, emb) in heard.windows.iter().zip(&e) {
        let mid = (w.start + w.end) / 2.0;
        let chosen = stretches.iter().any(|&(a, b)| a <= mid && mid <= b);
        if chosen && !shared(w) {
            seconds += w.end - w.start;
            out.push(emb.clone());
        }
    }
    if out.is_empty() {
        for (w, emb) in heard.windows.iter().zip(&e) {
            let covered: f64 = stretches
                .iter()
                .map(|&(a, b)| (w.end.min(b) - w.start.max(a)).max(0.0))
                .sum();
            if covered > 0.0 && !shared(w) {
                seconds += covered;
                out.push(emb.clone());
            }
        }
    }
    Ok((out, seconds))
}

/// Embeddings of `stretches` of `track` read straight from `file`, and their seconds, on the CPU:
/// the voice sample only when no cached window covers the selection. Each stretch is read at least
/// [`MIN_CLIP_S`] long, centred on it.
pub fn embed_stretches(
    file: &Path,
    track: usize,
    stretches: &[(f64, f64)],
    cache_dir: &Path,
) -> Result<(Vec<Vec<f32>>, f64)> {
    let model = models::ensure(&models::SPEAKER_EMBEDDING, cache_dir)?;
    let mut emb = Embedder::open(&model)?;
    let mut out = Vec::new();
    let mut seconds = 0.0;
    for &(a, b) in stretches {
        let len = (b - a).max(MIN_CLIP_S);
        let start = ((a + b) / 2.0 - len / 2.0).max(0.0);
        let mut r = PcmReader::open(
            file,
            track,
            RATE,
            1,
            Span {
                start,
                len: Some(len),
            },
        )?;
        let mut samples = Vec::new();
        loop {
            let more = r.read(RATE as usize)?;
            if more.is_empty() {
                break;
            }
            samples.extend(more);
        }
        r.finish()?;
        if let Some(e) = emb.embed(&samples)? {
            out.push(e);
            seconds += b - a;
        }
    }
    Ok((out, seconds))
}
