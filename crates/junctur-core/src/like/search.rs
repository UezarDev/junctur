//! The matcher behind "find all like this" (7b), pure over a [`Spectrogram`]: each example's patch
//! slid over the track, scored by normalised cross-correlation, and the peaks of that score.
//!
//! Each band's mean over the patch (and over the window it is compared with) is taken out, and the
//! rest scaled to unit length as a whole, so a louder or quieter copy of the sound and a steady
//! background tone under it score the same: only how the sound moves across the bands counts.
//! Rejected: normalising each band's variance on its own, which makes a quiet band of steady noise
//! count as much as the sound itself.

use std::collections::BTreeSet;

use super::mel::{Spectrogram, BANDS};

/// A time that matches a "not this" better than any example loses this many times the difference.
/// Rejected: the plan's "less half its best not-this match" everywhere, which lowered the real
/// matches as much as the lookalike whenever the two resembled each other (in the tests, the real
/// chirps fell from 0.8 to about 0.5 as the lookalike was taken out).
pub const NEG_WEIGHT: f32 = 2.0;
/// Frames scored per piece of work: about 3 minutes, small enough to share among the cores.
const CHUNK: usize = 16_384;

/// An example's frames, each band's mean taken out and scaled to unit length, frame-major.
#[derive(Debug, Clone)]
pub struct Patch {
    values: Vec<f32>,
    /// Frames.
    pub len: usize,
}

impl Patch {
    /// The patch of `spec` from frame `start`, `len` frames; `None` when it does not fit in the
    /// spectrogram or nothing in it moves (digital silence, one steady tone).
    pub fn of(spec: &Spectrogram, start: usize, len: usize) -> Option<Patch> {
        if len == 0 || start + len > spec.frames() {
            return None;
        }
        let rows = &spec.data()[start * BANDS..(start + len) * BANDS];
        let mut mean = [0f32; BANDS];
        for row in rows.chunks_exact(BANDS) {
            for (m, &v) in mean.iter_mut().zip(row) {
                *m += v as f32 / len as f32;
            }
        }
        let mut values: Vec<f32> = rows
            .chunks_exact(BANDS)
            .flat_map(|row| row.iter().zip(&mean).map(|(&v, m)| v as f32 - m))
            .collect();
        let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        // Under a fifth of a byte step per value: nothing moves.
        if norm < 0.2 * (values.len() as f32).sqrt() {
            return None;
        }
        values.iter_mut().for_each(|v| *v /= norm);
        Some(Patch { values, len })
    }
}

/// Per frame of a range: the score, and which positive example matched best there.
#[derive(Debug, Clone, Default)]
pub struct Scores {
    pub score: Vec<f32>,
    pub best: Vec<u16>,
}

/// The score of every frame `t` in `from..to` (index 0 is frame `from`): its best match over
/// `pos`, less [`NEG_WEIGHT`] times what its best "not this" match near it beats that by; -1 where
/// no positive fits before `to`. A patch is compared only with windows inside `from..to`. "Near":
/// a negative's match is spread over half its length either way, since a sound's windows a few
/// frames off its own still match the examples well but not the exact "not this" window the user
/// rejected. Shared out among the cores in [`CHUNK`]s.
pub fn scores(spec: &Spectrogram, from: usize, to: usize, pos: &[Patch], neg: &[Patch]) -> Scores {
    let to = to.min(spec.frames());
    let n = to.saturating_sub(from);
    let mut out = Scores {
        score: vec![-1.0; n],
        best: vec![0; n],
    };
    if n == 0 || pos.is_empty() {
        return out;
    }
    // One row per length of negative (a ✗ is a match, as long as its example, so there are few):
    // each is spread over its own reach before they are compared.
    let mut lens: Vec<usize> = neg.iter().map(|p| p.len).collect();
    lens.sort_unstable();
    lens.dedup();
    let group: Vec<usize> = neg
        .iter()
        .map(|p| lens.partition_point(|&l| l < p.len))
        .collect();
    let mut worst: Vec<Vec<f32>> = lens.iter().map(|_| vec![f32::NEG_INFINITY; n]).collect();
    let mut worst_chunks: Vec<_> = worst.iter_mut().map(|w| w.chunks_mut(CHUNK)).collect();
    let work: Vec<Work> = out
        .score
        .chunks_mut(CHUNK)
        .zip(out.best.chunks_mut(CHUNK))
        .enumerate()
        .map(|(i, (score, best))| Work {
            start: from + i * CHUNK,
            score,
            best,
            worst: worst_chunks.iter_mut().filter_map(|c| c.next()).collect(),
        })
        .collect();
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(work.len());
    let mut shares: Vec<Vec<_>> = (0..threads).map(|_| Vec::new()).collect();
    for (i, w) in work.into_iter().enumerate() {
        shares[i % threads].push(w);
    }
    let group = group.as_slice();
    std::thread::scope(|s| {
        for share in shares {
            s.spawn(move || {
                for w in share {
                    chunk(spec, to, pos, neg, group, w);
                }
            });
        }
    });
    let mut near = vec![f32::NEG_INFINITY; if neg.is_empty() { 0 } else { n }];
    for (row, &len) in worst.iter().zip(&lens) {
        for (m, v) in near.iter_mut().zip(spread(row, len / 2)) {
            *m = m.max(v);
        }
    }
    for (s, w) in out.score.iter_mut().zip(&near) {
        if *s > -1.0 && *w > *s {
            *s -= NEG_WEIGHT * (*w - *s);
        }
    }
    out
}

