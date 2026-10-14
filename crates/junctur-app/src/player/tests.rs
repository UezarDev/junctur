#[test]
fn a_word_edit_is_refused_only_when_its_own_words_changed() {
    // A piece of a running transcription landing while a word is fixed adds words
    // after it and must not refuse the fix; a re-transcription that moved the word must.
    use super::words::same_target;
    use super::Words;
    use junctur_core::transcribe::Word;
    let w = |start: f64, text: &str| Word {
        start,
        end: start + 0.3,
        text: text.into(),
        p: 1.0,
        p_min: None,
    };
    let then = Words::from_words(vec![w(1.0, " Bane"), w(2.0, " is"), w(3.0, " here")]);
    let more = Words::from_words(vec![
        w(1.0, " Bane"),
        w(2.0, " is"),
        w(3.0, " here"),
        w(301.0, " later"),
    ]);
    assert!(same_target(&more, &then, 0, 2));
    let moved = Words::from_words(vec![w(1.1, " Bane"), w(2.0, " is"), w(3.0, " here")]);
    assert!(!same_target(&moved, &then, 0, 0));
    assert!(same_target(&moved, &then, 1, 2));
    let heard = Words::from_words(vec![w(1.0, " Vayne"), w(2.0, " is")]);
    assert!(!same_target(&heard, &then, 0, 0));
    assert!(!same_target(&heard, &then, 2, 2), "gone");
    // A word starting in the same millisecond put before it changes its place there.
    let crowded = Words::from_words(vec![w(1.0, " uh"), w(1.0, " Bane"), w(2.0, " is")]);
    let first = Words::from_words(vec![w(1.0, " Bane"), w(2.0, " is")]);
    assert!(!same_target(&crowded, &first, 0, 0));
}

use super::meters::Meters;
use super::playback::{playable, speed_ok, Loads};
use super::*;

#[test]
fn plays_from_a_real_time_only() {
    assert!(playable(0.0).is_ok());
    assert!(playable(12.5).is_ok());
    for bad in [-0.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(playable(bad).is_err(), "{bad} accepted");
    }
}

#[test]
fn speeds_outside_a_quarter_to_four_times_are_refused() {
    assert!(speed_ok(1.0) && speed_ok(1.25) && speed_ok(2.0) && speed_ok(0.25) && speed_ok(4.0));
    assert!(!speed_ok(0.0) && !speed_ok(-1.0) && !speed_ok(4.5) && !speed_ok(f64::NAN));
    assert!(!speed_ok(f64::INFINITY));
}

#[test]
fn a_replaced_load_does_not_shift_later_ones() {
    let mut l = Loads::default();
    let a = l.push("f".into(), Form::Source);
    let b = l.push("f".into(), Form::Source);
    let c = l.push("f".into(), Form::Source);
    assert_eq!((a, b, c), (1, 2, 3));
    // 2 was replaced before it began, so mpv starts 3 after 1.
    assert_eq!(l.started(1).map(|x| x.entry), Some(1));
    assert_eq!(l.started(3).map(|x| x.entry), Some(3));
    assert!(l.waiting.is_empty());
    // A refused loadfile takes back its id.
    l.push("f".into(), Form::Source);
    l.unpush();
    assert_eq!(l.push("f".into(), Form::Source), 4);
    assert!(l.started(9).is_none(), "an entry the player never loaded");
}

#[test]
fn meters_fall_silent_once_when_paused_or_stalled() {
    let mut m = Meters::new();
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let json = |db: &str| Some(format!(r#"{{"lavfi.astats.1.Peak_level":"{db}"}}"#));
    assert_eq!(m.due_in(false), None, "nothing to do before playing");
    assert_eq!(m.take(json("-10"), true, 1, at(0)), Some(vec![Some(-10.0)]));
    assert_eq!(
        m.take(json("-12"), true, 1, at(50)),
        Some(vec![Some(-12.0)])
    );
    // The same reading a few times running is still live (mpv filters in bursts)...
    assert_eq!(
        m.take(json("-12"), true, 1, at(200)),
        Some(vec![Some(-12.0)])
    );
    // ...until it is old: a stalled player. Silence is sent once.
    assert_eq!(m.take(json("-12"), true, 1, at(400)), Some(vec![None]));
    assert_eq!(m.take(json("-12"), true, 1, at(450)), None);
    assert_eq!(m.take(json("-8"), true, 1, at(500)), Some(vec![Some(-8.0)]));
    // Paused: silence once, then no more ticks.
    assert!(m.due_in(false).is_some(), "one more tick for the silence");
    assert_eq!(m.take(None, false, 1, at(550)), Some(vec![None]));
    assert_eq!(m.due_in(false), None);
}
