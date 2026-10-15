use super::*;
use crate::test_support::scratch;
use junctur_core::signal::Activity;
use junctur_core::tracks::{LogicalTrack, Sameness};

fn logical(members: &[usize], role: Role) -> LogicalTrack {
    LogicalTrack {
        members: members.to_vec(),
        sameness: if members.len() > 1 {
            Sameness::Exact
        } else {
            Sameness::Single
        },
        activity: Activity {
            mean_db: -30.0,
            peak_db: -10.0,
            near_silent_share: 0.0,
            silent: false,
        },
        speech: None,
        role,
        confidence: 1.0,
        reasons: vec![],
    }
}

fn report(logical: Vec<LogicalTrack>) -> TrackReport {
    TrackReport {
        file: PathBuf::from("rec.mp4"),
        duration: 1.0,
        windows: vec![],
        window_seconds: 1.0,
        tracks: vec![],
        logical,
        speech_error: None,
        from_cache: true,
    }
}

fn muted(m: &Mixer) -> Vec<bool> {
    m.state().tracks.iter().map(|t| t.muted).collect()
}

#[test]
fn with_a_mix_only_the_mix_is_heard() {
    let r = report(vec![
        logical(&[0], Role::Mix),
        logical(&[1], Role::Voice),
        logical(&[2], Role::GamePc),
    ]);
    let m = Mixer::new(3, Some(&r), None);
    assert_eq!(muted(&m), [false, true, true]);
    assert_eq!(m.state().source, Source::Roles);
    assert_eq!(m.state().tracks[1].role, Some(Role::Voice));
    assert_eq!(m.weights(), "1 0 0");
    // The mix need not be track 1.
    let r = report(vec![logical(&[0], Role::Voice), logical(&[1], Role::Mix)]);
    assert_eq!(muted(&Mixer::new(2, Some(&r), None)), [true, false]);
}

#[test]
fn separate_tracks_are_all_heard_but_copies_are_not() {
    let r = report(vec![
        logical(&[0, 3], Role::Voice),
        logical(&[1], Role::Voice),
        logical(&[2], Role::Silent),
    ]);
    let m = Mixer::new(4, Some(&r), None);
    assert_eq!(muted(&m), [false, false, false, true]);
    assert_eq!(m.state().tracks[3].copy_of, Some(0));
    assert_eq!(m.state().tracks[0].copy_of, None);
    // Four copies of one mix: track 1 alone.
    let r = report(vec![logical(&[0, 1, 2, 3], Role::Mix)]);
    assert_eq!(
        muted(&Mixer::new(4, Some(&r), None)),
        [false, true, true, true]
    );
}

#[test]
fn without_roles_track_one_until_they_come() {
    let mut m = Mixer::new(3, None, None);
    assert_eq!(m.state().source, Source::Provisional);
    assert_eq!(muted(&m), [false, true, true]);
    let r = report(vec![
        logical(&[0], Role::Voice),
        logical(&[1], Role::Voice),
        logical(&[2], Role::Voice),
    ]);
    assert!(m.learn(&r), "the roles changed what is heard");
    assert_eq!(muted(&m), [false, false, false]);
    assert_eq!(m.state().source, Source::Roles);
    assert!(!m.learn(&r), "the same roles again change nothing");
    // A report naming no track of the file still leaves something heard.
    let m = Mixer::new(2, Some(&report(vec![logical(&[7], Role::Mix)])), None);
    assert_eq!(muted(&m), [false, true]);
}

/// A project in a scratch folder (the file itself is never read).
fn project(name: &str) -> (PathBuf, Project) {
    let d = scratch("mixer", name);
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let p = Project::open(&file, Some(d.join("cache")), Some(d.join("project"))).unwrap();
    (d, p)
}

fn remember(m: &mut Mixer, track: usize, mute: bool) {
    m.set_muted(track, mute).unwrap().unwrap().write().unwrap();
}

