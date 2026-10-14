use super::*;

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
    // Speaker 0 is the bigger voice (the second one): talk-time order.
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
    // Twenty windows of Ana; the user pinned the last five to Bea.
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
    // 31 windows, so the 3% share is under one window: a lone window must not be a speaker of
    // its own. It is nearest the first voice, unlike enough to be its own cluster.
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
fn the_floor_decides_on_a_short_recording_and_the_share_on_a_long_one() {
    // Three windows of a third voice: at the floor. Beside 40 and 40 (2.5 windows' share)
    // they are a speaker; beside 200 and 200 they are under 3% and fold.
    for (each, speakers) in [(40, 3), (200, 2)] {
        let mut e = voice(&axis(0), each, 0.2);
        e.extend(voice(&axis(1), each, 0.2));
        e.extend(voice(&axis(2), MIN_WINDOWS, 0.2));
        let l = two_pass(&e, FIRST, MERGE, 0.03);
        assert_eq!(l.iter().max(), Some(&(speakers - 1)), "{each} each");
    }
}

#[test]
fn a_short_clip_with_no_cluster_big_enough_keeps_its_clusters_with_or_without_voices() {
    // Two windows of each of two strangers: no cluster reaches the floor. They stay two
    // speakers, and a remembered voice not heard here does not take them.
    let mut e = voice(&axis(0), 2, 0.2);
    e.extend(voice(&axis(1), 2, 0.2));
    assert_eq!(two_pass(&e, FIRST, MERGE, 0.03), vec![0, 0, 1, 1]);
    let l = label(&e, &[unit(&axis(6))], VOICE_MATCH, CENTROID_MATCH, 0.03);
    assert_eq!(l.voices, vec![None, None]);
    assert_eq!(l.windows, vec![0, 0, 1, 1]);
    // When a voice is heard, the small clusters fold into the nearest speaker, the voice.
    let mut heard = e.clone();
    heard.push(unit(&axis(6)));
    let l = label(&heard, &[unit(&axis(6))], VOICE_MATCH, CENTROID_MATCH, 0.03);
    assert_eq!(l.voices, vec![Some(0)]);
}

#[test]
fn a_small_cluster_like_a_voice_is_that_voice() {
    // Two windows, each under the window threshold against Ana's print, whose centroid is
    // over the centroid one: a cluster under the floor, kept as Ana, never folded.
    let mut e = voice(&axis(1), 15, 0.2);
    e.extend(voice(&axis(2), 15, 0.2));
    for k in [3, 4] {
        let mut v = axis(0);
        v[k] = 0.5;
        e.push(unit(&v));
    }
    let print = unit(&axis(0));
    assert!(e[30..].iter().all(|w| dot(w, &print) < 0.95));
    let l = label(&e, &[print], 0.95, CENTROID_MATCH, 0.03);
    assert_eq!(l.voices, vec![Some(0), None, None]);
    assert_eq!(&l.windows[30..], &[0, 0]);
    // Without the voice the same two windows fold into a stranger.
    let l = label(&e, &[], 0.95, CENTROID_MATCH, 0.03);
    assert_eq!(l.voices, vec![None, None]);
}

#[test]
fn dot_matches_the_plain_sum() {
    let a: Vec<f32> = (0..515).map(|i| (i as f32 * 0.37).sin()).collect();
    let b: Vec<f32> = (0..515).map(|i| (i as f32 * 0.11).cos()).collect();
    let plain: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    assert!((dot(&a, &b) - plain).abs() < 1e-3);
}
