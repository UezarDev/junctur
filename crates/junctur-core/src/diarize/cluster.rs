//! Grouping speaker embeddings into speakers, in two passes.
//!
//! Pass 1 groups windows into many tight clusters (centroid clustering at [`FIRST`]); pass 2 merges
//! the clusters' centroids, closest pair first, while they are at least [`MERGE`]-similar; clusters
//! still under a minimum share, or under [`MIN_WINDOWS`], are dropped, and every window goes to its
//! nearest remaining centroid.
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

/// Fewest windows a cluster needs to be a speaker of its own, whatever its share (5u). Clusters
/// under it fold into their nearest speaker like those under the minimum share. The share alone
/// vanished on little talk: 3% of the 2-minute clip's 33 windows is under one, so five single
/// windows (a laugh, one short line) were five people, nine speakers for three. Measured there
/// against the user's pins: a floor of 2 still split one person in three; 3 gives two speakers.
/// 3 windows are 4.5 s of one voice, so a few sentences stay a person. Added to the share, never
/// in place of it: alone, on the 1 h 45 4-track sample, it kept 13 speakers for 3, its clusters of
/// 3 to 31 windows being pieces of the main voices (0.70-0.78 to them, just under [`MERGE`]). It
/// decides up to 100 windows (2.5 min of speech), the share past that.
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
/// single sum must add in order and stays scalar. Labelling a 30 h recording is millions of
/// 512-number dot products, on every `analyze`, `cut`, enrollment and rename (5b repair pass).
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

/// Speaker label per embedding, two-pass (module docs). Clusters holding under `min_share` of the
/// windows, or under [`MIN_WINDOWS`] of them, are dropped and their windows go to the nearest kept centroid however unlike it they
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

/// Passes 1 and 2: the groups left after merging, each with its unit centroid and member count.
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

/// A window at least this similar to an enrolled voiceprint is that person's (5b), before any
/// clustering. Measured on the 4-track sample against the PNGtuber avatars (5b's phase file): a
/// person's own windows sit at 0.54-0.80 against their print (10th-90th percentile), other people's
/// at 0.46-0.65 (median-90th); with everyone enrolled 0.5-0.7 gives the same labels, as windows
/// under it still reach their nearest voice. 0.6 keeps most of a stranger's windows for clustering.
pub const VOICE_MATCH: f32 = 0.6;
/// A cluster of the remaining windows whose centroid is at least this similar to a voiceprint is
/// that person. Higher than [`VOICE_MATCH`] because a centroid is an average and so much cleaner
/// than one window: on the sample a person's own windows average to 0.97-0.98 against their print,
/// while Uezar's brother, the nearest voice to his, averages 0.77 against Uezar's.
pub const CENTROID_MATCH: f32 = 0.85;

/// Speakers per window, with enrolled voices (5b).
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    /// Speaker index per window, dense from 0: the enrolled voices heard, in the voices' order,
    /// then the other speakers, most windows first.
    pub windows: Vec<usize>,
    /// Per speaker, the voice (an index into the voiceprints) it is; `None` for "Speaker N".
    pub voices: Vec<Option<usize>>,
}

/// Labels windows by enrolled voiceprints first (5b), then 5a's two passes for the rest.
///
/// A window at least `threshold` similar to its nearest voice is that voice. The others are
/// clustered in two passes; a cluster whose centroid is at least `centroid` similar to a voice is
/// that voice too (a person's quieter windows, clustered together, are still them); clusters under
/// `min_share` of all windows, or under [`MIN_WINDOWS`], are dropped (never a voice: a window or
/// cluster like a voice, and a pinned window, is that voice whatever its size, 5u); then every one of these windows goes to its nearest kept
/// cluster or voice, as in [`two_pass`]. With no voices this is exactly [`two_pass`] (the clusters
/// ordered by size). Voiceprints must be unit length, like the embeddings.
pub fn label(
    embeddings: &[Vec<f32>],
    voices: &[Vec<f32>],
    threshold: f32,
    centroid: f32,
    min_share: f64,
) -> Labels {
    label_fixed(embeddings, voices, &[], threshold, centroid, min_share)
}

