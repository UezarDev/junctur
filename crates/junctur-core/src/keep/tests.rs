use super::*;
use crate::edit::{Range, Window};

fn judged(parts: &[(f64, f64, Kind, f64)]) -> Judged {
    Judged {
        model: laya::NAME.into(),
        prompt: "p".into(),
        version: state::VERSION,
        parts: parts
            .iter()
            .map(|&(start, end, kind, margin)| Judgement {
                start,
                end,
                kind,
                margin,
                state: format!("{start}"),
                hash: format!("{start}"),
            })
            .collect(),
    }
}

fn ranges(v: &[(f64, f64)]) -> Vec<Range> {
    v.iter().map(|&(start, end)| Range { start, end }).collect()
}

/// Silence 0-10 (Laya keeps), talk 10-20 (cuts), silence 20-30 (cuts), talk 30-40 (keeps); the
/// reduction keeps the talk.
fn sample() -> (Judged, Vec<Range>) {
    let j = judged(&[
        (0.0, 10.0, Kind::Silent, 2.0),
        (10.0, 20.0, Kind::Talk, -3.0),
        (20.0, 30.0, Kind::Silent, -1.0),
        (30.0, 40.0, Kind::Talk, 0.1),
    ]);
    (j, ranges(&[(10.0, 20.0), (30.0, 40.0)]))
}

#[test]
fn suggestions_are_where_laya_differs_from_an_untouched_list() {
    let (j, auto) = sample();
    let (items, sum) = suggestions(&j, &Fit::default(), &auto, &auto, &Choices::default());
    let at: Vec<(f64, bool)> = items.iter().map(|i| (i.start, i.keep)).collect();
    // Keep the first silence, cut the first talk; the rest Laya agrees with.
    assert_eq!(at, [(0.0, true), (10.0, false)]);
    assert_eq!((sum.keep, sum.cut, sum.accepted), (1, 1, 0));
    assert_eq!((sum.keep_s, sum.cut_s), (10.0, 10.0));
    // The last talk (P 0.52) is uncertain but agreed with, so it is no suggestion.
    assert_eq!(sum.uncertain, 0);
}

#[test]
fn a_hand_edit_wins_and_is_a_pair() {
    let (j, auto) = sample();
    // The user cut the first talk themselves and kept the second silence.
    let now = ranges(&[(20.0, 40.0)]);
    let (items, _) = suggestions(&j, &Fit::default(), &now, &auto, &Choices::default());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].start, 0.0);
    let pairs = collect(Path::new("r.mp4"), &j, &now, &auto, &Choices::default());
    let got: Vec<(f64, bool, &str)> = pairs
        .iter()
        .map(|p| (p.start, p.keep, p.how.as_str()))
        .collect();
    assert_eq!(got, [(10.0, false, "hand"), (20.0, true, "hand")]);
}

#[test]
fn accepting_dismissing_and_undoing_record_answers() {
    let (j, _) = sample();
    // The automatic cut keeps the talk with pads into the silence around it.
    let auto = ranges(&[(9.7, 20.4), (29.6, 40.0)]);
    let fit = Fit::default();
    let mut c = Choices::default();
    let (items, _) = suggestions(&j, &fit, &auto, &auto, &c);
    let edits = accept(&items, &j, &auto, &mut c, false);
    // The talk's cut takes its pads too (no sliver of silence left kept), and comes first so the
    // kept silence beside it stays whole.
    assert_eq!(edits, [(9.7, 20.4, false), (0.0, 10.0, true)]);
    // As the app leaves the list after the edits.
    let now = ranges(&[(0.0, 10.0), (29.6, 40.0)]);
    let (items, sum) = suggestions(&j, &fit, &now, &auto, &c);
    assert_eq!(sum.accepted, 2);
    assert!(items.iter().all(|i| i.state == ItemState::Accepted));
    // Accepted twice is no edit.
    assert!(accept(&items, &j, &auto, &mut c, false).is_empty());
    // Undo the cut alone: its span cut, then what the automatic cut kept of it kept again; the
    // answer is now "keep, dismissed".
    assert_eq!(
        undo(&items[1], &j, &auto, &mut c),
        [(9.7, 20.4, false), (9.7, 20.4, true)]
    );
    let a = c.answer(&items[1].part_of()).unwrap();
    assert_eq!((a.keep, a.how), (true, How::Dismissed));
    // Undo of the kept silence: the part cut, and the talk's pad that reached into it kept again.
    let mut again = c.clone();
    assert_eq!(
        undo(&items[0], &j, &auto, &mut again),
        [(0.0, 10.0, false), (9.7, 10.0, true)]
    );
    // A dismissed part is no longer listed, and undo of a waiting one is nothing.
    let (items, _) = suggestions(&j, &fit, &now, &auto, &c);
    assert_eq!(items.len(), 1);
    let waiting = Item {
        state: ItemState::Suggested,
        ..items[0]
    };
    assert!(undo(&waiting, &j, &auto, &mut c).is_empty());
    // Every answer is a pair.
    let pairs = collect(Path::new("r.mp4"), &j, &now, &auto, &c);
    assert_eq!(pairs.len(), 2);
}

