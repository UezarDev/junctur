//! Per-window scores kept inside the edit file (a DECISIONS item): "uncertain" later means near
//! the cutoff, and learning adjusts these same numbers.

use super::Window;
use crate::vad::Region;

/// Score windows of `len` seconds over `[0, total]`: speech share (union over tracks, overlap
/// counted once) and the number of words that start inside.
pub fn windows(speech: &[Region], word_starts: &[f64], total: f64, len: f64) -> Vec<Window> {
    if total <= 0.0 || len <= 0.0 {
        return Vec::new();
    }
    let n = (total / len).ceil() as usize;
    let mut cover = vec![0.0f64; n];
    for (a, b) in union(speech) {
        let (mut i, last) = ((a / len) as usize, ((b / len) as usize).min(n - 1));
        while i <= last {
            let (ws, we) = (i as f64 * len, (i + 1) as f64 * len);
            cover[i] += (b.min(we) - a.max(ws)).max(0.0);
            i += 1;
        }
    }
    let mut words = vec![0u32; n];
    for &t in word_starts {
        if t >= 0.0 {
            words[((t / len) as usize).min(n - 1)] += 1;
        }
    }
    (0..n)
        .map(|i| {
            let start = i as f64 * len;
            let span = len.min(total - start);
            let speech = (cover[i] / span).clamp(0.0, 1.0);
            Window {
                start,
                score: speech,
                speech,
                words: words[i],
            }
        })
        .collect()
}

fn union(speech: &[Region]) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = speech.iter().map(|r| (r.start, r.end)).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_and_words() {
        let sp = [
            Region {
                start: 2.0,
                end: 7.0,
            },
            Region {
                start: 3.0,
                end: 4.0,
            },
        ];
        let w = windows(&sp, &[2.5, 6.0, 11.0], 12.0, 5.0);
        assert_eq!(w.len(), 3);
        assert!((w[0].speech - 0.6).abs() < 1e-9);
        assert!((w[1].speech - 0.4).abs() < 1e-9);
        assert_eq!(w[2].speech, 0.0);
        assert_eq!((w[0].words, w[1].words, w[2].words), (1, 1, 1));
        assert!(windows(&[], &[], 0.0, 5.0).is_empty());
    }
}
