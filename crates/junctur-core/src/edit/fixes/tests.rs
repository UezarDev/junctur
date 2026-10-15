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

#[test]
fn a_fix_is_set_changed_and_removed() {
    let mut f = Vec::new();
    assert!(set_fix(&mut f, 1, 12.34, 0, " Wezar,", " Uezar, ").unwrap());
    assert_eq!(f[0].text, "Uezar,");
    assert!(
        !set_fix(&mut f, 1, 12.34, 0, " Wezar,", "Uezar,").unwrap(),
        "same"
    );
    assert!(set_fix(&mut f, 0, 3.0, 0, "a", "b").unwrap());
    assert_eq!(
        f.iter().map(|x| x.track).collect::<Vec<_>>(),
        [0, 1],
        "in time"
    );
    // The heard text again, or nothing, takes the fix away.
    assert!(set_fix(&mut f, 1, 12.34, 0, " Wezar,", "Wezar,").unwrap());
    assert!(set_fix(&mut f, 0, 3.0, 0, "a", "  ").unwrap());
    assert!(f.is_empty());
    assert!(
        !set_fix(&mut f, 0, 3.0, 0, "a", "a").unwrap(),
        "nothing to fix"
    );
    // One line, not too long.
    assert!(set_fix(&mut f, 0, 3.0, 0, "a", "Tsun\nheto").unwrap());
    assert_eq!(f[0].text, "Tsun heto");
    assert!(set_fix(&mut f, 0, 3.0, 0, "a", &"x".repeat(MAX_FIX + 1)).is_err());
}

/// A confirmed word is a fix to its own text; a fixed word stays fixed, and the heard text
/// typed in again takes the confirmation away like any fix.
#[test]
fn a_word_is_confirmed_as_a_fix_to_what_was_heard() {
    let mut f = Vec::new();
    assert!(confirm_fix(&mut f, 1, 12.34, 0, " Uezar, ").unwrap());
    assert_eq!(f[0].text, "Uezar,");
    assert!(!confirm_fix(&mut f, 1, 12.34, 0, "Uezar,").unwrap(), "once");
    assert_eq!(Fixes::new(&f).text(1, &word(12.34, "Uezar,")), "Uezar,");
    assert!(set_fix(&mut f, 0, 3.0, 0, "Wezar", "Uezar").unwrap());
    assert!(!confirm_fix(&mut f, 0, 3.0, 0, "Wezar").unwrap());
    assert_eq!(f[0].text, "Uezar", "the fix stands");
    assert!(set_fix(&mut f, 1, 12.34, 0, "Uezar,", "Uezar,").unwrap());
    assert_eq!(f.len(), 1);
    assert!(confirm_fix(&mut f, 0, 9.0, 0, "  ").is_err());
}

/// A word inside a span is not confirmed beside it (the span is the user's text for
/// it), and a span set over a confirmed word takes the confirmation into it.
#[test]
fn a_confirmation_and_a_span_never_both_name_a_word() {
    let at = |start| WordAt { start, nth: 0 };
    let mut f = Vec::new();
    assert!(confirm_fix(&mut f, 0, 2.0, 0, "dos").unwrap());
    assert!(set_span(&mut f, 0, at(1.0), at(3.0), "uno dos tres", "one two three").unwrap());
    assert_eq!(f.len(), 1, "the confirmation went into the span");
    assert!(f[0].is_span());
    assert!(!confirm_fix(&mut f, 0, 2.0, 0, "dos").unwrap());
    assert_eq!(f.len(), 1, "no confirmation inside a span");
    // Outside it, on another track or past its end: confirmed as before.
    assert!(confirm_fix(&mut f, 1, 2.0, 0, "dos").unwrap());
    assert!(confirm_fix(&mut f, 0, 4.0, 0, "cuatro").unwrap());
}