#[test]
fn the_users_mutes_are_remembered_and_beat_the_roles() {
    let (d, p) = project("remember");
    let r = report(vec![logical(&[0], Role::Mix), logical(&[1], Role::Voice)]);
    let mut m = Mixer::new(2, Some(&r), Some(p.clone()));
    remember(&mut m, 1, false);
    remember(&mut m, 0, true);
    assert!(m.set_muted(2, true).is_err(), "no third track");
    let again = Mixer::new(2, Some(&r), Some(p.clone()));
    assert_eq!(muted(&again), [true, false]);
    assert_eq!(again.state().source, Source::Remembered);
    assert_eq!(again.state().tracks[0].role, Some(Role::Mix));
    // Roles learned later do not undo them.
    let mut later = Mixer::new(2, None, Some(p.clone()));
    later.learn(&r);
    assert_eq!(muted(&later), [true, false]);
    // A file with another track count ignores them.
    assert_eq!(
        muted(&Mixer::new(3, Some(&r), Some(p.clone()))),
        [false, true, true]
    );
    // Broken: ignored.
    std::fs::write(p.dir.join(FILE), b"{").unwrap();
    assert_eq!(
        muted(&Mixer::new(2, Some(&r), Some(p.clone()))),
        [false, true]
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Opened mid-analysis, the user unmutes track 3; the roles then say three separate tracks:
/// track 2, never touched, follows them, and track 3 stays the user's.
#[test]
fn roles_move_only_the_tracks_the_user_left_alone() {
    let (d, p) = project("untouched");
    let mut m = Mixer::new(3, None, Some(p.clone()));
    assert_eq!(muted(&m), [false, true, true]);
    remember(&mut m, 2, false);
    let separate = report(vec![
        logical(&[0], Role::Voice),
        logical(&[1], Role::Voice),
        logical(&[2], Role::Voice),
    ]);
    assert!(m.learn(&separate));
    assert_eq!(muted(&m), [false, false, false]);
    // So on the next open, from the file: track 3 is the user's, the rest follow the roles.
    let with_mix = report(vec![
        logical(&[0], Role::Voice),
        logical(&[1], Role::Mix),
        logical(&[2], Role::Voice),
    ]);
    let reopened = Mixer::new(3, Some(&with_mix), Some(p.clone()));
    assert_eq!(muted(&reopened), [true, false, false]);
    assert_eq!(
        reopened
            .state()
            .tracks
            .iter()
            .map(|t| t.set)
            .collect::<Vec<_>>(),
        [false, false, true]
    );
    // A re-analysis that changes the roles moves the untouched tracks and the labels.
    let mut m = reopened;
    assert!(m.learn(&separate));
    assert_eq!(muted(&m), [false, false, false]);
    assert_eq!(m.state().tracks[1].role, Some(Role::Voice));
    // A file from before `set` was kept counts every mute as the user's.
    std::fs::write(p.dir.join(FILE), br#"{"muted":[true,false,true]}"#).unwrap();
    let old = Mixer::new(3, Some(&separate), Some(p.clone()));
    assert_eq!(muted(&old), [true, false, true]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_solo_hears_the_soloed_tracks_muted_or_not_and_is_not_remembered() {
    let (d, p) = project("solo");
    let r = report(vec![
        logical(&[0], Role::Mix),
        logical(&[1], Role::Voice),
        logical(&[2], Role::Voice),
    ]);
    let mut m = Mixer::new(3, Some(&r), Some(p.clone()));
    assert_eq!(m.weights(), "1 0 0");
    // Track 2 is muted (only the mix plays); soloing it hears it alone.
    m.set_solo(1, true).unwrap();
    assert_eq!(m.weights(), "0 1 0");
    m.set_solo(2, true).unwrap();
    assert_eq!(m.weights(), "0 1 1");
    assert_eq!(
        muted(&m),
        [false, true, true],
        "the mutes stay as they were"
    );
    assert!(m.set_solo(3, true).is_err());
    // A mute saved meanwhile carries no solo, and the next open has none.
    remember(&mut m, 0, true);
    let again = Mixer::new(3, Some(&r), Some(p));
    assert!(again.state().tracks.iter().all(|t| !t.solo));
    assert_eq!(again.weights(), "0 0 0");
    m.set_solo(1, false).unwrap();
    m.set_solo(2, false).unwrap();
    assert_eq!(
        m.weights(),
        "0 0 0",
        "back to the mutes: the mix was muted meanwhile"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn graphs_for_several_one_and_no_tracks() {
    assert_eq!(Mixer::new(0, None, None).load_options(), None);
    let one = Mixer::new(1, None, None).load_options().unwrap();
    assert!(one.contains("[aid1]aformat="), "{one}");
    assert!(one.contains("[ao]"));
    assert!(!one.contains("amerge"), "one track needs no merge: {one}");
    assert!(one.contains("amix=inputs=1:normalize=0:weights=1,"));
    let r = report(vec![logical(&[0], Role::Mix), logical(&[1], Role::Voice)]);
    let two = Mixer::new(2, Some(&r), None).load_options().unwrap();
    assert!(two.contains("[a0][a1]amerge=inputs=2[ao]"), "{two}");
    assert!(two.contains("[s1]pan=stereo|c0=c2|c1=c3[t1]"), "{two}");
    assert!(
        two.contains("amix=inputs=2:normalize=0:weights=1 0,"),
        "{two}"
    );
    assert!(two.contains("@meter:lavfi=[astats="));
    // Each graph is quoted by its byte length, so its commas do not split the options.
    let (complex, af) = two.split_once(",af=").unwrap();
    let body = |o: &str| {
        let (len, rest) = o.trim_start_matches('%').split_once('%').unwrap();
        assert_eq!(len.parse::<usize>().unwrap(), rest.len(), "{o}");
    };
    body(complex.strip_prefix("lavfi-complex=").unwrap());
    body(af);
    // More tracks than mpv can carry: the first 32 take part.
    assert_eq!(Mixer::new(40, None, None).len(), MAX_TRACKS);
}

#[test]
fn levels_take_the_louder_channel_of_each_track() {
    let json = r#"{"lavfi.astats.1.Peak_level":"-20.5","lavfi.astats.2.Peak_level":"-18.0",
            "lavfi.astats.3.Peak_level":"-inf","lavfi.astats.4.Peak_level":"-inf",
            "lavfi.astats.5.Peak_level":"-30"}"#;
    let l = levels(json, 4);
    assert_eq!(l[0], Some(-18.0));
    assert_eq!(l[1], Some(FLOOR_DB), "silence");
    assert_eq!(l[2], Some(-30.0), "one channel present");
    assert_eq!(l[3], None);
    assert_eq!(levels("not json", 2), vec![None, None]);
    assert_eq!(
        heard(r#"{"lavfi.astats.Overall.Peak_level":"-12.25"}"#),
        Some(-12.25)
    );
    assert_eq!(heard("{}"), None);
}
