use super::*;

fn word(start: f64, text: &str) -> Word {
    Word {
        start,
        end: start + 0.3,
        text: text.into(),
        p: 1.0,
        p_min: None,
    }
}

fn fix(track: usize, start: f64, text: &str) -> Fix {
    Fix {
        track,
        start,
        nth: 0,
        through: None,
        text: text.into(),
    }
}

fn at(start: f64, nth: usize) -> WordAt {
    WordAt { start, nth }
}

/// What `ws` show, each heard on its track: a span's text once.
fn shown(fixes: &[Fix], ws: &[(usize, Word)]) -> Vec<String> {
    let mut by = Fixes::new(fixes);
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (t, w) in ws {
        match by.of(*t, w) {
            Some(super::super::Fixed::Span { index, text }) => {
                if !seen.contains(&index) {
                    seen.push(index);
                    out.push(text.to_string());
                }
            }
            Some(super::super::Fixed::Word(t)) => out.push(t.to_string()),
            None => out.push(w.text.trim().to_string()),
        }
    }
    out
}

fn refs(ws: &[(usize, Word)]) -> Vec<(usize, &Word)> {
    ws.iter().map(|(t, w)| (*t, w)).collect()
}

/// "Bane" heard twice (once with a comma), and once on the other track.
fn banes() -> Vec<(usize, Word)> {
    vec![
        (0, word(1.0, " Bane,")),
        (0, word(1.5, " is")),
        (1, word(2.0, " bane")),
        (0, word(3.0, " Bane.")),
        (0, word(4.0, " Baneful")),
    ]
}

#[test]
fn a_fix_spreads_to_every_word_heard_alike_in_its_own_punctuation() {
    let ws = banes();
    let mut f = vec![fix(0, 1.0, "Vayne,")];
    let s = spread(
        &mut f,
        &refs(&ws),
        Which::Words {
            track: 0,
            first: at(1.0, 0),
            last: at(1.0, 0),
        },
    );
    assert_eq!(
        shown(&f, &ws),
        ["Vayne,", "is", "Vayne", "Vayne.", "Baneful"]
    );
    assert_eq!(s.added.len(), 2, "{s:?}");
    assert_eq!((s.added[0].track, s.added[1].track), (1, 0));
    assert!(s.unclear.is_empty());
    // Spread again: nothing more, every "Bane" is fixed.
    assert_eq!(spread(&mut f, &refs(&ws), Which::All), Spread::default());
    assert_eq!(s.heard, ["bane", "Bane."]);
    // "Undo those": the others say what was heard again, the fix made by hand stays, and one
    // changed since stays as changed.
    f[1].text = "Vain".into();
    assert_eq!(keep_heard(&mut f, &s.added, &s.heard), 1);
    assert_eq!(shown(&f, &ws), ["Vayne,", "is", "Vain", "Bane.", "Baneful"]);
    // Kept as heard is kept: spreading every fix again does not reach it.
    f[1].text = "bane".into();
    assert_eq!(spread(&mut f, &refs(&ws), Which::All), Spread::default());
    assert_eq!(shown(&f, &ws), ["Vayne,", "is", "bane", "Bane.", "Baneful"]);
}

#[test]
fn one_index_kept_for_fix_after_fix_spreads_as_a_fresh_one_does() {
    let mut ws = banes();
    ws.extend([
        (1, word(6.0, " so")),
        (1, word(6.0, " Bane")),
        (0, word(7.0, " big")),
        (0, word(7.5, " bane")),
        (1, word(8.0, " big")),
        (1, word(8.5, " Bane!")),
        (0, word(9.0, "")),
    ]);
    let words = refs(&ws);
    let index = SpreadIndex::new(&words);
    assert_eq!(index, SpreadIndex::new(&words), "built the same each time");
    let one = |track, start, nth| Which::Words {
        track,
        first: at(start, nth),
        last: at(start, nth),
    };
    let steps = [
        (fix(0, 1.0, "Vayne,"), one(0, 1.0, 0)),
        (fix(0, 7.0, "pig"), one(0, 7.0, 0)),
        (fix(0, 4.0, "Painful"), one(0, 4.0, 0)),
        (fix(0, 1.5, "is"), Which::All),
    ];
    let (mut fresh, mut kept) = (Vec::new(), Vec::new());
    for (f, which) in steps {
        fresh.push(f.clone());
        kept.push(f);
        fixes::sort(&mut fresh);
        fixes::sort(&mut kept);
        let a = spread(&mut fresh, &words, which);
        let b = spread_with(&mut kept, &words, &index, which);
        assert_eq!(a, b);
        assert_eq!(fresh, kept);
    }
    assert!(fresh.len() > 4, "the fixes spread: {fresh:?}");
    // An index of other words (fewer of them) is not used.
    let fewer = refs(&ws[..3]);
    let mut f = vec![fix(0, 1.0, "Vayne,")];
    let mut g = f.clone();
    assert_eq!(
        spread_with(&mut f, &fewer, &index, Which::All),
        spread(&mut g, &fewer, Which::All)
    );
    assert_eq!(f, g);
}

