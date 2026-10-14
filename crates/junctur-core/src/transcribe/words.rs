//! Whisper tokens to words. Tokens are sub-word byte pieces; a piece starting with a space opens a
//! new word. Bytes are joined before decoding because one accented letter can span two tokens.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Mean token probability, 0..1.
    pub p: f32,
    /// The lowest probability of its tokens, 0..1 (5o): how unsure Whisper was of its weakest
    /// piece, which is what the transcript panel flags a word by. A misheard name is often one
    /// unsure piece among sure ones, which the mean ([`Self::p`]) averages away. `None` in a
    /// transcript made before 5o, whose words are then never flagged; read without it, written only
    /// when set, so such a transcript still reads and is not taken for outdated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p_min: Option<f32>,
}

/// One text token: its bytes and its time span (seconds).
pub struct Token {
    pub bytes: Vec<u8>,
    pub start: f64,
    pub end: f64,
    pub p: f32,
}

pub fn group(tokens: &[Token]) -> Vec<Word> {
    let mut words = Vec::new();
    let mut cur: Option<Piece> = None;
    for t in tokens {
        let opens = t.bytes.first() == Some(&b' ');
        match cur.as_mut() {
            Some(w) if !opens => w.add(t),
            _ => {
                if let Some(w) = cur.take() {
                    w.push_to(&mut words);
                }
                let mut w = Piece {
                    bytes: Vec::new(),
                    start: t.start,
                    end: t.end,
                    sum: 0.0,
                    n: 0,
                    low_text: f32::INFINITY,
                    low: f32::INFINITY,
                };
                w.add(t);
                cur = Some(w);
            }
        }
    }
    if let Some(w) = cur {
        w.push_to(&mut words);
    }
    words
}

/// A word being joined from its tokens.
struct Piece {
    bytes: Vec<u8>,
    start: f64,
    end: f64,
    /// Summed and counted token probability, for the mean.
    sum: f32,
    n: u32,
    /// The lowest probability of a token holding part of a letter or digit (5o), and of any token.
    /// Whisper is often unsure whether a comma or a full stop follows a word it heard plainly
    /// ("hola" sure, its "," not); flagging the word for that would bury the misheard ones.
    low_text: f32,
    low: f32,
}

impl Piece {
    fn add(&mut self, t: &Token) {
        self.bytes.extend(&t.bytes);
        self.end = self.end.max(t.end);
        self.sum += t.p;
        self.n += 1;
        self.low = self.low.min(t.p);
        // A byte past ASCII is part of a letter (one accented letter can span two tokens).
        if t.bytes
            .iter()
            .any(|b| !b.is_ascii() || b.is_ascii_alphanumeric())
        {
            self.low_text = self.low_text.min(t.p);
        }
    }

    fn push_to(self, words: &mut Vec<Word>) {
        let text = String::from_utf8_lossy(&self.bytes).trim().to_string();
        if text.is_empty() {
            return;
        }
        let low = if self.low_text.is_finite() {
            self.low_text
        } else {
            self.low
        };
        words.push(Word {
            start: self.start,
            end: self.end.max(self.start),
            text,
            p: self.sum / self.n as f32,
            p_min: Some(low),
        });
    }
}

#[cfg(test)]
mod tests {
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

    /// 5o: a word's weakest piece is its confidence; the mean is kept beside it as before.
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
        // A letter split across tokens counts as a letter.
        let w = group(&[tp(b" canci", 0.0, 0.3, 0.9), tp(&[0xC3], 0.3, 0.35, 0.4)]);
        assert_eq!(w[0].p_min, Some(0.4));
    }

    /// A transcript from before 5o reads with no confidence, and one without it writes as before.
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
}