/// [`label`], with some windows' voice already known (5c): `fixed[i]` is the voice window `i` is
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
        // their nearest candidate split a person between their voice and someone else's
        // (the 5b review).
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

    fn axis(i: usize) -> Vec<f32> {
        let mut v = vec![0.0; 8];
        v[i] = 1.0;
        v
    }

    #[test]
    fn without_voices_labelling_is_two_pass_with_the_biggest_first() {
        let mut e = voice(&axis(0), 10, 0.2);
        e.extend(voice(&axis(1), 30, 0.2));
        let l = label(&e, &[], VOICE_MATCH, CENTROID_MATCH, 0.03);
        assert_eq!(l.voices, vec![None, None]);
        // Speaker 0 is the bigger voice (the second one), like 5a's talk-time order.
        assert!(l.windows[..10].iter().all(|&x| x == 1) && l.windows[10..].iter().all(|&x| x == 0));
        assert!(label(&[], &[], VOICE_MATCH, CENTROID_MATCH, 0.03)
            .windows
            .is_empty());
    }

    #[test]
    fn enrolled_voices_come_first_and_a_stranger_is_a_speaker_of_their_own() {
        let ana = voice(&axis(0), 20, 0.3);
        let bea = voice(&axis(1), 20, 0.3);
        let stranger = voice(&axis(2), 20, 0.3);
        let mut e: Vec<Vec<f32>> = stranger.clone();
        e.extend(bea.clone());
        e.extend(ana.clone());
        // Voices: Ana, Bea, and Cleo who never talks here.
        let prints = vec![unit(&axis(0)), unit(&axis(1)), unit(&axis(5))];
        let l = label(&e, &prints, VOICE_MATCH, CENTROID_MATCH, 0.03);
        // Ana and Bea in the voices' order, then the stranger; Cleo is not a speaker.
        assert_eq!(l.voices, vec![Some(0), Some(1), None]);
        assert!(l.windows[..20].iter().all(|&x| x == 2), "{:?}", l.windows);
        assert!(l.windows[20..40].iter().all(|&x| x == 1));
        assert!(l.windows[40..].iter().all(|&x| x == 0));
    }

    #[test]
    fn a_fixed_window_is_its_voice_whatever_it_sounds_like() {
        // Twenty windows of Ana; the user pinned the last five to Bea (5c).
        let e = voice(&axis(0), 20, 0.3);
        let prints = vec![unit(&axis(0)), unit(&axis(1))];
        let mut fixed = vec![None; 15];
        fixed.extend([Some(1); 5]);
        let l = label_fixed(&e, &prints, &fixed, VOICE_MATCH, CENTROID_MATCH, 0.03);
        assert_eq!(l.voices, vec![Some(0), Some(1)]);
        assert!(l.windows[..15].iter().all(|&x| x == 0), "{:?}", l.windows);
        assert!(l.windows[15..].iter().all(|&x| x == 1), "{:?}", l.windows);
        // Nothing fixed, or a voice that is not there: as `label`.
        let plain = label(&e, &prints, VOICE_MATCH, CENTROID_MATCH, 0.03);
        assert_eq!(
            label_fixed(&e, &prints, &[Some(7)], VOICE_MATCH, CENTROID_MATCH, 0.03),
            plain
        );
    }

    #[test]
    fn a_voices_quieter_windows_clustered_together_are_still_that_voice() {
        // Windows under the window threshold but whose average is the voice.
        let print = unit(&axis(0));
        let mut v = axis(0);
        v[3] = 0.5; // cosine 0.89 to the print, under a window threshold of 0.95
        let mut w = axis(0);
        w[4] = 0.5;
        let e: Vec<Vec<f32>> = (0..20)
            .map(|i| unit(if i % 2 == 0 { &v } else { &w }))
            .collect();
        assert!(dot(&e[0], &print) < 0.95);
        let l = label(&e, &[print], 0.95, CENTROID_MATCH, 0.03);
        assert_eq!(l.voices, vec![Some(0)]);
        assert!(l.windows.iter().all(|&x| x == 0));
    }

    #[test]
    fn a_cluster_like_a_voice_goes_to_it_whole() {
        // Two voices; a group of Ana's windows each under the window threshold, some of them
        // individually nearer Bea's print, but as a group plainly Ana.
        let ana = unit(&axis(0));
        let mut bea = axis(0);
        bea[1] = 1.4;
        let bea = unit(&bea);
        let e: Vec<Vec<f32>> = (0..20)
            .map(|i| {
                let mut v = axis(0);
                v[3 + i % 4] = 0.5;
                // Every fifth window leans towards Bea, enough to be nearer her print alone.
                if i % 5 == 4 {
                    v[1] = 0.6;
                }
                unit(&v)
            })
            .collect();
        assert!(e.iter().any(|w| dot(w, &bea) > dot(w, &ana)));
        let l = label(&e, &[ana, bea], 0.99, CENTROID_MATCH, 0.03);
        assert_eq!(l.voices, vec![Some(0)]);
        assert!(l.windows.iter().all(|&x| x == 0), "{:?}", l.windows);
    }

    #[test]
    fn a_single_window_folds_into_its_nearest_speaker_on_a_short_recording() {
        // 5u: 31 windows, so the 3% share is under one window and a lone window used to be a
        // speaker of its own. It is nearest the first voice, unlike enough to be its own cluster.
        let mut e = voice(&axis(0), 15, 0.2);
        e.extend(voice(&axis(1), 15, 0.2));
        e.push(unit(&[0.65, 0.0, 0.76, 0.0, 0.0, 0.0, 0.0, 0.0]));
        let (groups, _) = merged(&e, FIRST, MERGE, MAX_GROUPS);
        assert_eq!(groups.len(), 3, "the lone window is a cluster of its own");
        let l = two_pass(&e, FIRST, MERGE, 0.03);
        assert_eq!(l.iter().max(), Some(&1), "{l:?}");
        assert_eq!(l[30], l[0]);
        // The same with voices enrolled: the lone window is not a third speaker either.
        let l = label(&e, &[unit(&axis(6))], VOICE_MATCH, CENTROID_MATCH, 0.03);
        assert_eq!(l.voices, vec![None, None]);
        assert_eq!(l.windows[30], l.windows[0]);
    }

    #[test]
    fn a_short_speaker_with_a_few_sentences_stays_a_speaker() {
        // Four windows (6 s) of a third voice on a short recording: over the floor, a person.
        let mut e = voice(&axis(0), 15, 0.2);
        e.extend(voice(&axis(1), 15, 0.2));
        e.extend(voice(&axis(2), MIN_WINDOWS + 1, 0.2));
        let l = two_pass(&e, FIRST, MERGE, 0.03);
        assert_eq!(l.iter().max(), Some(&2), "{l:?}");
        assert!(l[30..].iter().all(|&x| x == l[30]) && l[30] != l[0] && l[30] != l[15]);
        // On a long recording the floor asks nothing more than the share: 13 of 413 windows is
        // over 3%, and they stay a speaker.
        let mut e = voice(&axis(0), 200, 0.2);
        e.extend(voice(&axis(1), 200, 0.2));
        e.extend(voice(&axis(2), 13, 0.2));
        let l = two_pass(&e, FIRST, MERGE, 0.03);
        assert_eq!(l.iter().max(), Some(&2));
        assert!(l[400..].iter().all(|&x| x == l[400]) && l[400] != l[0]);
    }

    #[test]
    fn a_remembered_or_pinned_single_window_is_never_folded() {
        // One window that is a remembered voice (Ana's print), and one window of the first
        // stranger's voice the user pinned to Bea: each a speaker though a single window.
        let mut e = voice(&axis(0), 15, 0.2);
        e.extend(voice(&axis(1), 15, 0.2));
        e.push(unit(&axis(2)));
        e.push(e[0].clone());
        let prints = vec![unit(&axis(2)), unit(&axis(7))];
        let mut fixed = vec![None; 31];
        fixed.push(Some(1));
        let l = label_fixed(&e, &prints, &fixed, VOICE_MATCH, CENTROID_MATCH, 0.03);
        assert_eq!(l.voices, vec![Some(0), Some(1), None, None]);
        assert_eq!(l.windows[30], 0);
        assert_eq!(l.windows[31], 1);
        assert!(l.windows[..30].iter().all(|&x| x >= 2), "{:?}", l.windows);
    }

    #[test]
    fn dot_matches_the_plain_sum() {
        let a: Vec<f32> = (0..515).map(|i| (i as f32 * 0.37).sin()).collect();
        let b: Vec<f32> = (0..515).map(|i| (i as f32 * 0.11).cos()).collect();
        let plain: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        assert!((dot(&a, &b) - plain).abs() < 1e-3);
    }
}
