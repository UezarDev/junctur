use super::*;
use crate::keep::PRESETS;
use crate::labels::{Kind as LabelKind, Labelled, Share};

fn r(start: f64, end: f64) -> Region {
    Region { start, end }
}

/// Peaks at `waves::RATE` a second: byte 20 everywhere, `loud` (start, end, byte) louder.
fn peaks(seconds: f64, loud: &[(f64, f64, u8)]) -> Vec<u8> {
    let rate = crate::waves::RATE as f64;
    let mut v = vec![20u8; (seconds * rate) as usize];
    for &(a, b, byte) in loud {
        v[(a * rate) as usize..(b * rate) as usize].fill(byte);
    }
    v
}

fn input<'a>(prompt: &'a str, speech: &'a [Region], d: f64, p: &'a [u8]) -> Input<'a> {
    Input {
        prompt,
        speech,
        duration: d,
        labels: None,
        peaks: Some(p),
    }
}

fn spans_of(j: &Judged) -> Vec<(f64, f64)> {
    j.parts.iter().map(|p| (p.start, p.end)).collect()
}

#[test]
fn the_presets_read_as_the_kind_of_video_they_describe() {
    let read: Vec<Strictness> = PRESETS.iter().map(|p| Strictness::read(p.1)).collect();
    assert_eq!(
        read,
        [Strictness::Strict, Strictness::Loose, Strictness::TalkOnly]
    );
}

#[test]
fn the_prompt_is_read_in_english_and_spanish_first_level_first() {
    use Strictness::*;
    for (prompt, level) in [
        ("", Usual),
        ("League with friends", Usual),
        ("Only the fights!", Strict),
        ("Solo las peleas, por favor", Strict),
        ("los MEJORES momentos", Strict),
        ("Todo el directo, sin tiempo muerto", Loose),
        ("Pódcast de la charla", TalkOnly),
        ("podcast highlights", TalkOnly),
        ("highlights of the whole stream", Strict),
        // Whole words: "vodka" is not "vod", "shortcut" not "short video".
        ("a shortcut to vodka", Usual),
        // Plurals and singulars read as their phrase (3e).
        ("podcasts with friends", TalkOnly),
        ("two interviews", TalkOnly),
        ("Entrevistas", TalkOnly),
        ("full streams", Loose),
        ("montages", Strict),
        ("the best moment", Strict),
    ] {
        assert_eq!(Strictness::read(prompt), level, "{prompt:?}");
    }
    assert_eq!(fold("¡Qué PELEA!  ya"), " que pelea ya ");
}

#[test]
fn a_long_silence_with_a_busy_window_is_kept_whole_and_a_quiet_one_is_not() {
    // Talk at 0-10, 130-140 and 200-210; the 120 s silence between the first two is loud from 60
    // to 70 only, the other two silences are quiet.
    let speech = [r(0.0, 10.0), r(130.0, 140.0), r(200.0, 210.0)];
    let p = peaks(260.0, &[(60.0, 70.0, 200)]);
    let j = judge(&input("highlights", &speech, 260.0, &p));
    assert_eq!((j.model.as_str(), j.version), (NAME, VERSION));
    // The two loud windows count (strict: 10% of 46 windows, the rest tied at 0), and the
    // stretch that holds them is kept in full, not padded windows.
    assert_eq!(spans_of(&j), [(10.0, 130.0)]);
    assert!(j
        .parts
        .iter()
        .all(|p| p.kind == Kind::Silent && p.margin >= 0.0));
    assert!(j.parts[0].state.contains("game audio busier than"));
    assert_eq!(
        summary(&j),
        "keep: talk as the cut leaves it; silence read as strict, every silent stretch holding the busiest 10% of the silent windows: 1 busy silent stretches to keep, whole (120 s)"
    );
    // Talk only keeps nothing.
    let none = judge(&input("podcast", &speech, 260.0, &p));
    assert!(none.parts.is_empty());
    assert!(summary(&none)
        .ends_with("talk only, no silent part kept: 0 busy silent stretches to keep, whole (0 s)"));
}

#[test]
fn the_line_is_the_recordings_own_whatever_the_volume() {
    // The same shape at two game volumes keeps the same windows.
    let speech = [r(0.0, 5.0)];
    let quiet = peaks(105.0, &[(40.0, 50.0, 60)]);
    let loud: Vec<u8> = quiet.iter().map(|b| b.saturating_add(150)).collect();
    let a = judge(&input("", &speech, 105.0, &quiet));
    let b = judge(&input("", &speech, 105.0, &loud));
    assert!(!a.parts.is_empty());
    assert_eq!(spans_of(&a), spans_of(&b));
}

