//! Grouping duplicate tracks and guessing roles. Pure, so it is testable without ffmpeg.

use super::roles::{decide, Context, Speech};
use super::{LogicalTrack, Sameness};
use crate::ffmpeg::ANALYSIS_RATE;
use crate::signal::{self, Activity};

/// Mean level below which a non-silent track counts as "faint": faint tracks sit near -70 dB, a
/// main mix near -35 dB.
pub const FAINT_MEAN_DB: f64 = -55.0;
/// Correlation at or above which two non-identical tracks count as the same source (encoder noise,
/// a slightly different gain stage).
pub const NEAR_DUPLICATE_CORR: f64 = 0.999;
/// Two near-duplicates must also be within this level of each other; a ducked copy is a different
/// track for editing purposes.
pub const NEAR_DUPLICATE_DB: f64 = 1.0;
/// Residual energy share under which a track counts as a mix of the other tracks (-13 dB).
pub const MIX_RESIDUAL: f64 = 0.05;

/// Group, measure and label aligned per-track samples, by loudness alone.
pub fn classify(samples: &[Vec<f32>]) -> Vec<LogicalTrack> {
    classify_with(samples, &mut |_| None)
}

/// As [`classify`], with `speech_of` measuring speech on each distinct audible track (once per
/// group, on its first member).
pub fn classify_with(
    samples: &[Vec<f32>],
    speech_of: &mut dyn FnMut(&[f32]) -> Option<Speech>,
) -> Vec<LogicalTrack> {
    let acts: Vec<Activity> = samples
        .iter()
        .map(|s| signal::activity(s, ANALYSIS_RATE))
        .collect();

    let groups = group_duplicates(samples, &acts);

    // Roles, one per group.
    let audible: Vec<usize> = (0..groups.len())
        .filter(|&g| !acts[groups[g].0[0]].silent)
        .collect();
    let substantial = audible
        .iter()
        .filter(|&&g| acts[groups[g].0[0]].mean_db >= FAINT_MEAN_DB)
        .count();

    groups
        .iter()
        .enumerate()
        .map(|(g, (members, sameness))| {
            let rep = members[0];
            let a = acts[rep].clone();
            let mut reasons: Vec<String> = copies(members.len(), *sameness).into_iter().collect();
            let speech = if a.silent {
                None
            } else {
                speech_of(&samples[rep])
            };
            let cx = Context {
                sum_of_others: if a.silent {
                    None
                } else {
                    mix_of_others(g, rep, &groups, &audible, samples)
                },
                faint: a.mean_db < FAINT_MEAN_DB,
                substantial,
            };
            let (role, confidence, why) = decide(&a, speech, &cx);
            reasons.push(why);
            LogicalTrack {
                members: members.clone(),
                sameness: *sameness,
                activity: a,
                speech,
                role,
                confidence,
                reasons,
            }
        })
        .collect()
}

/// Tracks grouped as copies of one another: exact first, then near. Each track is compared with
/// each group's first member.
fn group_duplicates(samples: &[Vec<f32>], acts: &[Activity]) -> Vec<(Vec<usize>, Sameness)> {
    let mut groups: Vec<(Vec<usize>, Sameness)> = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        let mut placed = false;
        for (members, same) in groups.iter_mut() {
            let rep = members[0];
            if signal::identical(&samples[rep], s) {
                members.push(i);
                if *same == Sameness::Single {
                    *same = Sameness::Exact;
                }
                placed = true;
                break;
            }
            let close = signal::correlation(&samples[rep], s)
                .is_some_and(|c| c >= NEAR_DUPLICATE_CORR)
                && (acts[rep].mean_db - acts[i].mean_db).abs() <= NEAR_DUPLICATE_DB;
            if close {
                members.push(i);
                *same = Sameness::Near;
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push((vec![i], Sameness::Single));
        }
    }
    groups
}

/// The reason a group of `n` copies gives, when it has more than one track.
fn copies(n: usize, sameness: Sameness) -> Option<String> {
    let kind = if sameness == Sameness::Exact {
        "identical"
    } else {
        "near-identical"
    };
    (n > 1).then(|| format!("{n} tracks are {kind} copies; analysed once"))
}

/// Residual of group `g` fitted as a weighted sum of the other audible groups, when there are at
/// least two of them, the fit is good enough and every weight is non-negative (a mix adds its
/// sources; a part is only the mix minus the others).
fn mix_of_others(
    g: usize,
    rep: usize,
    groups: &[(Vec<usize>, Sameness)],
    audible: &[usize],
    samples: &[Vec<f32>],
) -> Option<f64> {
    let parts: Vec<&[f32]> = audible
        .iter()
        .filter(|&&o| o != g)
        .map(|&o| samples[groups[o].0[0]].as_slice())
        .collect();
    if parts.len() < 2 {
        return None;
    }
    let (r, w) = signal::sum_fit(&samples[rep], &parts)?;
    (r < MIX_RESIDUAL && w.iter().all(|&x| x > -0.05)).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::Role;

    fn tone(freq: f32, amp: f32) -> Vec<f32> {
        (0..16_000)
            .map(|i| amp * (i as f32 * freq * std::f32::consts::TAU / 16_000.0).sin())
            .collect()
    }

    #[test]
    fn four_copies_collapse_to_one_mix() {
        let t = tone(440.0, 0.3);
        let l = classify(&[t.clone(), t.clone(), t.clone(), t]);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].members, vec![0, 1, 2, 3]);
        assert_eq!(l[0].sameness, Sameness::Exact);
        assert_eq!(l[0].role, Role::Mix);
    }

    #[test]
    fn loud_faint_silent_like_the_three_track_file() {
        let l = classify(&[tone(440.0, 0.3), tone(700.0, 0.0005), vec![0.0; 16_000]]);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].role, Role::Mix);
        assert_eq!(l[1].role, Role::Unknown);
        assert_eq!(l[2].role, Role::Silent);
    }

    #[test]
    fn near_copy_collapses_but_a_ducked_copy_does_not() {
        let a = tone(440.0, 0.3);
        let noisy: Vec<f32> = a
            .iter()
            .enumerate()
            .map(|(i, x)| x + if i % 2 == 0 { 1e-4 } else { -1e-4 })
            .collect();
        let ducked: Vec<f32> = a.iter().map(|x| x * 0.5).collect();
        let l = classify(&[a, noisy, ducked]);
        assert_eq!(l[0].members, vec![0, 1]);
        assert_eq!(l[0].sameness, Sameness::Near);
        assert_eq!(l[1].members, vec![2]);
    }

    #[test]
    fn sum_track_is_the_mix_and_parts_get_roles() {
        let voice: Vec<f32> = tone(300.0, 0.3)
            .into_iter()
            .enumerate()
            .map(|(i, x)| if i < 8_000 { x } else { 0.0 })
            .collect();
        let game = tone(1700.0, 0.2);
        let mix: Vec<f32> = voice.iter().zip(&game).map(|(a, b)| a + b).collect();
        let l = classify(&[mix, voice, game]);
        assert_eq!(l[0].role, Role::Mix);
        assert_eq!(l[1].role, Role::Voice);
        assert_eq!(l[2].role, Role::GamePc);
    }

    #[test]
    fn no_tracks_no_logical_tracks() {
        assert!(classify(&[]).is_empty());
    }
}