#[test]
fn a_word_already_fixed_keeps_its_own_fix_and_a_typed_text_goes_as_typed() {
    let ws = banes();
    // The second "Bane" was fixed by hand to something else: it stays.
    let mut f = vec![fix(0, 1.0, "Vayne!"), fix(0, 3.0, "Bain.")];
    let s = spread(
        &mut f,
        &refs(&ws),
        Which::Words {
            track: 0,
            first: at(1.0, 0),
            last: at(1.0, 0),
        },
    );
    // "Vayne!" changed the punctuation it was heard with: written as typed.
    assert_eq!(
        shown(&f, &ws),
        ["Vayne!", "is", "Vayne!", "Bain.", "Baneful"]
    );
    assert_eq!(s.added.len(), 1);
    // From every fix, the two disagree: no more spreading, and the heard word is unclear.
    let mut more = ws.clone();
    more.push((0, word(5.0, " BANE")));
    let mut g = vec![fix(0, 1.0, "Vayne,"), fix(0, 3.0, "Bain.")];
    let s = spread(&mut g, &refs(&more), Which::All);
    assert!(s.added.is_empty(), "{s:?}");
    assert_eq!(s.unclear, ["bane"]);
}

#[test]
fn a_punctuation_only_fix_or_a_case_fix_spreads_as_it_should() {
    let ws = vec![
        (0, word(1.0, " so")),
        (0, word(2.0, " so")),
        (0, word(3.0, " tsunheto")),
        (0, word(4.0, " Tsunheto?")),
        (0, word(5.0, " tsunheto")),
    ];
    // "so" made "so," is not a mishearing: not spread.
    let mut f = vec![fix(0, 1.0, "so,")];
    assert!(spread(&mut f, &refs(&ws), Which::All).added.is_empty());
    // A name heard in lower case: its case spreads, the other word's punctuation kept.
    let mut f = vec![fix(0, 3.0, "Tsunheto")];
    let s = spread(&mut f, &refs(&ws), Which::All);
    assert_eq!(
        shown(&f, &ws),
        ["so", "so", "Tsunheto", "Tsunheto?", "Tsunheto"]
    );
    // "Tsunheto?" already reads as the fix: no fix needed there.
    assert_eq!(s.added.len(), 1, "{s:?}");
}

#[test]
fn a_span_spreads_over_the_same_words_on_any_track_never_over_a_fixed_one() {
    let ws = vec![
        (0, word(1.0, " the")),
        (0, word(1.3, " wezar")),
        (0, word(1.6, " went")),
        (0, word(2.0, " The")),
        (0, word(2.3, " Wezar")),
        (0, word(2.6, " went.")),
        (1, word(3.0, " the")),
        (1, word(3.3, " wezar")),
        (1, word(3.6, " went")),
        (0, word(4.0, " the")),
        (0, word(4.3, " wezar")),
        (0, word(4.6, " sat")),
        (0, word(5.0, " the")),
        (0, word(5.3, " wezar")),
        (0, word(5.6, " went")),
    ];
    let mut f = vec![
        Fix {
            track: 0,
            start: 1.0,
            nth: 0,
            through: Some(at(1.6, 0)),
            text: "Uezar went".into(),
        },
        fix(0, 5.6, "left"),
    ];
    let s = spread(&mut f, &refs(&ws), Which::All);
    assert_eq!(
        shown(&f, &ws),
        [
            "Uezar went",
            "Uezar went",
            "Uezar went",
            "the",
            "wezar",
            "sat",
            "the",
            "wezar",
            "left"
        ]
    );
    assert_eq!(s.added.len(), 2, "{s:?}");
    assert!(s.added.iter().all(Fix::is_span));
    // A retyped sentence that only moved punctuation is not spread.
    let mut g = vec![Fix {
        track: 0,
        start: 1.0,
        nth: 0,
        through: Some(at(1.6, 0)),
        text: "the wezar, went.".into(),
    }];
    assert!(spread(&mut g, &refs(&ws), Which::All).added.is_empty());
}

