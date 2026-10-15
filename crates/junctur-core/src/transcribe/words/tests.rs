use super::*;

fn t(s: &[u8], a: f64, b: f64) -> Token {
    tp(s, a, b, 1.0)
}

fn tp(s: &[u8], a: f64, b: f64, p: f32) -> Token {
    Token {
        bytes: s.to_vec(),
        start: a,
        end: b,
        p,
    }
}

#[test]
fn joins_pieces_and_split_utf8() {
    // "canción" with "ó" (C3 B3) split across two tokens, then "hola,".
    let w = group(&[
        t(b" canci", 0.0, 0.3),
        t(&[0xC3], 0.3, 0.35),
        t(&[0xB3, b'n'], 0.35, 0.5),
        t(b" hola", 0.6, 0.8),
        t(b",", 0.8, 0.82),
    ]);
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].text, "canción");
    assert_eq!((w[0].start, w[0].end), (0.0, 0.5));
    assert_eq!(w[1].text, "hola,");
}

#[test]
fn empty_and_blank() {
    assert!(group(&[]).is_empty());
    assert!(group(&[t(b" ", 0.0, 0.1)]).is_empty());
}

/// A word's weakest piece is its confidence; the mean is kept beside it.
#[test]
fn a_word_keeps_its_lowest_token_probability() {
    let w = group(&[
        tp(b" Tsun", 0.0, 0.2, 0.9),
        tp(b"he", 0.2, 0.3, 0.2),
        tp(b"to", 0.3, 0.4, 0.7),
        tp(b" dijo", 0.5, 0.8, 0.95),
    ]);
    assert_eq!(w[0].p_min, Some(0.2));
    assert!((w[0].p - 0.6).abs() < 1e-6, "{}", w[0].p);
    assert_eq!(w[1].p_min, Some(0.95));
    // An unsure comma does not make its word unsure; a word of punctuation alone keeps its own.
    let w = group(&[
        tp(b" hola", 0.0, 0.2, 0.9),
        tp(b",", 0.2, 0.25, 0.1),
        tp(b" ...", 0.3, 0.4, 0.3),
    ]);
    assert_eq!(w[0].p_min, Some(0.9));
    assert_eq!(w[1].p_min, Some(0.3));
    // A letter split across tokens counts as a letter in each ("ó" is C3 B3).
    let w = group(&[
        tp(b" canci", 0.0, 0.3, 0.9),
        tp(&[0xC3], 0.3, 0.35, 0.4),
        tp(&[0xB3, b'n'], 0.35, 0.5, 0.8),
    ]);
    assert_eq!(w[0].text, "canción");
    assert_eq!(w[0].p_min, Some(0.4));
    let w = group(&[
        tp(b" canci", 0.0, 0.3, 0.9),
        tp(&[0xC3, 0xB3], 0.3, 0.35, 0.8),
        tp(b"n", 0.35, 0.5, 0.3),
    ]);
    assert_eq!(w[0].p_min, Some(0.3));
}

/// Punctuation past ASCII (common in Spanish) is not a letter either, whole or split.
#[test]
fn unsure_spanish_punctuation_does_not_flag_its_word() {
    let open_q = "¿".as_bytes(); // C2 BF
    let dash = "—".as_bytes(); // E2 80 94
    let dots = "…".as_bytes(); // E2 80 A6
    let quote = "”".as_bytes(); // E2 80 9D
    let w = group(&[
        tp(&[b' ', open_q[0], open_q[1]], 0.0, 0.1, 0.05),
        tp(b"por", 0.1, 0.3, 0.9),
        tp(dash, 0.3, 0.35, 0.1),
        tp(b" bien", 0.4, 0.6, 0.8),
        tp(dots, 0.6, 0.65, 0.02),
        tp(b" dijo", 0.7, 0.9, 0.95),
        tp(&quote[..2], 0.9, 0.92, 0.1),
        tp(&quote[2..], 0.92, 0.95, 0.1),
    ]);
    assert_eq!(w[0].text, "¿por—");
    assert_eq!(w[0].p_min, Some(0.9));
    assert_eq!(w[1].p_min, Some(0.8));
    assert_eq!(w[2].text, "dijo”");
    assert_eq!(w[2].p_min, Some(0.95));
    // A dash alone is its own confidence.
    let w = group(&[tp(&[b' ', dash[0], dash[1], dash[2]], 0.0, 0.1, 0.2)]);
    assert_eq!(w[0].p_min, Some(0.2));
}

/// A word without a lowest probability reads with none, and writes without the field.
#[test]
fn an_older_word_reads_without_a_lowest_probability() {
    let old: Word =
        serde_json::from_str(r#"{"start":1.0,"end":1.5,"text":"hola","p":0.8}"#).unwrap();
    assert_eq!(old.p_min, None);
    assert_eq!(
        serde_json::to_string(&old).unwrap(),
        r#"{"start":1.0,"end":1.5,"text":"hola","p":0.8}"#
    );
    let new = Word {
        p_min: Some(0.25),
        ..old
    };
    let back: Word = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
    assert_eq!(back.p_min, Some(0.25));
}
