use super::*;

fn word(start: f64, end: f64, text: &str) -> Word {
    Word {
        start,
        end,
        text: text.into(),
        p: 1.0,
        p_min: None,
    }
}

#[test]
fn words_so_far_only_grow_at_the_end() {
    let dir = std::env::temp_dir().join(format!("junctur-sofar-words-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let seg = |track: usize, start: f64, words: &[(f64, &str)]| {
        let words: Vec<String> = words
            .iter()
            .map(|(s, t)| serde_json::to_string(&word(*s, s + 0.4, t)).unwrap())
            .collect();
        format!(
            r#"{{"track":{track},"start":{start},"end":{},"lang":"es","model":"m","words":[{}]}}"#,
            start + 20.0,
            words.join(",")
        )
    };
    let header = r#"{"version":2,"languages":"es+en","model":"m","device":"cpu"}"#;
    // Piece 0 on both tracks, and track 0 already into piece 1 (its chunk at 302 s).
    let lines = [
        header.to_string(),
        seg(0, 1.0, &[(1.0, "uno"), (290.0, "dos")]),
        seg(0, 302.0, &[(302.0, "cinco")]),
        seg(1, 280.0, &[(285.0, "tres"), (301.0, "cuatro")]),
    ];
    let path = dir.join("transcript.partial.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    // Whole up to 302 s (track 1's next chunk would start there): 302's own word waits.
    let early = Words::load_so_far(&dir, 302.0);
    let text = |w: &Words| w.list.iter().map(|w| w.text.clone()).collect::<Vec<_>>();
    assert_eq!(text(&early), ["uno", "tres", "dos", "cuatro"]);
    // The last word's span stops where the transcript does.
    let s = CutSettings::default();
    let (_, b) = early
        .span(3, 3, false, &placing(&Envelope::default(), &s, 1000.0, &[]))
        .unwrap();
    assert!(b <= 302.0, "{b}");
    // Track 1 reaches its next piece: the earlier words keep their indices.
    std::fs::write(
        &path,
        [lines.join("\n"), seg(1, 320.0, &[(320.0, "seis")])].join("\n") + "\n",
    )
    .unwrap();
    let later = Words::load_so_far(&dir, 600.0);
    assert_eq!(text(&later)[..4], text(&early)[..]);
    assert_eq!(text(&later)[4..], ["cinco", "seis"]);
    // No chunks of this run yet (it waits for the lane): no words, even beside an earlier
    // run's finished transcript, which the new words would replace under the same indices.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(
        dir.join("transcript.json"),
        r#"{"choice":{"model":"m","device":"cpu","note":null},"languages":["es"],
            "segments":[{"track":0,"start":0,"end":1,"lang":"es","model":"m",
            "words":[{"start":0.1,"end":0.5,"text":"viejo","p":1.0}]}],"audio_s":1,"elapsed_s":1}"#,
    )
    .unwrap();
    assert_eq!(Words::load(&dir).unwrap().list.len(), 1);
    assert!(Words::load_so_far(&dir, 0.0).list.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

fn sample() -> Words {
    // Out of order, as two tracks' segments come.
    Words::from_words(vec![
        word(12.0, 12.5, "tres"),
        word(10.0, 10.4, "uno"),
        word(10.5, 11.0, "dos"),
    ])
}

#[test]
fn sorted_and_compact() {
    let v = TranscriptView::new(&sample(), &Speakers::default(), &[], &[]);
    assert_eq!(v.text, vec!["uno", "dos", "tres"]);
    assert_eq!(v.start, vec![10_000, 10_500, 12_000]);
    assert_eq!(v.end, vec![10_400, 11_000, 12_500]);
    assert_eq!(v.speaker, vec![0, 0, 0]);
    // A fixed word shows as fixed.
    let fix = Fix {
        track: 0,
        start: 10.5,
        nth: 0,
        through: None,
        text: "Uezar".into(),
    };
    let v = TranscriptView::new(&sample(), &Speakers::default(), &[fix], &[]);
    assert_eq!(v.text, vec!["uno", "Uezar", "tres"]);
}

/// Each word's confidence in percent, none for a word without a probability or one the user
/// fixed or confirmed.
#[test]
fn a_word_is_sure_as_its_weakest_piece_until_the_user_checks_it() {
    let sure = |p: Option<f32>, start: f64, text: &str| Word {
        p_min: p,
        ..word(start, start + 0.4, text)
    };
    let w = Words::from_words(vec![
        sure(Some(0.399), 10.0, "uno"),
        sure(Some(1.0), 10.5, "Wezar"),
        sure(None, 11.0, "viejo"),
        sure(Some(0.05), 11.5, "Tsunejo"),
        sure(Some(0.2), 12.0, "bien"),
    ]);
    let none = Speakers::default();
    let v = TranscriptView::new(&w, &none, &[], &[]);
    assert_eq!(v.sure, vec![39, 100, NOT_FLAGGED, 5, 20]);
    let fix = |start: f64, text: &str| Fix {
        track: 0,
        start,
        nth: 0,
        through: None,
        text: text.into(),
    };
    // "Tsunejo" fixed, "bien" confirmed (its own text): neither is flagged any more.
    let v = TranscriptView::new(&w, &none, &[fix(11.5, "Tsunheto"), fix(12.0, "bien")], &[]);
    assert_eq!(v.sure, vec![39, 100, NOT_FLAGGED, NOT_FLAGGED, NOT_FLAGGED]);
    assert_eq!(v.text[3..], ["Tsunheto", "bien"]);
    // A span retyped over "uno" to "viejo" vouches for every word in it; the words after it
    // are still read once each, in order, and flagged as before.
    let span = Fix {
        through: Some(junctur_core::edit::WordAt {
            start: 11.0,
            nth: 0,
        }),
        ..fix(10.0, "one old")
    };
    let v = TranscriptView::new(&w, &none, &[span], &[]);
    assert_eq!(v.sure, vec![NOT_FLAGGED, NOT_FLAGGED, NOT_FLAGGED, 5, 20]);
    assert_eq!(v.span, vec![1, 1, 1, 0, 0]);
    assert_eq!(v.spans, ["one old"]);
}

/// A probability of exactly k % reads as k, so it is not under a threshold of k.
#[test]
fn a_whole_percentage_is_not_read_one_lower() {
    for k in 0..=100u8 {
        assert_eq!(percent(k as f32 / 100.0), k, "{k} %");
    }
    assert_eq!(percent(0.35), 35);
    assert_eq!(percent(0.3499), 34);
    assert_eq!(percent(0.2499), 24);
    assert_eq!((percent(-0.1), percent(1.5)), (0, 100));
}

#[test]
fn a_fix_shows_on_its_own_word_of_those_starting_together() {
    // "en" and "él" start at one millisecond; a fix on "él" must not show on "en". Another
    // track's word at that moment is counted on its own track.
    let w = Words::from_tracked(vec![
        (0, word(54.174, 54.944, "manada")),
        (0, word(54.944, 54.944, "en")),
        (1, word(54.944, 55.2, "yes")),
        (0, word(54.9441, 54.944, "él")),
        (0, word(55.5, 55.8, "y")),
    ]);
    assert_eq!(
        (0..5).map(|i| w.nth(i)).collect::<Vec<_>>(),
        [0, 0, 0, 1, 0]
    );
    let fix = Fix {
        track: 0,
        start: 54.944,
        nth: w.nth(3),
        through: None,
        text: "miel".into(),
    };
    let v = TranscriptView::new(&w, &Speakers::default(), &[fix], &[]);
    assert_eq!(v.text, ["manada", "en", "yes", "miel", "y"]);
}

/// Confirming one of the words starting in one millisecond unflags that word alone, and the
/// words after it read their own text.
#[test]
fn a_confirmed_word_of_those_starting_together_is_the_only_one_unflagged() {
    let at = |start: f64, p: Option<f32>, text: &str| Word {
        p_min: p,
        ..word(start, start + 0.2, text)
    };
    let w = Words::from_words(vec![
        at(54.944, None, "en"),
        at(54.944, Some(0.2), "él"),
        at(54.944, Some(0.3), "la"),
        at(55.5, Some(0.1), "y"),
    ]);
    let confirm = Fix {
        track: 0,
        start: 54.944,
        nth: w.nth(1),
        through: None,
        text: "él".into(),
    };
    let v = TranscriptView::new(&w, &Speakers::default(), &[confirm], &[]);
    assert_eq!(v.sure, vec![NOT_FLAGGED, NOT_FLAGGED, 30, 10]);
    assert_eq!(v.text, ["en", "él", "la", "y"]);
}

#[test]
fn a_span_shows_as_its_words_heard_and_its_text_once() {
    // "the wezar went" retyped as one text; someone's "yeah" on track 1 between its words is
    // not in it.
    let w = Words::from_tracked(vec![
        (0, word(1.0, 1.4, "so")),
        (0, word(2.0, 2.2, "the")),
        (0, word(2.2, 2.6, "wezar")),
        (1, word(2.4, 2.6, "yeah")),
        (0, word(3.0, 3.4, "went")),
        (0, word(4.0, 4.4, "ok")),
    ]);
    let r = w.retyped(4, 1, "x", &Speakers::default(), &[]).unwrap();
    assert_eq!(r.track, 0);
    assert_eq!(r.heard, "the wezar went");
    assert_eq!((r.first.start, r.last.start), (2.0, 3.0));
    let fix = Fix {
        track: 0,
        start: r.first.start,
        nth: r.first.nth,
        through: Some(r.last),
        text: "Uezar left".into(),
    };
    let v = TranscriptView::new(&w, &Speakers::default(), &[fix], &[]);
    assert_eq!(v.text, ["so", "the", "wezar", "yeah", "went", "ok"]);
    assert_eq!(v.span, [0, 1, 1, 0, 1, 0]);
    assert_eq!(v.spans, ["Uezar left"]);
    assert_eq!(v.track, [0, 0, 0, 1, 0, 0]);
    // Refused: starting and ending on two tracks; two people's words.
    assert!(w.retyped(1, 3, "x", &Speakers::default(), &[]).is_err());
    assert!(w.retyped(0, 9, "x", &Speakers::default(), &[]).is_err());
    use junctur_core::edit::Turn;
    let turn = |start, end, speaker| Turn {
        start,
        end,
        speaker,
    };
    let two = Speakers::new(
        vec![turn(0.5, 1.9, 1), turn(1.9, 5.0, 2)],
        &[None, None],
        None,
    );
    let why = w.retyped(0, 2, "So Uezar", &two, &[]).unwrap_err();
    assert!(why.contains("two people"), "{why}");
    // Clearing is never refused for the speakers: a span over two people's words can still be
    // taken away, by an empty text or the heard words.
    assert!(w.retyped(0, 2, "", &two, &[]).is_ok());
    assert!(w.retyped(0, 2, " so  the wezar", &two, &[]).is_ok());
    assert!(
        w.retyped(1, 4, "x", &two, &[]).is_ok(),
        "one person's words, and track 1's are not counted"
    );
    // A same-start group is a run like any other.
    let g = Words::from_tracked(vec![
        (0, word(54.944, 54.944, "en")),
        (0, word(54.944, 55.0, "él")),
    ]);
    let r = g.retyped(0, 1, "x", &Speakers::default(), &[]).unwrap();
    assert_eq!((r.first.nth, r.last.nth), (0, 1));
}

#[test]
fn words_are_labelled_by_turn_or_by_track() {
    use junctur_core::edit::Turn;
    let turn = |start, end, speaker| Turn {
        start,
        end,
        speaker,
    };
    // By voice: "uno" and "dos" in speaker 2's turn, "tres" in speaker 1's.
    let voices = Speakers::new(
        vec![turn(9.8, 11.2, 2), turn(11.8, 13.0, 1)],
        &[None, None],
        None,
    );
    assert_eq!(
        TranscriptView::new(&sample(), &voices, &[], &[]).speaker,
        vec![2, 2, 1]
    );
    // By track: the track a word was heard on decides, whatever the turns overlap.
    let w = Words::from_tracked(vec![
        (3, word(10.0, 10.4, "hola")),
        (0, word(10.1, 10.3, "sí")),
    ]);
    assert_eq!(w.tracks, vec![3, 0]);
    let tracks = Speakers::new(
        vec![turn(10.0, 10.4, 1), turn(9.0, 11.0, 2)],
        &[Some(0), Some(3)],
        None,
    );
    assert_eq!(
        TranscriptView::new(&w, &tracks, &[], &[]).speaker,
        vec![2, 1]
    );
}

#[test]
fn spans_stay_in_the_pauses() {
    let w = sample();
    let s = CutSettings::default();
    let env = Envelope::default();
    // Cut "dos": from the pause after "uno" (only 0.1 s, so its far edge) to 0.25 s before
    // "tres".
    let (a, b) = w.span(1, 1, false, &placing(&env, &s, 60.0, &[])).unwrap();
    assert!(
        (a - 10.5).abs() < 1e-9 && (b - 11.75).abs() < 1e-9,
        "{a} {b}"
    );
    let (a, b) = w.span(1, 1, true, &placing(&env, &s, 60.0, &[])).unwrap();
    assert!(
        (a - 10.4).abs() < 1e-9 && (b - 11.35).abs() < 1e-9,
        "{a} {b}"
    );
    // Reversed indices are the same span; out of range is none.
    assert_eq!(
        w.span(2, 1, false, &placing(&env, &s, 60.0, &[])),
        w.span(1, 2, false, &placing(&env, &s, 60.0, &[]))
    );
    assert_eq!(w.span(0, 3, false, &placing(&env, &s, 60.0, &[])), None);
}

#[test]
fn a_long_word_on_another_track_is_not_clipped_by_start_order() {
    // "long" (track A) 10-15 outlasts "x" (track B) 10.5-11. Cutting "dos" 12-12.5: by start
    // order the pause before "dos" would begin at 11.0, inside "long".
    let w = Words::from_words(vec![
        word(10.0, 15.0, "long"),
        word(10.5, 11.0, "x"),
        word(12.0, 12.5, "dos"),
        word(16.0, 16.5, "after"),
    ]);
    let s = CutSettings::default();
    let (a, b) = w
        .span(2, 2, false, &placing(&Envelope::default(), &s, 60.0, &[]))
        .unwrap();
    assert_eq!(
        a, 12.0,
        "no pause before: the selection's own edge, said as such"
    );
    // The trailing boundary goes in the real pause after "long" ends, not inside it.
    assert!((15.0..=16.0).contains(&b), "{b}");
}

#[test]
fn retimed_words_are_sent_with_their_heard_starts_and_spans_reach_the_earliest_next() {
    // Track 1's word retimed to start before track 0's word heard before it.
    let w = Words::from_tracked(vec![
        (0, word(10.0, 10.4, "uno")),
        (0, word(10.5, 11.0, "dos")),
        (1, word(10.6, 11.0, "yes")),
    ]);
    let times = vec![Retime {
        track: 1,
        start: 10.6,
        nth: 0,
        from: 10.45,
        to: 11.0,
    }];
    let v = TranscriptView::new(&w, &Speakers::default(), &[], &times);
    assert_eq!(v.start, [10_000, 10_500, 10_450]);
    assert_eq!(v.heard, [10_000, 10_500, 10_600]);
    assert!(TranscriptView::new(&w, &Speakers::default(), &[], &[])
        .heard
        .is_empty());
    // Cutting "uno": what follows starts at 10.45 (the retimed "yes"), not 10.5.
    let s = CutSettings::default();
    let env = Envelope::default();
    let (_, b) = w
        .span(0, 0, false, &placing(&env, &s, 60.0, &times))
        .unwrap();
    assert!(b <= 10.45, "{b}");
}

/// The words' cut placing in these tests, as the editor gives it from a list.
fn placing<'a>(
    env: &'a Envelope,
    settings: &'a CutSettings,
    duration: f64,
    times: &'a [Retime],
) -> Placing<'a> {
    Placing {
        env,
        settings,
        duration,
        times,
    }
}