#[test]
fn a_fix_follows_its_word_into_a_new_transcription() {
    let old = vec![
        (0, word(1.0, " Bane,")),
        (0, word(2.0, " tsunheto")),
        (0, word(3.0, " ok")),
        (0, word(4.0, " the")),
        (0, word(4.3, " wezar")),
        (0, word(6.0, " Mel")),
        (0, word(8.0, " hom")),
    ];
    let mut f = vec![
        fix(0, 1.0, "Vayne,"),
        fix(0, 2.0, "Tsunheto"),
        fix(0, 3.0, "okay"),
        Fix {
            track: 0,
            start: 4.0,
            nth: 0,
            through: Some(at(4.3, 0)),
            text: "Uezar".into(),
        },
        fix(0, 6.0, "Mell"),
        fix(0, 8.0, "home"),
    ];
    let carried = snapshot(&f, &refs(&old));
    assert_eq!(carried.len(), 6);
    assert_eq!(carried[0].heard, ["Bane,"]);
    // While it ran, the user fixed a new word on their own (at 8.1, where "hom" was): the old
    // fix of that word does not go over it.
    f.push(fix(0, 8.1, "house"));
    let new = vec![
        // Moved a little, still misheard: the fix moves with it.
        (0, word(1.1, " Bane,")),
        // Heard right now, but in lower case: the case fix stays.
        (0, word(2.0, " tsunheto")),
        // Heard as the fix: not needed.
        (0, word(3.05, "okay")),
        // The span's words came out as one word: a word fix.
        (0, word(4.1, " thewezar")),
        // Something else entirely where "Mel" was: dropped.
        (0, word(6.0, " and")),
        (0, word(8.1, " hom")),
    ];
    let r = carry(&mut f, &carried, &refs(&new));
    assert_eq!(r.kept, 3, "{r:?}");
    assert_eq!(r.right, 1);
    let gone: Vec<(String, f64)> = r.gone.iter().map(|g| (g.heard.join(" "), g.from)).collect();
    assert_eq!(gone, [("Mel".to_string(), 6.0), ("hom".to_string(), 8.0)]);
    assert_eq!(r.gone[0].fix.text, "Mell", "set aside as it was");
    assert_eq!(
        shown(&f, &new),
        ["Vayne,", "Tsunheto", "okay", "Uezar", "and", "house"]
    );
    assert_eq!(f.iter().filter(|x| x.start == 1.1).count(), 1);
    // Carried again onto the same words: nothing moves (a resumed run's end).
    let again = snapshot(&f, &refs(&new));
    let before = f.clone();
    carry(&mut f, &again, &refs(&new));
    assert_eq!(f, before);
}

#[test]
fn a_spread_never_lays_a_fix_over_one_whose_word_is_gone() {
    // A fix left on no word (a word re-transcribed away) inside the keys a
    // spread span would cover; with both the list would not read back.
    let ws = vec![
        (0, word(1.0, " the")),
        (0, word(1.3, " wezar")),
        (0, word(2.0, " the")),
        (0, word(2.3, " wezar")),
    ];
    let mut f = vec![
        Fix {
            track: 0,
            start: 1.0,
            nth: 0,
            through: Some(at(1.3, 0)),
            text: "Uezar".into(),
        },
        fix(0, 2.15, "stale"),
    ];
    let s = spread(&mut f, &refs(&ws), Which::All);
    assert!(s.added.is_empty(), "{s:?}");
    assert!(!super::super::fixes::clash(&f));
    // A word fix next to it still spreads.
    let mut g = vec![fix(0, 1.3, "Uezar"), fix(0, 2.15, "stale")];
    assert_eq!(spread(&mut g, &refs(&ws), Which::All).added.len(), 1);
    assert!(!super::super::fixes::clash(&g));
}

