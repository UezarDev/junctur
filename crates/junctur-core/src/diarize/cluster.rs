//! Grouping speaker embeddings into speakers: tight clusters at [`FIRST`], then their centroids
//! merged at [`MERGE`]. Only tight centroids merge, since a running centroid drifts toward a "mean
//! voice", and only centroids, since a long stream's window similarity matrix does not fit in memory.

/// Pass 1: a window joins the most similar cluster when at least this similar, else starts one.
/// Tight on purpose: a cluster should hold one voice, even if a voice ends up in several clusters.
pub const FIRST: f32 = 0.7;
/// Pass 2: centroids at least this similar are one speaker. 0.75-0.8 is where the speaker count
/// holds steady; 0.7 joins two voices, 0.85 and up splits them.
pub const MERGE: f32 = 0.78;

/// Fewest windows a cluster needs to be a speaker of its own, on top of the minimum share: on little
/// talk the share is under one window, so a laugh would be a person. 3 windows are 4.5 s of a voice.
pub const MIN_WINDOWS: usize = 3;

/// Whether a cluster of `count` of `n` windows is big enough to be a speaker of its own.
fn big_enough(count: usize, n: usize, min_share: f64) -> bool {
    count >= MIN_WINDOWS && count as f64 >= min_share * n as f64
}

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Eight running sums rather than one, so the compiler can keep them in one vector register: a
/// single sum must add in order and stays scalar. Labelling is millions of these dot products.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail: f32 = ca
        .remainder()
        .iter()
        .zip(cb.remainder())
        .map(|(x, y)| x * y)
        .sum();
    let mut acc = [0.0f32; 8];
    for (x, y) in ca.zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    acc.iter().sum::<f32>() + tail
}

/// `v` scaled to unit length (zero stays zero).
fn unit(v: &[f32]) -> Vec<f32> {
    let mut u = v.to_vec();
    normalize(&mut u);
    u
}

/// Most groups pass 1 makes; past it a window that resembles no group joins the nearest one. Pass 2
/// is O(k^3) over a k x k matrix, so k must stay bounded however long the recording.
pub const MAX_GROUPS: usize = 400;

/// A cluster: the sum of its members, its unit-length direction (the centroid, kept so a
/// similarity is one dot product), and how many members there are.
struct Group {
    sum: Vec<f32>,
    unit: Vec<f32>,
    count: usize,
}

impl Group {
    fn add(&mut self, e: &[f32], count: usize) {
        self.sum.iter_mut().zip(e).for_each(|(c, x)| *c += x);
        self.unit = unit(&self.sum);
        self.count += count;
    }
}

/// Pass 1: centroid clusters at `threshold`, never merged, at most `cap` of them. Returns the
/// groups and each window's.
fn first_pass(embeddings: &[Vec<f32>], threshold: f32, cap: usize) -> (Vec<Group>, Vec<usize>) {
    let mut groups: Vec<Group> = Vec::new();
    let mut labels = Vec::with_capacity(embeddings.len());
    for e in embeddings {
        let e = unit(e);
        let best = groups
            .iter()
            .enumerate()
            .map(|(i, g)| (i, dot(&g.unit, &e)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, s)) if s >= threshold || groups.len() >= cap => {
                groups[i].add(&e, 1);
                labels.push(i);
            }
            _ => {
                groups.push(Group {
                    sum: e.clone(),
                    unit: e,
                    count: 1,
                });
                labels.push(groups.len() - 1);
            }
        }
    }
    (groups, labels)
}

/// Pass 2: merges the closest pair of live groups while their centroids are at least `threshold`
/// similar. Keeps the similarity matrix and recomputes only the merged group's row, so each merge
/// is O(k^2) for k groups (at most [`MAX_GROUPS`]).
fn merge(groups: &mut [Group], threshold: f32) -> (Vec<bool>, Vec<usize>) {
    let k = groups.len();
    let mut alive = vec![true; k];
    // Which group each one was folded into (itself while alive).
    let mut into: Vec<usize> = (0..k).collect();
    let mut sim = vec![f32::NEG_INFINITY; k * k];
    for i in 0..k {
        for j in 0..i {
            sim[i * k + j] = dot(&groups[i].unit, &groups[j].unit);
        }
    }
    loop {
        let mut best = (f32::NEG_INFINITY, 0, 0);
        for i in 0..k {
            if !alive[i] {
                continue;
            }
            for j in 0..i {
                if alive[j] && sim[i * k + j] > best.0 {
                    best = (sim[i * k + j], i, j);
                }
            }
        }
        let (s, i, j) = best;
        if s < threshold {
            return (alive, into);
        }
        // Fold i into j (j < i).
        into[i] = j;
        let add = std::mem::take(&mut groups[i].sum);
        let count = groups[i].count;
        groups[j].add(&add, count);
        alive[i] = false;
        for o in 0..k {
            if alive[o] && o != j {
                let c = dot(&groups[j].unit, &groups[o].unit);
                let (a, b) = if o > j { (o, j) } else { (j, o) };
                sim[a * k + b] = c;
            }
        }
    }
}

