//! Which tracks to transcribe. Pure, so it is testable without ffmpeg.

use crate::tracks::{LogicalTrack, Role};

/// Voice tracks when the recording has them, so speech is heard without game audio over it;
/// otherwise the mix; otherwise any audible track whose role is unclear.
pub fn speech_tracks(logical: &[LogicalTrack]) -> Vec<&LogicalTrack> {
    for role in [Role::Voice, Role::Mix, Role::Unknown] {
        let found: Vec<&LogicalTrack> = logical.iter().filter(|t| t.role == role).collect();
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::Activity;
    use crate::tracks::Sameness;

    fn track(i: usize, role: Role) -> LogicalTrack {
        LogicalTrack {
            members: vec![i],
            sameness: Sameness::Single,
            activity: Activity {
                mean_db: -30.0,
                peak_db: -10.0,
                near_silent_share: 0.5,
                silent: role == Role::Silent,
            },
            role,
            confidence: 0.5,
            reasons: Vec::new(),
        }
    }

    fn picked(l: &[LogicalTrack]) -> Vec<usize> {
        speech_tracks(l).iter().map(|t| t.members[0]).collect()
    }

    #[test]
    fn voices_beat_the_mix() {
        let l = [
            track(0, Role::Mix),
            track(1, Role::Voice),
            track(2, Role::GamePc),
            track(3, Role::Voice),
        ];
        assert_eq!(picked(&l), vec![1, 3]);
    }

    #[test]
    fn falls_back_to_mix_then_unknown() {
        assert_eq!(
            picked(&[track(0, Role::Mix), track(1, Role::Silent)]),
            vec![0]
        );
        assert_eq!(
            picked(&[track(0, Role::Unknown), track(1, Role::Silent)]),
            vec![0]
        );
        assert!(picked(&[track(0, Role::Silent), track(1, Role::GamePc)]).is_empty());
    }
}
