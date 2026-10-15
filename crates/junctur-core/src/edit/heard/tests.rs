use super::*;
use crate::edit::carry::{carry, carry_times, snapshot, snapshot_times, CarryReport};
use crate::edit::Retime;

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
fn a_carried_fix_takes_its_new_words_punctuation_and_keeps_its_shape() {
    // "Vayne," onto a "Bane." heard now is "Vayne.".
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
    // A sentence is not made a one-word fix: set aside. And the track with no words sets its fix aside too.
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
    // The second "nuevo" kept as heard (Looks right, or an older Junctur's Undo those): a new
    // transcription keeps that on its word, as it now reads, and the other fix stays its own.
    let old = vec![(0, word(1.0, " nuevo")), (0, word(4.0, " nuevo"))];
    let mut f = vec![fix(0, 1.0, "gordo"), fix(0, 4.0, "nuevo")];
    let carried = snapshot(&f, &refs(&old));
    let new = vec![(0, word(1.0, " nuevo")), (0, word(4.1, " nuevo,"))];
    let r = carry(&mut f, &carried, &refs(&new));
    assert_eq!((r.kept, r.right), (2, 0));
    assert_eq!(f[1].text, "nuevo,");
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

#[test]
fn the_fixes_are_listed_on_their_words_and_a_removed_one_goes_from_the_list() {
    // 5ae: a sentence an older Junctur gave every "no"; the list names each place with its words,
    // and Remove takes the fixes out, so the words read as heard (5ah: no line of the heard text
    // is left behind, nothing would bring the sentence back).
    let ws = vec![
        (0, word(1.0, " no")),
        (0, word(2.0, " lo")),
        (0, word(3.0, " sé")),
        (0, word(4.0, " no,")),
        (0, word(5.0, " bien")),
    ];
    let sentence = "que tiene de bueno la energia nuclear.";
    let mut f = vec![
        fix(0, 1.0, sentence),
        Fix {
            through: Some(at(3.0, 0)),
            ..fix(0, 2.0, "lo se")
        },
        fix(0, 4.0, sentence),
        fix(0, 5.0, "bien"),
        fix(0, 9.0, "gone"),
    ];
    let p = placed(&f, &refs(&ws));
    let got: Vec<_> = p
        .iter()
        .map(|p| (p.fix.text.as_str(), p.words, p.heard.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (sentence, Some((0, 0)), "no"),
            ("lo se", Some((1, 2)), "lo sé"),
            (sentence, Some((3, 3)), "no,"),
            ("bien", Some((4, 4)), "bien"),
            ("gone", None, ""),
        ]
    );
    assert!(!p[0].as_heard());
    assert!(p[3].as_heard(), "Looks right's line");
    // Both sentence copies and the fix on no word go; the span and the confirmation stay. One
    // listed but changed since is not touched.
    let mut listed = vec![p[0].fix.clone(), p[2].fix.clone(), p[4].fix.clone()];
    listed.push(Fix {
        text: "changed since".into(),
        ..p[1].fix.clone()
    });
    assert_eq!(remove(&mut f, &listed), 3);
    assert_eq!(shown(&f, &ws), ["no", "lo se", "no,", "bien"]);
    assert_eq!(f.len(), 2, "{f:?}");
    assert_eq!(remove(&mut f, &listed), 0, "already gone");
}