#[test]
fn a_carried_fix_takes_its_new_words_punctuation_and_keeps_its_shape() {
    // "Vayne," onto a "Bane." heard now is "Vayne.", as a spread would write it.
    let old = vec![
        (0, word(1.0, " Bane,")),
        (0, word(3.0, " the")),
        (0, word(3.3, " wezar")),
        (1, word(5.0, " hola")),
    ];
    let mut f = vec![
        fix(0, 1.0, "Vayne,"),
        Fix {
            track: 0,
            start: 3.0,
            nth: 0,
            through: Some(at(3.3, 0)),
            text: "Uezar went".into(),
        },
        fix(1, 5.0, "Hola,"),
    ];
    let carried = snapshot(&f, &refs(&old));
    // Track 1 heard nothing this time; the sentence came out as one word.
    let new = vec![(0, word(1.0, " Bane.")), (0, word(3.1, " thewezar"))];
    let r = carry(&mut f, &carried, &refs(&new));
    assert_eq!(f[0].text, "Vayne.");
    // A sentence is not made a one-word fix (a spread would copy it onto every such word):
    // set aside. And the track with no words sets its fix aside too.
    let gone: Vec<&str> = r.gone.iter().map(|g| g.fix.text.as_str()).collect();
    assert_eq!(gone, ["Uezar went", "Hola,"]);
    assert_eq!(f.len(), 1);
    assert!(!super::super::fixes::clash(&f));
    // A transcript with no words at all (a run that heard nothing) moves nothing.
    let mut g = vec![fix(1, 5.0, "Hola,")];
    let carried = snapshot(&g, &refs(&old));
    assert_eq!(carry(&mut g, &carried, &[]), CarryReport::default());
    assert_eq!(g, [fix(1, 5.0, "Hola,")]);
}

#[test]
fn a_fix_kept_as_heard_is_never_carried_longer_than_a_fix_may_be() {
    // A span kept as heard past `MAX_SPAN`, or a word heard now longer than `MAX_FIX`, would not
    // read back: set aside.
    let run = |w: &str| -> Vec<(usize, Word)> {
        (0..100)
            .map(|i| (0, word(1.0 + i as f64 * 0.1, w)))
            .collect()
    };
    let old = run(" palabra");
    let said: Vec<&str> = old.iter().map(|(_, w)| w.text.trim()).collect();
    let mut f = vec![Fix {
        track: 0,
        start: 1.0,
        nth: 0,
        through: Some(at(1.0 + 99.0 * 0.1, 0)),
        text: said.join(" "),
    }];
    assert!(f[0].text.chars().count() <= super::super::MAX_SPAN);
    let carried = snapshot(&f, &refs(&old));
    // Heard again as longer words in the same time: as said now it is over a span's limit.
    let r = carry(&mut f, &carried, &refs(&run(" palabrotas")));
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(r.gone.len(), 1);
    // One word kept as heard, now heard with a run of punctuation past a word fix's limit.
    let mut g = vec![fix(0, 1.0, "Bane")];
    let carried = snapshot(&g, &refs(&[(0, word(1.0, " Bane"))]));
    let loud = format!(" Bane{}", "!".repeat(super::super::MAX_FIX));
    let r = carry(&mut g, &carried, &refs(&[(0, word(1.0, &loud))]));
    assert_eq!(r.gone.len(), 1, "{r:?}");
    assert!(g.is_empty());
}

#[test]
fn a_retimed_word_follows_its_word_into_a_new_transcription() {
    // A word pulled out of a cut by moving the cut's edge keeps that edge after
    // Correct: its `time` line moves onto the new word, the side the user moved kept.
    let old = vec![
        (0, word(1.0, " Bane,")),
        (0, word(2.0, " said")),
        (0, word(4.0, " Mel")),
        (1, word(6.0, " hola")),
    ];
    let rt = |track: usize, start: f64, from: f64, to: f64| Retime {
        track,
        start,
        nth: 0,
        from,
        to,
    };
    let timed = |start: f64, end: f64, text: &str| Word {
        end,
        ..word(start, text)
    };
    // "Bane," ended at 1.3 as heard, the edge put it at 1.5; "said" started at 2.0, the edge
    // put it at 1.9, its end Whisper's; "Mel" and "hola" are retimed too.
    let mut t = vec![
        rt(0, 1.0, 1.0, 1.5),
        rt(0, 2.0, 1.9, 2.3),
        rt(0, 4.0, 4.0, 4.6),
        rt(1, 6.0, 6.0, 6.5),
    ];
    // A fix on "Bane,": the new word may be heard as the fix.
    let f = vec![fix(0, 1.0, "Vayne,")];
    let carried = snapshot_times(&t, &f, &refs(&old));
    assert_eq!(carried.len(), 4);
    assert_eq!(carried[0].fixed.as_deref(), Some("Vayne,"));
    // While it ran, the user retimed a new word on their own: it stays.
    t.push(rt(0, 8.0, 8.0, 8.9));
    let new = vec![
        // Heard as the fix now, a little later: the user's end kept, the start the new one.
        (0, timed(1.1, 1.35, " Vayne,")),
        // Heard from the edge already, its end as before: not needed.
        (0, timed(1.9, 2.3, " said")),
        // Something else where "Mel" was: set aside. Track 1 heard nothing: set aside.
        (0, word(4.0, " and")),
        (0, word(8.0, " bien")),
    ];
    let r = carry_times(&mut t, &carried, &refs(&new));
    assert_eq!((r.kept, r.right), (1, 1), "{r:?}");
    let gone: Vec<&str> = r.gone.iter().map(|g| g.heard.as_str()).collect();
    assert_eq!(gone, ["Mel", "hola"]);
    assert_eq!(t, [rt(0, 1.1, 1.1, 1.5), rt(0, 8.0, 8.0, 8.9)]);
    // Read where the words are: "Vayne," ends where the edge is.
    let mut times = super::super::Times::new(&t);
    assert_eq!(times.of(0, &new[0].1), (1.1, 1.5));
    // Carried again onto the same words: nothing moves.
    let again = snapshot_times(&t, &f, &refs(&new));
    let before = t.clone();
    carry_times(&mut t, &again, &refs(&new));
    assert_eq!(t, before);
    // The user's end at 1.5, the new word now starting at 1.6: nothing of it left before the
    // edge, set aside rather than written backwards.
    let mut v = vec![rt(0, 1.0, 1.0, 1.5)];
    let c = snapshot_times(&v, &[], &refs(&old));
    let r = carry_times(&mut v, &c, &refs(&[(0, timed(1.6, 1.9, " Bane,"))]));
    assert_eq!(r.gone.len(), 1);
    assert!(v.is_empty());
}