#[test]
fn stretches_either_side_of_a_sentence_are_kept_apart() {
    // Two silences split by a sentence at 30-32; loud at both its edges.
    let speech = [r(30.0, 32.0)];
    let p = peaks(62.0, &[(27.0, 30.0, 200), (32.0, 35.0, 200)]);
    let j = judge(&input("", &speech, 62.0, &p));
    assert_eq!(spans_of(&j), [(0.0, 30.0), (32.0, 62.0)]);
}

#[test]
fn fight_labels_keep_a_quiet_window_and_nothing_is_kept_without_any_signal() {
    let speech = [r(0.0, 2.0)];
    let piece = |start: f64, end: f64, fight: f64| Labelled {
        start,
        end,
        frames: 5,
        top: vec![Share {
            label: if fight > 0.0 {
                "teamfight"
            } else {
                "walking on the map"
            }
            .into(),
            score: fight.max(0.5),
        }],
        busy: None,
        level_db: None,
    };
    let labels = Labels {
        model: "m".into(),
        kind: LabelKind::League,
        labels: vec![],
        prompts: vec![],
        busy_track: None,
        pieces: vec![
            piece(2.0, 32.0, 0.0),
            piece(32.0, 62.0, 0.9),
            piece(62.0, 92.0, 0.0),
            piece(92.0, 122.0, 0.0),
        ],
    };
    let flat = peaks(122.0, &[]);
    let mut i = input("", &speech, 122.0, &flat);
    i.labels = Some(&labels);
    let j = judge(&i);
    // The fight piece's 6 windows are the top quarter of 24 (all even on busy): their stretch.
    assert_eq!(spans_of(&j), [(2.0, 122.0)]);
    // The labels kept it, so they are said first; the game audio is known, so it is said too.
    assert_eq!(
        j.parts[0].state,
        "fight labels 90%, more than 75% of the silence; game audio busier than 0% of the silence"
    );
    // Busy unknown (no peaks, none in the labels): not said at all (3e).
    let unheard = Input { peaks: None, ..i };
    assert_eq!(
        judge(&unheard).parts[0].state,
        "fight labels 90%, more than 75% of the silence"
    );
    // No peaks, no labels: nothing to tell windows apart, nothing kept.
    let none = Input {
        labels: None,
        peaks: None,
        ..i
    };
    assert!(judge(&none).parts.is_empty());
    // No silence at all: nothing.
    assert!(judge(&input("", &[r(0.0, 50.0)], 50.0, &flat))
        .parts
        .is_empty());
}

#[test]
fn without_peaks_each_piece_busy_stands_for_its_windows() {
    let labels = Labels {
        model: "m".into(),
        kind: LabelKind::Generic,
        labels: vec![],
        prompts: vec![],
        busy_track: None,
        pieces: (0..4)
            .map(|k| Labelled {
                start: k as f64 * 25.0,
                end: (k + 1) as f64 * 25.0,
                frames: 0,
                top: vec![],
                busy: Some(if k == 2 { 0.95 } else { 0.2 }),
                level_db: None,
            })
            .collect(),
    };
    let i = Input {
        prompt: "",
        speech: &[],
        duration: 100.0,
        labels: Some(&labels),
        peaks: None,
    };
    assert_eq!(spans_of(&judge(&i)), [(0.0, 100.0)]);
}

#[test]
fn a_stretch_well_above_the_best_window_left_out_is_sure_and_a_close_call_is_not() {
    use crate::keep::Fit;
    let speech = [r(0.0, 5.0)];
    // A ramp: each later window louder; strict keeps the top tenth.
    let loud: Vec<(f64, f64, u8)> = (0..20)
        .map(|k| {
            (
                5.0 + k as f64 * 5.0,
                10.0 + k as f64 * 5.0,
                30 + k as u8 * 10,
            )
        })
        .collect();
    let p = peaks(105.0, &loud);
    let j = judge(&input("highlights", &speech, 105.0, &p));
    assert_eq!(spans_of(&j), [(5.0, 105.0)]);
    let f = Fit::default();
    assert!(f.keeps(j.parts[0].margin) && !f.uncertain(j.parts[0].margin));
    // Its best window ranks 0.95, the best left out 0.85: two thirds of the way to the top.
    assert!(
        (j.parts[0].margin - TOP_MARGIN * 2.0 / 3.0).abs() < 1e-3,
        "{:?}",
        j.parts
    );
    // 41 silences of 4 s, one window each, each louder than the one before: the usual quarter
    // keeps the top 10; the lowest of them is one step above the best left out, a close call.
    let speech: Vec<Region> = (0..40)
        .map(|k| r(4.0 + 6.0 * k as f64, 6.0 + 6.0 * k as f64))
        .collect();
    let loud: Vec<(f64, f64, u8)> = (0..41)
        .map(|k| (6.0 * k as f64, 6.0 * k as f64 + 4.0, 20 + 5 * k as u8))
        .collect();
    let p = peaks(244.0, &loud);
    let j = judge(&input("", &speech, 244.0, &p));
    assert_eq!(j.parts.len(), 10);
    assert_eq!(spans_of(&j)[0], (186.0, 190.0));
    assert!(f.uncertain(j.parts[0].margin), "{:?}", j.parts[0]);
    assert!(!f.uncertain(j.parts[9].margin), "{:?}", j.parts[9]);
}

