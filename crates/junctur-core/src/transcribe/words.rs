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
    /// The lowest probability of its tokens, 0..1, which the transcript panel flags a word by: a
    /// misheard name is often one unsure piece the mean averages away. `None` in older transcripts.
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
                    tokens: Vec::new(),
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
    /// Each token's bytes in [`Self::bytes`] (from, to) and its probability, for [`lowest`].
    tokens: Vec<(usize, usize, f32)>,
}

impl Piece {
    fn add(&mut self, t: &Token) {
        let from = self.bytes.len();
        self.bytes.extend(&t.bytes);
        self.tokens.push((from, self.bytes.len(), t.p));
        self.end = self.end.max(t.end);
        self.sum += t.p;
        self.n += 1;
    }

    fn push_to(self, words: &mut Vec<Word>) {
        let text = String::from_utf8_lossy(&self.bytes).trim().to_string();
        if text.is_empty() {
            return;
        }
        let low = lowest(&self.bytes, &self.tokens);
        words.push(Word {
            start: self.start,
            end: self.end.max(self.start),
            text,
            p: self.sum / self.n as f32,
            p_min: Some(low),
        });
    }
}

/// A word's confidence: the lowest probability among its tokens holding part of a letter or digit
/// (else among all), so an unsure comma or `¿` does not bury the misheard words. Letters are told
/// from decoded characters, and a character split across tokens belongs to each of them.
fn lowest(bytes: &[u8], tokens: &[(usize, usize, f32)]) -> f32 {
    // Per byte: part of a letter or digit. Bytes that do not decode are not.
    let mut letter = vec![false; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        let width = match bytes[i] {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        let char_at = bytes
            .get(i..i + width)
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|s| s.chars().next());
        match char_at {
            Some(c) => {
                letter[i..i + width].fill(c.is_alphanumeric());
                i += width;
            }
            None => i += 1,
        }
    }
    let low = |text_only: bool| {
        tokens
            .iter()
            .filter(|&&(a, b, _)| !text_only || letter[a..b].iter().any(|&l| l))
            .map(|t| t.2)
            .fold(f32::INFINITY, f32::min)
    };
    let text = low(true);
    if text.is_finite() {
        text
    } else {
        low(false)
    }
}

#[cfg(test)]
mod tests;
