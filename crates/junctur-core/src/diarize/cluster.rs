//! Grouping speaker embeddings into speakers, in two passes.
//!
//! Pass 1 groups windows into many tight clusters (centroid clustering at [`FIRST`]); pass 2 merges
//! the clusters' centroids, closest pair first, while they are at least [`MERGE`]-similar; clusters
//! still under a minimum share are dropped, and every window goes to its nearest remaining centroid.
//!
//! 1a's one pass (merge at 0.5 as it went, then fold strays) found a single speaker on the 4-track
//! sample, on the vocal stem and on the mix alike: a running centroid drifts toward a "mean voice"
//! that every window is close to. Merging only centroids, and only once they are tight, gives three
//! speakers on both sources, agreeing on 88% of windows (5a's measurement). Agglomerative clustering
//! over all windows was rejected in 1a and still is: a 30 h stream has tens of thousands of windows,
//! and their similarity matrix does not fit in memory; pass 1's few hundred centroids do.

/// Pass 1: a window joins the most similar cluster when at least this similar, else starts one.
/// Tight on purpose: a cluster should hold one voice, even if a voice ends up in several clusters.
pub const FIRST: f32 = 0.7;
/// Pass 2: centroids at least this similar are one speaker. On the 4-track sample 0.75-0.8 is a
/// plateau (three speakers, stem and mix agreeing); 0.7 joins two of them, 0.85 and up splits voices
/// into five or more.
pub const MERGE: f32 = 0.78;

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `v` scaled to unit length (zero stays zero).
fn unit(v: &[f32]) -> Vec<f32> {
    let mut u = v.to_vec();
    normalize(&mut u);
    u
}

/// Most groups pass 1 makes. Past it a window that resembles no group joins the nearest one
/// instead of starting another: pass 2 keeps a k x k similarity matrix and merges in O(k^3) over
/// all, so k must stay bounded however long the recording. The 4-track sample (1 h 45, 1 425
/// windows) makes 91, merged in 4 ms; at the cap a merge pass is ~0.3 s.
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
fn merge(groups: &mut [Group], threshold: f32) -> Vec<bool> {
    let k = groups.len();
    let mut alive = vec![true; k];
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
            return alive;
        }
        // Fold i into j (j < i).
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

/// Speaker label per embedding, two-pass (module docs). Clusters holding under `min_share` of the
/// windows are dropped and their windows go to the nearest kept centroid however unlike it they
/// are: 1a left a stray under 0.3 similar as a "minor" speaker of its own, but a word needs a
/// speaker to be shown under, and a minor one is noise in a transcript or a caption. When no
/// cluster reaches the share, every merged cluster is kept. Labels are dense, ordered by first
/// appearance.
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
    let (mut groups, _) = first_pass(embeddings, first, cap.max(1));
    let alive = merge(&mut groups, merge_at);
    let live: Vec<usize> = (0..groups.len()).filter(|&g| alive[g]).collect();
    let big: Vec<usize> = live
        .iter()
        .copied()
        .filter(|&g| groups[g].count as f64 >= min_share * embeddings.len() as f64)
        .collect();
    let kept = if big.is_empty() { live } else { big };
    let mut dense: Vec<Option<usize>> = vec![None; groups.len()];
    let mut next = 0;
    embeddings
        .iter()
        .map(|e| {
            let g = kept
                .iter()
                .copied()
                .max_by(|&a, &b| dot(&groups[a].unit, e).total_cmp(&dot(&groups[b].unit, e)))
                .expect("at least one group");
            *dense[g].get_or_insert_with(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let mut v = v.to_vec();
        normalize(&mut v);
        v
    }

    /// `n` embeddings spread around `centre`, each tilted a little along its own axis.
    fn voice(centre: &[f32], n: usize, spread: f32) -> Vec<Vec<f32>> {
        (0..n)
            .map(|i| {
                let mut v = centre.to_vec();
                let axis = 3 + i % (v.len() - 3);
                v[axis] += spread;
                unit(&v)
            })
            .collect()
    }

    #[test]
    fn two_voices_make_two_speakers_whatever_the_order() {
        let a = voice(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 20, 0.3);
        let b = voice(&[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 20, 0.3);
        let mixed: Vec<_> = a
            .iter()
            .zip(&b)
            .flat_map(|(x, y)| [x.clone(), y.clone()])
            .collect();
        let l = two_pass(&mixed, FIRST, MERGE, 0.03);
        assert_eq!(l.iter().max(), Some(&1));
        assert!(l.chunks(2).all(|p| p[0] == 0 && p[1] == 1), "{l:?}");
    }

    #[test]
    fn one_voice_split_in_pass_one_is_merged_in_pass_two() {
        // Two tight sub-groups of one voice: apart enough for pass 1, close as centroids.
        let a = voice(&[1.0, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 10, 0.1);
        let b = voice(&[1.0, -0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 10, 0.1);
        let all: Vec<_> = a.into_iter().chain(b).collect();
        let (groups, _) = first_pass(&all, 0.97, MAX_GROUPS);
        assert!(groups.len() >= 2);
        let l = two_pass(&all, 0.97, MERGE, 0.03);
        assert!(l.iter().all(|&x| x == 0), "{l:?}");
    }

    #[test]
    fn strays_go_to_the_nearest_speaker() {
        let mut e = voice(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 40, 0.2);
        e.extend(voice(&[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 40, 0.2));
        e.push(unit(&[0.5, 0.0, 0.86, 0.0, 0.0, 0.0, 0.0, 0.0])); // its own cluster, then dropped
        let l = two_pass(&e, FIRST, MERGE, 0.03);
        assert_eq!(l.iter().max(), Some(&1));
        assert_eq!(l[80], l[0]);
    }

    #[test]
    fn pass_one_stops_making_groups_at_the_cap() {
        // Eight orthogonal directions, each its own group uncapped.
        let e: Vec<Vec<f32>> = (0..8)
            .map(|i| {
                let mut v = vec![0.0; 8];
                v[i] = 1.0;
                v
            })
            .collect();
        assert_eq!(first_pass(&e, FIRST, MAX_GROUPS).0.len(), 8);
        let (groups, labels) = first_pass(&e, FIRST, 3);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups.iter().map(|g| g.count).sum::<usize>(), 8);
        assert!(labels.iter().all(|&l| l < 3));
        assert_eq!(two_pass_capped(&e, FIRST, MERGE, 0.0, 3).len(), 8);
    }

    #[test]
    fn empty_and_zero_vectors() {
        assert!(two_pass(&[], FIRST, MERGE, 0.03).is_empty());
        let mut z = vec![0.0; 4];
        normalize(&mut z);
        assert_eq!(z, vec![0.0; 4]);
        // Nothing reaches the share: every cluster is kept rather than none.
        let e = vec![unit(&[1.0, 0.0]), unit(&[0.0, 1.0])];
        assert_eq!(two_pass(&e, FIRST, MERGE, 0.9), vec![0, 1]);
    }
}