#[test]
fn windows_keep_their_speech_share_and_lean_toward_a_kept_stretch() {
    let j = Judged {
        model: NAME.into(),
        prompt: String::new(),
        version: VERSION,
        parts: vec![Judgement {
            start: 10.0,
            end: 15.0,
            kind: Kind::Silent,
            margin: 4.0,
            state: String::new(),
            hash: String::new(),
        }],
    };
    let w = |start: f64, speech: f64| Window {
        start,
        score: 0.33,
        speech,
        words: 0,
    };
    let mut windows = vec![w(0.0, 1.0), w(5.0, 0.0), w(10.0, 0.0), w(12.5, 0.2)];
    score_windows(&mut windows, 5.0, &j);
    let p = crate::keep::fit::p_keep(4.0, 1.0);
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    assert_eq!(windows[0].score, 1.0);
    assert_eq!(windows[1].score, 0.0);
    assert_eq!(windows[2].score, r3(p));
    assert_eq!(windows[3].score, r3((2.5 * p + 2.5 * 0.2) / 5.0));
}

#[test]
fn a_silence_whose_pieces_round_apart_is_still_one_stretch() {
    // A 70.7 s gap from 0.2 s: its second piece's end, `start + step`, falls an ulp short of the
    // third's start, `a + 2 * step`, so joining the pieces back split it (3e).
    let speech = [r(0.0, 0.2), r(70.9, 72.9)];
    let cut = pieces::pieces(&speech, 80.0);
    assert!(cut[1].end < cut[2].start, "{cut:?}");
    // Loud in the last piece only: the whole silence is kept, from its start.
    let p = peaks(80.0, &[(60.0, 65.0, 200)]);
    let j = judge(&input("highlights", &speech, 80.0, &p));
    assert_eq!(spans_of(&j), [(0.2, 70.9)]);
}

#[test]
fn ties_at_the_line_are_broken_so_the_share_holds() {
    // 41 silences of 4 s, one window each, every other one equally loud (21 tied): the usual
    // quarter is 10 windows, so 10 stretches, the earliest, not all 21.
    let speech: Vec<Region> = (0..40)
        .map(|k| r(4.0 + 6.0 * k as f64, 6.0 + 6.0 * k as f64))
        .collect();
    let loud: Vec<(f64, f64, u8)> = (0..41)
        .filter(|k| k % 2 == 0)
        .map(|k| (6.0 * k as f64, 6.0 * k as f64 + 4.0, 200))
        .collect();
    let p = peaks(244.0, &loud);
    let j = judge(&input("", &speech, 244.0, &p));
    assert_eq!(j.parts.len(), 10);
    assert_eq!(spans_of(&j)[9], (108.0, 112.0));
    // The tie went on past the line: a close call.
    let f = crate::keep::Fit::default();
    assert!(
        j.parts.iter().all(|p| f.uncertain(p.margin)),
        "{:?}",
        j.parts
    );
}

#[test]
fn a_tie_inside_a_kept_stretch_is_no_close_call() {
    // 0-20 s loud throughout (4 tied windows), then 17 quiet silences of 4 s: strict keeps 2 of 21
    // windows; the other two loud ones are in the same stretch, so it is clear, not a close call.
    let mut speech = vec![r(20.0, 22.0)];
    speech.extend((0..16).map(|k| r(26.0 + 6.0 * k as f64, 28.0 + 6.0 * k as f64)));
    let p = peaks(122.0, &[(0.0, 20.0, 200)]);
    let j = judge(&input("highlights", &speech, 122.0, &p));
    assert_eq!(spans_of(&j), [(0.0, 20.0)]);
    let f = crate::keep::Fit::default();
    assert!(!f.uncertain(j.parts[0].margin), "{:?}", j.parts);
}
