//! Which stretches of audio Whisper sees: speech regions grouped into windows of at most ~30 s.
//!
//! Whisper decodes 30 s at a time, and feeding it silence or music is what makes it invent text, so
//! it only ever sees speech plus a little padding. Nearby regions share a window: one call per VAD
//! region would cost a full encoder pass for every half-second "sí".

use serde::{Deserialize, Serialize};

use crate::vad::Region;

/// Under Whisper's 30 s window, leaving room for the padding.
pub const MAX_LEN: f64 = 28.0;
/// Audio kept around speech, so first and last words are not clipped.
pub const PAD: f64 = 0.2;
/// Regions closer than this go in the same window.
pub const JOIN_GAP: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub start: f64,
    pub end: f64,
}

/// Group sorted speech regions into chunks within `[0, total]`. A region longer than [`MAX_LEN`]
/// is split into equal parts: a word on the seam may be cut, which beats Whisper dropping the tail.
pub fn plan(regions: &[Region], total: f64) -> Vec<Chunk> {
    let mut out: Vec<Chunk> = Vec::new();
    for r in regions {
        let (s, e) = ((r.start - PAD).max(0.0), (r.end + PAD).min(total));
        if e <= s {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if s - last.end <= JOIN_GAP && e - last.start <= MAX_LEN {
                last.end = last.end.max(e);
                continue;
            }
        }
        let parts = ((e - s) / MAX_LEN).ceil().max(1.0) as usize;
        let step = (e - s) / parts as f64;
        for i in 0..parts {
            out.push(Chunk {
                start: s + i as f64 * step,
                end: if i + 1 == parts {
                    e
                } else {
                    s + (i + 1) as f64 * step
                },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Region {
        Region { start, end }
    }

    #[test]
    fn empty_gives_nothing() {
        assert!(plan(&[], 100.0).is_empty());
    }

    #[test]
    fn close_regions_join_and_far_ones_do_not() {
        let c = plan(&[r(1.0, 2.0), r(2.5, 3.0), r(10.0, 11.0)], 100.0);
        assert_eq!(c.len(), 2);
        assert!((c[0].start - 0.8).abs() < 1e-9 && (c[0].end - 3.2).abs() < 1e-9);
        assert!((c[1].start - 9.8).abs() < 1e-9);
    }

    #[test]
    fn long_region_splits_and_padding_clamps() {
        let c = plan(&[r(0.0, 60.0)], 60.0);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].start, 0.0);
        assert_eq!(c[2].end, 60.0);
        assert!(c.iter().all(|x| x.end - x.start <= MAX_LEN));
    }

    #[test]
    fn join_stops_at_max_len() {
        let c = plan(&[r(0.0, 20.0), r(20.5, 30.0)], 100.0);
        assert_eq!(c.len(), 2);
    }
}
