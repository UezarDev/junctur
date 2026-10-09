//! Grouping speaker embeddings into speakers.
//!
//! Centroid clustering in one pass, then merging close centroids. Agglomerative clustering over all
//! pairs was rejected: a 30h stream has tens of thousands of segments, and a full similarity matrix
//! of that size does not fit in memory.

/// Cosine similarity above which two embeddings are the same speaker. CAM++ same-speaker pairs on
/// VoxCeleb mostly score above 0.5; clean studio pairs higher.
pub const SAME_SPEAKER: f32 = 0.5;

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

fn cos(a: &[f32], b: &[f32]) -> f32 {
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa <= 0.0 || bb <= 0.0 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}

/// Cluster label per embedding; labels are dense, ordered by first appearance.
pub fn cluster(embeddings: &[Vec<f32>], threshold: f32) -> Vec<usize> {
    let mut sums: Vec<Vec<f32>> = Vec::new();
    let mut labels: Vec<usize> = Vec::with_capacity(embeddings.len());
    for e in embeddings {
        let best = sums
            .iter()
            .enumerate()
            .map(|(i, c)| (i, cos(c, e)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, s)) if s >= threshold => {
                sums[i].iter_mut().zip(e).for_each(|(c, x)| *c += x);
                labels.push(i);
            }
            _ => {
                sums.push(e.clone());
                labels.push(sums.len() - 1);
            }
        }
    }
    // Merge centroids that drifted together; map every label to its merged root.
    let mut root: Vec<usize> = (0..sums.len()).collect();
    for i in 0..sums.len() {
        for j in 0..i {
            if root[j] == j && cos(&sums[i], &sums[j]) >= threshold {
                root[i] = j;
                let add = sums[i].clone();
                sums[j].iter_mut().zip(&add).for_each(|(c, x)| *c += x);
                break;
            }
        }
    }
    let mut dense: Vec<Option<usize>> = vec![None; sums.len()];
    let mut next = 0;
    labels
        .iter()
        .map(|&l| {
            let r = root[l];
            *dense[r].get_or_insert_with(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

/// Similarity a stray segment needs to be folded into a main speaker.
pub const ABSORB: f32 = 0.3;

/// Fold clusters holding under `min_share` of the segments into the nearest large cluster when it
/// is at least [`ABSORB`]-similar. One-pass clustering leaves many one-segment clusters (laughs,
/// crosstalk, a noisy second); most belong to a main speaker.
pub fn absorb_small(embeddings: &[Vec<f32>], labels: &[usize], min_share: f64) -> Vec<usize> {
    let n = labels.iter().max().map_or(0, |m| m + 1);
    let mut count = vec![0usize; n];
    let mut sums = vec![vec![0.0f32; embeddings.first().map_or(0, Vec::len)]; n];
    for (e, &l) in embeddings.iter().zip(labels) {
        count[l] += 1;
        sums[l].iter_mut().zip(e).for_each(|(c, x)| *c += x);
    }
    let big: Vec<usize> = (0..n)
        .filter(|&c| count[c] as f64 >= min_share * labels.len() as f64)
        .collect();
    labels
        .iter()
        .zip(embeddings)
        .map(|(&l, e)| {
            if big.contains(&l) {
                return l;
            }
            big.iter()
                .map(|&b| (b, cos(&sums[b], e)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|&(_, s)| s >= ABSORB)
                .map_or(l, |(b, _)| b)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_directions_make_two_speakers() {
        let a = vec![1.0, 0.0, 0.0];
        let a2 = vec![0.9, 0.1, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let l = cluster(&[a.clone(), b.clone(), a2, b, a], SAME_SPEAKER);
        assert_eq!(l, vec![0, 1, 0, 1, 0]);
    }

    #[test]
    fn strays_join_the_nearest_big_speaker() {
        let mut e = vec![vec![1.0, 0.0, 0.0]; 10];
        e.extend(vec![vec![0.0, 1.0, 0.0]; 10]);
        e.push(vec![0.4, 0.0, 0.917]); // cos 0.4: its own cluster, then absorbed
        e.push(vec![0.0, -1.0, 0.0]); // unlike anyone
        let l = absorb_small(&e, &cluster(&e, SAME_SPEAKER), 0.1);
        assert_eq!(l[20], 0);
        assert_eq!(l[21], 3);
    }

    #[test]
    fn empty_and_zero_vectors() {
        assert!(absorb_small(&[], &[], 0.1).is_empty());
        assert!(cluster(&[], SAME_SPEAKER).is_empty());
        let mut z = vec![0.0; 4];
        normalize(&mut z);
        assert_eq!(z, vec![0.0; 4]);
    }
}