/// Confirming one of the words starting in one millisecond confirms that word alone,
/// next to another's fix, and taking it away leaves the other.
#[test]
fn a_word_of_those_starting_together_is_confirmed_alone() {
    let ws = same_start();
    let mut f = Vec::new();
    assert!(set_fix(&mut f, 0, 54.944, 0, "en", "de").unwrap());
    assert!(confirm_fix(&mut f, 0, 54.944, 1, "él").unwrap());
    assert!(
        !confirm_fix(&mut f, 0, 54.944, 0, "en").unwrap(),
        "fixed already"
    );
    assert_eq!(f.iter().map(|x| x.nth).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(said(&f, &ws), ["manada", "de", "él", "y"]);
    assert!(set_fix(&mut f, 0, 54.944, 1, "él", "").unwrap());
    assert_eq!(said(&f, &ws), ["manada", "de", "él", "y"]);
    assert_eq!(f.len(), 1);
}

#[test]
fn two_fixes_on_one_word_are_found_before_a_list_is_written() {
    // The parser refuses such a list, so code adding fixes in bulk checks first.
    let span = Fix {
        track: 0,
        start: 1.0,
        nth: 0,
        through: Some(WordAt { start: 3.0, nth: 0 }),
        text: "a b c".into(),
    };
    let word = |track, start: f64| Fix {
        track,
        start,
        nth: 0,
        through: None,
        text: "x".into(),
    };
    assert!(!clash(&[span.clone(), word(0, 4.0), word(1, 2.0)]));
    assert!(clash(&[word(0, 4.0), span.clone(), word(0, 2.0)]));
    assert!(clash(&[word(0, 2.0), word(0, 2.0)]));
    assert!(!clash(&[]));
}

#[test]
fn a_half_millisecond_start_is_written_as_its_key() {
    assert_eq!(written(12.3125), "12.313");
    assert_eq!(written(2.5), "2.500");
    assert_eq!(written(0.0004), "0.000");
    assert_eq!(written(-1.5), "-1.500");
    let start: f64 = written(12.3125).parse().unwrap();
    assert_eq!(key(0, start, 0), key(0, 12.3125, 0));
}

#[test]
fn a_word_is_found_by_track_and_millisecond() {
    let fixes = vec![Fix {
        track: 1,
        start: 312.34,
        nth: 0,
        through: None,
        text: "Uezar".into(),
    }];
    // A start summed from a chunk's and a token's time, off in the last bits.
    let heard = word(300.0 + 12.34, "Wezar");
    assert_eq!(Fixes::new(&fixes).text(1, &heard), "Uezar");
    assert_eq!(Fixes::new(&fixes).text(0, &heard), "Wezar", "another track");
    assert_eq!(
        Fixes::new(&fixes).text(1, &word(312.341, "Wezar")),
        "Wezar",
        "another word"
    );
}

/// Words where Whisper gave "en" and "él" one start.
fn same_start() -> Vec<Word> {
    vec![
        word(54.174, "manada"),
        word(54.944, "en"),
        word(54.944, "él"),
        word(55.5, "y"),
    ]
}

/// What `ws` say, heard on track 0, read in order.
fn said(fixes: &[Fix], ws: &[Word]) -> Vec<String> {
    let mut by = Fixes::new(fixes);
    ws.iter().map(|w| by.text(0, w).to_string()).collect()
}

#[test]
fn words_starting_in_one_millisecond_each_take_their_own_fix() {
    // A fix on "él" stays off "en", and each can be taken away alone.
    let ws = same_start();
    let mut f = Vec::new();
    assert!(set_fix(&mut f, 0, 54.944, 1, "él", "miel").unwrap());
    assert_eq!(said(&f, &ws), ["manada", "en", "miel", "y"]);
    assert!(set_fix(&mut f, 0, 54.944, 0, "en", "de").unwrap());
    assert_eq!(said(&f, &ws), ["manada", "de", "miel", "y"]);
    // Taking one away leaves the other.
    assert!(set_fix(&mut f, 0, 54.944, 0, "en", "").unwrap());
    assert_eq!(said(&f, &ws), ["manada", "en", "miel", "y"]);
    assert!(set_fix(&mut f, 0, 54.944, 1, "él", "él").unwrap());
    assert!(f.is_empty());
    // Another track's word at the same moment counts on its own track.
    let f = vec![Fix {
        track: 0,
        start: 54.944,
        nth: 1,
        through: None,
        text: "miel".into(),
    }];
    let mut by = Fixes::new(&f);
    let other = word(54.944, "yes");
    assert_eq!(by.text(0, &ws[1]), "en");
    assert_eq!(by.text(2, &other), "yes");
    assert_eq!(by.text(0, &ws[2]), "miel");
    assert!(same_millisecond(54.944, 54.9444) && !same_millisecond(54.944, 54.945));
}

#[test]
fn two_tracks_interleaved_in_one_millisecond_are_counted_each_on_its_own() {
    // `Fixes` counts each track's words of a millisecond as they come, whatever the other
    // track's words between them, and starts again at the next millisecond.
    let fix = |track, start, nth, text: &str| Fix {
        track,
        start,
        nth,
        through: None,
        text: text.into(),
    };
    let f = vec![
        fix(0, 7.0, 1, "zero-1"),
        fix(1, 7.0, 0, "one-0"),
        fix(1, 7.0, 2, "one-2"),
        fix(0, 7.0, 2, "zero-2"),
        fix(1, 7.5, 0, "next"),
    ];
    let stream = [
        (1, word(7.0, "a")),
        (0, word(7.0, "b")),
        (0, word(7.0004, "c")),
        (1, word(7.0, "d")),
        (0, word(7.0, "e")),
        (1, word(7.0003, "f")),
        (1, word(7.5, "g")),
        (0, word(7.5, "h")),
    ];
    let mut by = Fixes::new(&f);
    let got: Vec<&str> = stream.iter().map(|(t, w)| by.text(*t, w)).collect();
    assert_eq!(
        got,
        ["one-0", "b", "zero-1", "d", "zero-2", "one-2", "next", "h"]
    );
}

fn at(start: f64, nth: usize) -> WordAt {
    WordAt { start, nth }
}

/// What `ws` show, heard on track 0: a span's text once, on its first word.
fn shown(fixes: &[Fix], ws: &[Word]) -> Vec<String> {
    let mut by = Fixes::new(fixes);
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for w in ws {
        match by.of(0, w) {
            Some(Fixed::Span { index, text }) => {
                if !seen.contains(&index) {
                    seen.push(index);
                    out.push(text.to_string());
                }
            }
            Some(Fixed::Word(t)) => out.push(t.to_string()),
            None => out.push(w.text.clone()),
        }
    }
    out
}

fn sentence() -> Vec<Word> {
    vec![
        word(1.0, "so"),
        word(2.0, "the"),
        word(2.5, "wezar"),
        word(3.0, "went"),
        word(3.5, "hom"),
        word(4.0, "ok"),
    ]
}

#[test]
fn a_span_replaces_the_fixes_inside_it_and_an_empty_text_clears_them() {
    // A sentence retyped as one edit; the word fixes inside it go.
    let ws = sentence();
    let mut f = Vec::new();
    set_fix(&mut f, 0, 2.5, 0, "wezar", "Uezar").unwrap();
    set_fix(&mut f, 0, 3.5, 0, "hom", "home").unwrap();
    set_fix(&mut f, 0, 4.0, 0, "ok", "okay").unwrap();
    assert_eq!(
        shown(&f, &ws),
        ["so", "the", "Uezar", "went", "home", "okay"]
    );
    let heard = "the wezar went hom";
    assert!(set_span(
        &mut f,
        0,
        at(2.0, 0),
        at(3.5, 0),
        heard,
        " Uezar  went home. "
    )
    .unwrap());
    assert_eq!(shown(&f, &ws), ["so", "Uezar went home.", "okay"]);
    assert_eq!(f.len(), 2, "the two word fixes inside went: {f:?}");
    assert_eq!(f[0].through, Some(at(3.5, 0)));
    // The same text again changes nothing; another replaces it.
    assert!(!set_span(&mut f, 0, at(2.0, 0), at(3.5, 0), heard, "Uezar went home.").unwrap());
    assert!(set_span(&mut f, 0, at(2.0, 0), at(3.5, 0), heard, "Uezar left.").unwrap());
    assert_eq!(shown(&f, &ws), ["so", "Uezar left.", "okay"]);
    // A span overlapping it takes its place whole.
    assert!(set_span(
        &mut f,
        0,
        at(1.0, 0),
        at(2.5, 0),
        "so the wezar",
        "So Uezar"
    )
    .unwrap());
    assert_eq!(shown(&f, &ws), ["So Uezar", "went", "hom", "okay"]);
    // The heard text again, or nothing, clears every fix in the words.
    assert!(set_span(
        &mut f,
        0,
        at(1.0, 0),
        at(4.0, 0),
        "so the wezar went hom ok",
        ""
    )
    .unwrap());
    assert!(f.is_empty());
    set_fix(&mut f, 0, 3.0, 0, "went", "left").unwrap();
    assert!(set_span(&mut f, 0, at(2.0, 0), at(3.5, 0), heard, heard).unwrap());
    assert!(f.is_empty());
    // Backwards is refused; too long too; one word is a word fix.
    assert!(set_span(&mut f, 0, at(3.5, 0), at(2.0, 0), heard, "x").is_err());
    assert!(set_span(
        &mut f,
        0,
        at(2.0, 0),
        at(3.5, 0),
        heard,
        &"x".repeat(MAX_SPAN + 1)
    )
    .is_err());
    assert!(set_span(
        &mut f,
        0,
        at(2.0, 0),
        at(3.5, 0),
        heard,
        &"x".repeat(MAX_FIX + 1)
    )
    .unwrap());
    assert!(set_span(&mut f, 0, at(2.5, 0), at(2.5, 0), "wezar", "Uezar").unwrap());
    assert_eq!(
        f.len(),
        1,
        "the word fix took the span holding the word: {f:?}"
    );
    assert!(!f[0].is_span());
}

#[test]
fn a_word_is_in_a_span_by_its_key_whatever_the_words_now() {
    // The span names its first and last words; a word between them is in it, also one the
    // transcript did not have when it was set, and nothing past its last key is.
    let span = vec![Fix {
        track: 0,
        start: 2.0,
        nth: 0,
        through: Some(at(3.5, 0)),
        text: "Uezar went home.".into(),
    }];
    // Re-transcribed: "the" gone, a word at 2.7, two words at 3.5 (the second after the last).
    let ws = [
        word(1.0, "so"),
        word(2.5, "wezar"),
        word(2.7, "uh"),
        word(3.5, "hom"),
        word(3.5, "e"),
        word(4.0, "ok"),
    ];
    assert_eq!(shown(&span, &ws), ["so", "Uezar went home.", "e", "ok"]);
    // Another track's words are not in it.
    let mut by = Fixes::new(&span);
    assert_eq!(by.of(1, &ws[1]), None);
    assert_eq!(
        by.text(0, &ws[1]),
        "wezar",
        "a span word reads as heard on its own"
    );
    // Spans of two tracks, in any order, each on its own.
    let two = vec![
        Fix {
            track: 1,
            start: 1.0,
            nth: 0,
            through: Some(at(4.0, 0)),
            text: "B".into(),
        },
        span[0].clone(),
    ];
    let mut by = Fixes::new(&two);
    assert_eq!(
        by.of(1, &ws[0]),
        Some(Fixed::Span {
            index: 0,
            text: "B"
        })
    );
    assert_eq!(
        by.of(0, &ws[1]),
        Some(Fixed::Span {
            index: 1,
            text: "Uezar went home."
        })
    );
    assert_eq!(by.of(0, &ws[5]), None);
}