#[test]
fn a_word_kept_as_heard_stays_so_after_a_new_transcription() {
    // "Undo those" left the second "nuevo" as heard. A new transcription keeps that on its
    // word, as it now reads, so the spread after it does not reach it.
    let old = vec![(0, word(1.0, " nuevo")), (0, word(4.0, " nuevo"))];
    let mut f = vec![fix(0, 1.0, "gordo"), fix(0, 4.0, "nuevo")];
    let carried = snapshot(&f, &refs(&old));
    let new = vec![(0, word(1.0, " nuevo")), (0, word(4.1, " nuevo,"))];
    let r = carry(&mut f, &carried, &refs(&new));
    assert_eq!((r.kept, r.right), (2, 0));
    assert_eq!(f[1].text, "nuevo,");
    assert!(spread(&mut f, &refs(&new), Which::All).added.is_empty());
    assert_eq!(shown(&f, &new), ["gordo", "nuevo,"]);
}

#[test]
fn a_span_is_laid_over_the_new_words_said_in_its_time() {
    let old = vec![
        (0, word(1.0, " so")),
        (0, word(2.0, " the")),
        (0, word(2.4, " wezar")),
        (0, word(2.8, " went")),
        (0, word(4.0, " ok")),
    ];
    let span = Fix {
        track: 0,
        start: 2.0,
        nth: 0,
        through: Some(at(2.8, 0)),
        text: "Uezar went home.".into(),
    };
    let mut f = vec![span.clone()];
    let carried = snapshot(&f, &refs(&old));
    assert_eq!(carried[0].from, 2.0);
    assert!((carried[0].to - 3.1).abs() < 1e-9, "{}", carried[0].to);
    let new = vec![
        (0, word(1.0, " so")),
        (0, word(2.05, " the")),
        (0, word(2.3, " wezar")),
        (0, word(2.5, " uh")),
        (0, word(2.75, " went")),
        (0, word(4.0, " ok")),
    ];
    let r = carry(&mut f, &carried, &refs(&new));
    assert_eq!(r.kept, 1);
    assert_eq!(shown(&f, &new), ["so", "Uezar went home.", "ok"]);
    assert_eq!(f[0].through, Some(at(2.75, 0)));
    // The same words heard again, earlier than its old time: carried onto them.
    let mut f = vec![span.clone()];
    let earlier = vec![
        (0, word(1.0, " so")),
        (0, word(1.5, " the")),
        (0, word(1.7, " wezar")),
        (0, word(1.9, " went")),
        (0, word(4.0, " ok")),
    ];
    let r = carry(&mut f, &carried, &refs(&earlier));
    assert_eq!(r.kept, 1);
    assert_eq!((f[0].start, f[0].through), (1.5, Some(at(1.9, 0))));
    // Said as the span now: not needed.
    let mut f = vec![span];
    let right = vec![
        (0, word(2.0, " Uezar")),
        (0, word(2.4, " went")),
        (0, word(2.8, " home.")),
    ];
    let r = carry(&mut f, &carried, &refs(&right));
    assert_eq!((r.right, f.len()), (1, 0));
}