#[test]
fn a_cut_of_talk_never_reaches_into_the_talk_beside_it() {
    // A long talk stretch cut in two parts: Laya cuts the first only.
    let j = judged(&[
        (0.0, 10.0, Kind::Silent, -2.0),
        (10.0, 30.0, Kind::Talk, -2.0),
        (30.0, 50.0, Kind::Talk, 2.0),
    ]);
    let auto = ranges(&[(9.7, 50.0)]);
    let mut c = Choices::default();
    let (items, _) = suggestions(&j, &Fit::default(), &auto, &auto, &c);
    assert_eq!(
        accept(&items, &j, &auto, &mut c, false),
        [(9.7, 30.0, false)]
    );
}

impl Item {
    fn part_of(&self) -> Part {
        Part {
            start: self.start,
            end: self.end,
            kind: self.kind,
        }
    }
}

#[test]
fn an_accepted_part_keeps_what_was_taken_after_a_refit() {
    let (j, auto) = sample();
    let mut c = Choices::default();
    let (items, _) = suggestions(&j, &Fit::default(), &auto, &auto, &c);
    accept(&items[..1], &j, &auto, &mut c, false);
    // A fit that would now cut the first silence.
    let strict = Fit {
        temperature: 1.0,
        cutoff: 0.95,
        pairs: 30,
    };
    let (items, _) = suggestions(
        &j,
        &strict,
        &ranges(&[(0.0, 20.0), (30.0, 40.0)]),
        &auto,
        &c,
    );
    let first = items.iter().find(|i| i.start == 0.0).unwrap();
    assert!(first.keep && first.state == ItemState::Accepted);
}

#[test]
fn pairs_are_replaced_per_recording_and_fit_across_them() {
    let dir = crate::test_support::scratch("keep", "pairs");
    let path = dir.join("keep_pairs.json");
    let pair = |rec: &str, margin: f64, keep: bool| Pair {
        recording: rec.into(),
        start: margin,
        end: margin + 1.0,
        kind: Kind::Talk,
        margin,
        keep,
        how: "hand".into(),
        state: String::new(),
    };
    store_pairs(&path, Path::new("a"), vec![pair("a", 1.0, true)]).unwrap();
    store_pairs(&path, Path::new("b"), vec![pair("b", -1.0, false)]).unwrap();
    let all = store_pairs(&path, Path::new("a"), vec![pair("a", 2.0, true)]).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(load_pairs(&path), all);
    assert_eq!(fitted(Some(&path)), Fit::default(), "two pairs fit nothing");
    let many: Vec<Pair> = (0..40)
        .map(|i| {
            pair(
                "c",
                i as f64 - 20.0 + 3.0,
                i as f64 - 20.0 + if i % 5 == 0 { 2.0 } else { 0.0 } > 0.0,
            )
        })
        .collect();
    store_pairs(&path, Path::new("c"), many).unwrap();
    let f = fitted(Some(&path));
    assert_eq!(f.pairs, 42);
    assert!(f.cutoff > 0.5, "Laya keeps too much here: {f:?}");
    // A broken file reads as no pairs.
    std::fs::write(&path, "{").unwrap();
    assert!(load_pairs(&path).is_empty());
}

#[test]
fn windows_take_the_parts_chance_weighted_by_cover() {
    let (j, _) = sample();
    let w = |start: f64| Window {
        start,
        score: 0.7,
        speech: 0.7,
        words: 0,
    };
    let mut windows = vec![w(0.0), w(5.0), w(7.5), w(40.0)];
    score_windows(&mut windows, 5.0, &j, &Fit::default());
    let p = |m: f64| (laya::p_keep(m, 1.0) * 1000.0).round() / 1000.0;
    assert_eq!(windows[0].score, p(2.0));
    // 7.5 to 12.5: half the kept silence, half the cut talk.
    let mixed = (laya::p_keep(2.0, 1.0) + laya::p_keep(-3.0, 1.0)) / 2.0;
    assert_eq!(windows[2].score, (mixed * 1000.0).round() / 1000.0);
    // Past every part: left as it was, and the speech share untouched everywhere.
    assert_eq!(windows[3].score, 0.7);
    assert!(windows.iter().all(|w| w.speech == 0.7));
}

#[test]
fn the_recordings_prompt_comes_first_then_the_last_one_when_on() {
    let own = Choices {
        prompt: Some(" mine ".into()),
        answers: vec![],
    };
    let on = Settings {
        on: true,
        last: Some("last".into()),
    };
    let off = Settings {
        on: false,
        ..on.clone()
    };
    assert_eq!(prompt_for(&own, &off).as_deref(), Some("mine"));
    assert_eq!(
        prompt_for(&Choices::default(), &on).as_deref(),
        Some("last")
    );
    assert_eq!(prompt_for(&Choices::default(), &off), None);
    let blank = Choices {
        prompt: Some("  ".into()),
        answers: vec![],
    };
    assert_eq!(prompt_for(&blank, &off), None);
}