/// Each value the largest of `x` within `reach` either side (a sliding max, kept in a deque).
fn spread(x: &[f32], reach: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(x.len());
    let mut q: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut next = 0;
    for i in 0..x.len() {
        while next < x.len() && next <= i + reach {
            while q.back().is_some_and(|&j| x[j] <= x[next]) {
                q.pop_back();
            }
            q.push_back(next);
            next += 1;
        }
        while q.front().is_some_and(|&j| j + reach < i) {
            q.pop_front();
        }
        out.push(q.front().map_or(f32::NEG_INFINITY, |&j| x[j]));
    }
    out
}

/// One piece of [`scores`]: the frames from `start`, as many as `score` holds.
struct Work<'a> {
    start: usize,
    score: &'a mut [f32],
    best: &'a mut [u16],
    /// Per length of negative, its piece of that length's row.
    worst: Vec<&'a mut [f32]>,
}

/// [`scores`] for one piece, windows ending by `to`: the best positive into its `score` and
/// `best`, each negative into the row of `worst` for its length (`group`).
fn chunk(spec: &Spectrogram, to: usize, pos: &[Patch], neg: &[Patch], group: &[usize], w: Work) {
    let Work {
        start,
        score,
        best,
        mut worst,
    } = w;
    let longest = pos.iter().chain(neg).map(|p| p.len).max().unwrap_or(0);
    let end = (start + score.len() + longest).min(to);
    // The bytes as floats once for every patch, centred so the dot products stay small.
    let bytes = &spec.data()[start * BANDS..end * BANDS];
    let rows: Vec<f32> = bytes.iter().map(|&b| b as f32 - 128.0).collect();
    let mut one = vec![0f32; score.len()];
    for (k, p) in pos.iter().enumerate() {
        ncc(bytes, &rows, p, &mut one);
        for t in 0..score.len() {
            if one[t] > score[t] {
                score[t] = one[t];
                best[t] = k as u16;
            }
        }
    }
    for (p, &g) in neg.iter().zip(group) {
        ncc(bytes, &rows, p, &mut one);
        for (w, &v) in worst[g].iter_mut().zip(&one) {
            *w = w.max(v);
        }
    }
}

/// `p`'s normalised cross-correlation with the window starting at each frame of `bytes` (also as
/// `rows`), into `out`; -1 where the window would run past `bytes`. The window's per-band sums are
/// integers over the bytes, slid a frame at a time, so they are exact however long the run.
fn ncc(bytes: &[u8], rows: &[f32], p: &Patch, out: &mut [f32]) {
    let frames = bytes.len() / BANDS;
    let l = p.len;
    out.fill(-1.0);
    if frames < l {
        return;
    }
    let mut s1 = [0u64; BANDS];
    let mut s2 = [0u64; BANDS];
    let add = |s1: &mut [u64; BANDS], s2: &mut [u64; BANDS], f: usize, sign: bool| {
        for (b, &v) in bytes[f * BANDS..(f + 1) * BANDS].iter().enumerate() {
            let (v, v2) = (v as u64, (v as u64) * (v as u64));
            if sign {
                s1[b] += v;
                s2[b] += v2;
            } else {
                s1[b] -= v;
                s2[b] -= v2;
            }
        }
    };
    for f in 0..l {
        add(&mut s1, &mut s2, f, true);
    }
    let width = l * BANDS;
    let last = (frames - l + 1).min(out.len());
    for (t, o) in out.iter_mut().enumerate().take(last) {
        let var: f64 = s1
            .iter()
            .zip(&s2)
            .map(|(&a, &b)| b as f64 - (a as f64) * (a as f64) / l as f64)
            .sum();
        // A window where nothing moves matches nothing.
        if var > 0.04 * width as f64 {
            *o = dot(&p.values, &rows[t * BANDS..t * BANDS + width]) / var.sqrt() as f32;
        } else {
            *o = 0.0;
        }
        if t + l < frames {
            add(&mut s1, &mut s2, t, false);
            add(&mut s1, &mut s2, t + l, true);
        }
    }
}

/// A dot product the compiler vectorises: eight running sums.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail: f32 = ca
        .remainder()
        .iter()
        .zip(cb.remainder())
        .map(|(x, y)| x * y)
        .sum();
    for (x, y) in ca.zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    acc.iter().sum::<f32>() + tail
}

/// The peaks of `score` at or above `floor`: local maxima, the highest first, none within `gap`
/// frames of a higher one, at most `max` of them, in time order. Greedy by score, so a peak is only
/// ever left out for a higher one: the peaks at any higher threshold are these, filtered.
pub fn peaks(score: &[f32], floor: f32, gap: usize, max: usize) -> Vec<(usize, f32)> {
    let n = score.len();
    let mut cand: Vec<(usize, f32)> = (0..n)
        .filter(|&i| {
            let s = score[i];
            s >= floor && (i == 0 || s >= score[i - 1]) && (i + 1 == n || s > score[i + 1])
        })
        .map(|i| (i, score[i]))
        .collect();
    cand.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut taken = BTreeSet::new();
    let mut out = Vec::new();
    for (i, s) in cand {
        if out.len() >= max {
            break;
        }
        let near = taken
            .range(i.saturating_sub(gap.saturating_sub(1))..i + gap)
            .next();
        if near.is_none() {
            taken.insert(i);
            out.push((i, s));
        }
    }
    out.sort_by_key(|p| p.0);
    out
}

#[cfg(test)]
pub(super) fn spread_for_tests(x: &[f32], reach: usize) -> Vec<f32> {
    spread(x, reach)
}