#[test]
fn a_words_own_edge_trims_the_neighbour_where_it_now_is_and_stops_at_the_words_to_come() {
    const FPS: Fps = Fps { num: 30, den: 1 };
    let words = Words::from_tracked(vec![
        (0, word(50.0, 50.4, "a")),
        (0, word(50.5, 50.9, "w")),
        (0, word(51.5, 51.9, "b")),
    ]);
    // "a" was heard before "w" but is retimed after it.
    let times = vec![Retime {
        track: 0,
        start: 50.0,
        nth: 0,
        from: 51.0,
        to: 51.3,
    }];
    let spans = |t: Vec<Retime>| -> Vec<(f64, f64, f64)> {
        t.iter().map(|r| (r.start, r.from, r.to)).collect()
    };
    // Its start has room: nothing on its track lies before it any more.
    let t = words
        .retimed(&times, 1, Side::Start, 1509, 60.0, FPS)
        .unwrap();
    assert_eq!(spans(t), [(50.0, 51.0, 51.3), (50.5, 50.3, 50.9)]);
    // Its end runs into "a", where "a" now is, and trims it there; "b" is not reached.
    let t = words
        .retimed(&times, 1, Side::End, 1536, 60.0, FPS)
        .unwrap();
    assert_eq!(spans(t), [(50.0, 51.2, 51.3), (50.5, 50.5, 51.2)]);
    // While transcribing, the last word's end stops where the words still to come start.
    let mut so_far = Words::from_tracked(vec![(0, word(50.5, 50.9, "w"))]);
    so_far.until = Some(52.0);
    let t = so_far.retimed(&[], 0, Side::End, 1650, 60.0, FPS).unwrap();
    assert_eq!(spans(t), [(50.5, 50.5, 52.0)]);
}