/// Speaker label per embedding, two-pass (module docs). Clusters under `min_share` or
/// [`MIN_WINDOWS`] go to the nearest kept centroid however unlike it, as a word needs a speaker to
/// show under; when none is big enough, all are kept. Labels are dense, by first appearance.
pub fn two_pass(embeddings: &[Vec<f32>], first: f32, merge_at: f32, min_share: f64) -> Vec<usize> {
    two_pass_capped(embeddings, first, merge_at, min_share, MAX_GROUPS)
}

fn two_pass_capped(
    embeddings: &[Vec<f32>],
    first: f32,
    merge_at: f32,
    min_share: f64,
    cap: usize,
) -> Vec<usize> {
    if embeddings.is_empty() {
        return Vec::new();
    }
    let (groups, _) = merged(embeddings, first, merge_at, cap);
    let big: Vec<&Group> = groups
        .iter()
        .filter(|g| big_enough(g.count, embeddings.len(), min_share))
        .collect();
    let kept = if big.is_empty() {
        groups.iter().collect()
    } else {
        big
    };
    let mut dense: Vec<Option<usize>> = vec![None; kept.len()];
    let mut next = 0;
    embeddings
        .iter()
        .map(|e| {
            let g = nearest(kept.iter().map(|g| g.unit.as_slice()), e).expect("a group");
            *dense[g].get_or_insert_with(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

/// Passes 1 and 2: the groups left after merging, each with its unit centroid and member count,
/// and the group (an index into them) each embedding ended up in.
fn merged(
    embeddings: &[Vec<f32>],
    first: f32,
    merge_at: f32,
    cap: usize,
) -> (Vec<Group>, Vec<usize>) {
    let (mut groups, labels) = first_pass(embeddings, first, cap.max(1));
    let (alive, into) = merge(&mut groups, merge_at);
    let root = |mut g: usize| {
        while into[g] != g {
            g = into[g];
        }
        g
    };
    let mut index = vec![usize::MAX; groups.len()];
    let mut live = Vec::new();
    for (g, group) in groups.into_iter().enumerate() {
        if alive[g] {
            index[g] = live.len();
            live.push(group);
        }
    }
    let members = labels.into_iter().map(|l| index[root(l)]).collect();
    (live, members)
}

/// Index of the candidate direction most similar to `e`.
fn nearest<'a>(candidates: impl Iterator<Item = &'a [f32]>, e: &[f32]) -> Option<usize> {
    candidates
        .enumerate()
        .map(|(i, c)| (i, dot(c, e)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A window at least this similar to an enrolled voiceprint is that person's, before any
/// clustering. Own windows sit near 0.54-0.80, others' near 0.46-0.65; 0.6 leaves most of a
/// stranger's windows for clustering.
pub const VOICE_MATCH: f32 = 0.6;
/// A cluster of the remaining windows whose centroid is at least this similar to a voiceprint is
/// that person. Higher than [`VOICE_MATCH`]: a centroid is an average, so much cleaner than one
/// window (own voice near 0.97, a similar other voice near 0.77).
pub const CENTROID_MATCH: f32 = 0.85;

/// Speakers per window, with enrolled voices.
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    /// Speaker index per window, dense from 0: the enrolled voices heard, in the voices' order,
    /// then the other speakers, most windows first.
    pub windows: Vec<usize>,
    /// Per speaker, the voice (an index into the voiceprints) it is; `None` for "Speaker N".
    pub voices: Vec<Option<usize>>,
}

/// Labels windows `threshold`-similar to an enrolled voiceprint as that voice, then clusters the
/// rest as [`two_pass`]; a cluster `centroid`-similar to a voice is that voice too. A voice is never
/// dropped however few its windows; with no voice heard and no cluster big enough, all clusters stay
/// rather than strangers taking absent voices. Voiceprints must be unit length.
pub fn label(
    embeddings: &[Vec<f32>],
    voices: &[Vec<f32>],
    threshold: f32,
    centroid: f32,
    min_share: f64,
) -> Labels {
    label_fixed(embeddings, voices, &[], threshold, centroid, min_share)
}

/// [`label`], with some windows' voice already known: `fixed[i]` is the voice window `i` is
/// whatever it sounds like (the user pinned those seconds to that person), and it is left out of
/// matching and clustering, so a pinned window neither joins a cluster nor pulls one toward it.
/// Windows past the end of `fixed`, or `None` there, are labelled as [`label`] does.
pub fn label_fixed(
    embeddings: &[Vec<f32>],
    voices: &[Vec<f32>],
    fixed: &[Option<usize>],
    threshold: f32,
    centroid: f32,
    min_share: f64,
) -> Labels {
    #[derive(Clone, Copy, PartialEq)]
    enum Who {
        Voice(usize),
        Cluster(usize),
    }
    let n = embeddings.len();
    let close_voice = |e: &[f32], threshold: f32| {
        nearest(voices.iter().map(Vec::as_slice), e).filter(|&v| dot(&voices[v], e) >= threshold)
    };
    // Windows like a voice are that voice; the rest (by index into `rest`) are clustered.
    let mut who: Vec<Option<Who>> = Vec::with_capacity(n);
    let mut rest: Vec<(usize, &Vec<f32>)> = Vec::new();
    for (i, e) in embeddings.iter().enumerate() {
        if let Some(v) = fixed
            .get(i)
            .copied()
            .flatten()
            .filter(|&v| v < voices.len())
        {
            who.push(Some(Who::Voice(v)));
            continue;
        }
        match close_voice(e, threshold) {
            Some(v) => who.push(Some(Who::Voice(v))),
            None => {
                who.push(None);
                rest.push((i, e));
            }
        }
    }
    let rest_e: Vec<Vec<f32>> = rest.iter().map(|(_, e)| (*e).clone()).collect();
    if voices.is_empty() {
        for ((i, _), c) in rest.iter().zip(two_pass(&rest_e, FIRST, MERGE, min_share)) {
            who[*i] = Some(Who::Cluster(c));
        }
    } else if !rest.is_empty() {
        let (groups, members) = merged(&rest_e, FIRST, MERGE, MAX_GROUPS);
        // A cluster like a voice is that voice as a whole: sending its windows one by one to
        // their nearest candidate would split a person between their voice and someone else's.
        let as_voice: Vec<Option<usize>> = groups
            .iter()
            .map(|g| close_voice(&g.unit, centroid))
            .collect();
        let kept: Vec<&Group> = groups
            .iter()
            .zip(&as_voice)
            .filter(|(g, v)| v.is_none() && big_enough(g.count, n, min_share))
            .map(|(g, _)| g)
            .collect();
        // No cluster big enough and no voice heard: keep them all, as `two_pass` does.
        // Folding them into the voices would name strangers after people who are not here.
        let heard = who.iter().any(Option::is_some) || as_voice.iter().any(Option::is_some);
        let kept: Vec<&Group> = if kept.is_empty() && !heard {
            groups.iter().collect()
        } else {
            kept
        };
        // Candidates for the other windows: the kept clusters, then the voices.
        let candidates: Vec<&[f32]> = kept
            .iter()
            .map(|g| g.unit.as_slice())
            .chain(voices.iter().map(Vec::as_slice))
            .collect();
        for ((i, e), &g) in rest.iter().zip(&members) {
            if let Some(v) = as_voice[g] {
                who[*i] = Some(Who::Voice(v));
                continue;
            }
            let c = nearest(candidates.iter().copied(), e).expect("a voice at least");
            who[*i] = Some(if c < kept.len() {
                Who::Cluster(c)
            } else {
                Who::Voice(c - kept.len())
            });
        }
    }
    let who: Vec<Who> = who.into_iter().map(|w| w.expect("labelled")).collect();
    // Dense speaker ids: the voices heard in their order, then clusters by size.
    let mut counts: std::collections::HashMap<usize, usize> = Default::default();
    for w in &who {
        if let Who::Cluster(c) = w {
            *counts.entry(*c).or_default() += 1;
        }
    }
    let mut clusters: Vec<(usize, usize)> = counts.into_iter().collect();
    clusters.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let order: Vec<Who> = (0..voices.len())
        .map(Who::Voice)
        .filter(|v| who.contains(v))
        .chain(clusters.iter().map(|&(c, _)| Who::Cluster(c)))
        .collect();
    Labels {
        windows: who
            .iter()
            .map(|w| order.iter().position(|o| o == w).expect("ordered"))
            .collect(),
        voices: order
            .iter()
            .map(|o| match o {
                Who::Voice(v) => Some(*v),
                Who::Cluster(_) => None,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