#[test]
fn choices_read_back_and_a_broken_file_is_an_error() {
    let dir = crate::test_support::scratch("keep", "choices");
    assert_eq!(load_choices(&dir).unwrap(), Choices::default());
    let mut c = Choices {
        prompt: Some("x".into()),
        answers: vec![],
    };
    c.set(Answer {
        start: 1.0,
        end: 2.0,
        keep: true,
        how: How::Accepted,
        bulk: true,
    });
    c.set(Answer {
        start: 1.0,
        end: 2.0,
        keep: false,
        how: How::Dismissed,
        bulk: false,
    });
    assert_eq!(c.answers.len(), 1);
    save_choices(&dir, &c).unwrap();
    assert_eq!(load_choices(&dir).unwrap(), c);
    std::fs::write(dir.join(CHOICES), "nope").unwrap();
    assert!(load_choices(&dir).is_err());
}

#[test]
fn agreement_counts_the_held_out_half_and_the_hand_edits() {
    let (j, auto) = sample();
    let now = ranges(&[(20.0, 40.0)]);
    let a = agreement(&j, &now, &auto);
    assert_eq!((a.all.parts, a.all.kept), (4, 2));
    assert_eq!(a.all.always_keep, 0.5);
    // Laya keeps 0-10 and 30-40, cuts 10-20 and 20-30; the list keeps 20-40.
    assert_eq!(a.all.laya, 0.5);
    assert_eq!(a.all.held_out, 2);
    assert_eq!((a.hand, a.hand_agree), (2, 1));
    assert_eq!((a.talk.parts, a.silent.parts), (2, 2));
}

#[test]
fn an_accept_taken_back_by_the_editors_undo_no_longer_stands() {
    let (j, auto) = sample();
    let fit = Fit::default();
    let mut c = Choices::default();
    let (items, _) = suggestions(&j, &fit, &auto, &auto, &c);
    accept(&items, &j, &auto, &mut c, false);
    // Ctrl+Z put the list back as it was: both are suggestions again, and no pair is counted.
    let (items, sum) = suggestions(&j, &fit, &auto, &auto, &c);
    assert_eq!((sum.accepted, sum.keep, sum.cut), (0, 1, 1));
    assert!(items.iter().all(|i| i.state == ItemState::Suggested));
    assert!(collect(Path::new("r.mp4"), &j, &auto, &auto, &c).is_empty());
    // A dismissed part the user then cut by hand: their hand edit, not the stale "keep".
    let mut d = Choices::default();
    dismiss(&items[1], &mut d);
    let now = ranges(&[(30.0, 40.0)]);
    let pairs = collect(Path::new("r.mp4"), &j, &now, &auto, &d);
    let got: Vec<(f64, bool, &str)> = pairs
        .iter()
        .map(|p| (p.start, p.keep, p.how.as_str()))
        .collect();
    assert_eq!(got, [(10.0, false, "hand")]);
}

#[test]
fn accept_all_is_no_training_pair_but_one_part_accepted_or_undone_is() {
    let (j, auto) = sample();
    let fit = Fit::default();
    let mut c = Choices::default();
    let (items, _) = suggestions(&j, &fit, &auto, &auto, &c);
    accept(&items, &j, &auto, &mut c, true);
    let now = ranges(&[(0.0, 10.0), (30.0, 40.0)]);
    // Taken, shown as accepted, but not a pair: Laya's choice is not the user's decision.
    let (items, sum) = suggestions(&j, &fit, &now, &auto, &c);
    assert_eq!(sum.accepted, 2);
    assert!(collect(Path::new("r.mp4"), &j, &now, &auto, &c).is_empty());
    // Undoing one of them is a decision about that part: a pair.
    undo(&items[1], &j, &auto, &mut c);
    let now = ranges(&[(0.0, 10.0), (10.0, 20.0), (30.0, 40.0)]);
    let pairs = collect(Path::new("r.mp4"), &j, &now, &auto, &c);
    let got: Vec<(f64, bool, &str)> = pairs
        .iter()
        .map(|p| (p.start, p.keep, p.how.as_str()))
        .collect();
    assert_eq!(got, [(10.0, true, "dismissed")]);
    // One part accepted on its own is a pair.
    let mut one = Choices::default();
    let (items, _) = suggestions(&j, &fit, &auto, &auto, &one);
    accept(&items[..1], &j, &auto, &mut one, false);
    let now = ranges(&[(0.0, 20.0), (30.0, 40.0)]);
    let pairs = collect(Path::new("r.mp4"), &j, &now, &auto, &one);
    assert_eq!(pairs.len(), 1);
    assert_eq!((pairs[0].start, pairs[0].how.as_str()), (0.0, "accepted"));
}
