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
    let mut cur: Option<(Vec<u8>, f64, f64, f32, u32)> = None;
    for t in tokens {
        let opens = t.bytes.first() == Some(&b' ');
        match cur.as_mut() {
            Some(w) if !opens => {
                w.0.extend(&t.bytes);
                w.2 = w.2.max(t.end);
                w.3 += t.p;
                w.4 += 1;
            }
            _ => {
                if let Some(w) = cur.take() {
                    push(&mut words, w);
                }
                cur = Some((t.bytes.clone(), t.start, t.end, t.p, 1));
            }
        }
    }
    if let Some(w) = cur {
        push(&mut words, w);
    }
    words
}

fn push(words: &mut Vec<Word>, (bytes, start, end, p, n): (Vec<u8>, f64, f64, f32, u32)) {
    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    if !text.is_empty() {
        words.push(Word {
            start,
            end: end.max(start),
            text,
            p: p / n as f32,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &[u8], a: f64, b: f64) -> Token {
        Token {
            bytes: s.to_vec(),
            start: a,
            end: b,
            p: 1.0,
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
}
