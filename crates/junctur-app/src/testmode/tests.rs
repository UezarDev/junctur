use super::*;

#[test]
fn playing_needs_frames_and_a_moving_position() {
    assert_eq!(verdict(false, 0, None, None), "idle");
    assert_eq!(verdict(true, 120, Some(1.0), Some(3.0)), "playing");
    // Redraws of a still frame, or a position that never moved, are not playback.
    assert_eq!(verdict(true, 0, Some(1.0), Some(3.0)), "stalled");
    assert_eq!(verdict(true, 120, Some(2.0), Some(2.0)), "stalled");
    assert_eq!(verdict(true, 120, None, Some(2.0)), "stalled");
}

#[test]
fn parses_positive_seconds_only() {
    assert_eq!(parse(Some("8")), Some(8.0));
    assert_eq!(parse(Some(" 2.5 ")), Some(2.5));
    assert_eq!(parse(Some("0")), None);
    assert_eq!(parse(Some("-1")), None);
    assert_eq!(parse(Some("yes")), None);
    assert_eq!(parse(None), None);
}

#[test]
fn parses_steps_in_time_order() {
    let s = parse_steps("4:skip=off; 2:seek=130.5 ;6:skip=on;7:pause;8:kseek=1").unwrap();
    assert_eq!(
        s,
        vec![
            (2.0, Action::Seek(130.5)),
            (4.0, Action::Skip(false)),
            (6.0, Action::Skip(true)),
            (7.0, Action::Pause),
            (8.0, Action::KeySeek(1.0)),
        ]
    );
    let s = parse_steps("9:reanalyse;10:reanalyse_fresh").unwrap();
    assert_eq!(
        s,
        vec![
            (9.0, Action::Reanalyse(false)),
            (10.0, Action::Reanalyse(true))
        ]
    );
    assert_eq!(
        parse_steps("3:keepall").unwrap(),
        vec![(3.0, Action::KeepAll)]
    );
    let s = parse_steps("1:cutw=3-5;2:keepf=30-60;3:undo;4:redo").unwrap();
    assert_eq!(
        s.iter().map(|x| x.1.clone()).collect::<Vec<_>>(),
        vec![
            Action::Edit(Step::Do(Op::Words {
                first: 3,
                last: 5,
                keep: false
            })),
            Action::Edit(Step::Do(Op::Frames {
                start: 30,
                end: 60,
                keep: true
            })),
            Action::Edit(Step::Undo),
            Action::Edit(Step::Redo),
        ]
    );
    assert_eq!(
        parse_steps("5:rename=2:Ana María").unwrap(),
        vec![(5.0, Action::Rename(2, "Ana María".into()))]
    );
    assert_eq!(
        parse_steps("1:color=2:#FF0000;2:captions=on;3:srt=C:\\x\\out.srt;4:preview=on").unwrap(),
        vec![
            (1.0, Action::Recolor(2, "#FF0000".into())),
            (2.0, Action::Captions(true)),
            (3.0, Action::Srt("C:\\x\\out.srt".into())),
            (4.0, Action::Preview(true)),
        ]
    );
    assert_eq!(
        parse_steps("1:enroll=10-40:Ana María;2:forget=Ana María").unwrap(),
        vec![
            (1.0, Action::Enroll(10, 40, "Ana María".into())),
            (2.0, Action::Forget("Ana María".into())),
        ]
    );
    assert_eq!(
        parse_steps("1:mute=2;2:unmute=1").unwrap(),
        vec![(1.0, Action::Mute(1, true)), (2.0, Action::Mute(0, false))]
    );
    assert!(parse_steps("1:mute=0").is_err(), "tracks count from 1");
    assert_eq!(
        parse_steps("1:solo=2;2:unsolo=2").unwrap(),
        vec![(1.0, Action::Solo(1, true)), (2.0, Action::Solo(1, false))]
    );
    assert!(parse_steps("1:solo=0").is_err());
    assert_eq!(
        parse_steps("3:menu=talking").unwrap(),
        vec![(3.0, Action::Menu("talking".into()))]
    );
    assert_eq!(
        parse_steps("1:speed=1.5;2:loop=10.5-12;3:unloop").unwrap(),
        vec![
            (1.0, Action::Speed(1.5)),
            (2.0, Action::Loop(10.5, 12.0)),
            (3.0, Action::Unloop)
        ]
    );
    assert_eq!(
        parse_steps("4:reload").unwrap(),
        vec![(4.0, Action::Reload)]
    );
    assert!(parse_steps("4:reload=now").is_err());
    assert!(parse_steps("1:loop=10").is_err());
    assert!(parse_steps("1:speed=fast").is_err());
    assert!(parse_steps("1:mute=x").is_err());
    assert!(parse_steps("1:enroll=10:Ana").is_err());
    assert!(parse_steps("1:forget=").is_err());
    assert!(parse_steps("5:captions=maybe").is_err());
    assert!(parse_steps("5:rename=x:Ana").is_err());
    assert!(parse_steps("5:rename=2").is_err());
    assert!(parse_steps("1:cutw=3").is_err());
    assert!(parse_steps("2:jump=3").is_err());
    assert!(parse_steps("x:pause").is_err());
    assert_eq!(parse_steps("").unwrap(), vec![]);
}
