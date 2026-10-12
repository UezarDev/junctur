//! The transcript as the editor sees it: every word of every track in one list, sorted by start, so
//! the frontend and the editor name a word by the same index.

use std::path::Path;

use junctur_core::edit::{snap_boundary, CutSettings, Envelope};
use junctur_core::transcribe::{self, Word};
use serde::Serialize;

#[derive(Debug, Default)]
pub struct Words(pub Vec<Word>);

impl Words {
    /// The project's `transcript.json`, or `None` when there is none (or it does not read).
    pub fn load(project_dir: &Path) -> Option<Self> {
        let t = transcribe::load(project_dir)?;
        Some(Self::from_words(t.words().cloned().collect()))
    }

    pub fn from_words(mut words: Vec<Word>) -> Self {
        // Stable: words of one chunk keep their order when two start together.
        words.sort_by(|a, b| a.start.total_cmp(&b.start));
        Self(words)
    }

    /// The source span that cutting (`keep` false) or keeping words `first..=last` changes: from a
    /// boundary in the pause before the first word to one in the pause after the last, each placed
    /// as the reduction places its own ([`snap_boundary`]). A missing neighbour (the first or last
    /// word of the recording) counts as a word ending or starting both pads away. `None` for an
    /// index out of range.
    pub fn span(
        &self,
        first: usize,
        last: usize,
        keep: bool,
        env: &Envelope,
        s: &CutSettings,
        duration: f64,
    ) -> Option<(f64, f64)> {
        let (first, last) = (first.min(last), first.max(last));
        let w = &self.0;
        let (a, b) = (w.get(first)?, w.get(last)?);
        let pads = s.pad_before + s.pad_after;
        let before = match first.checked_sub(1) {
            Some(i) => w[i].end,
            None => (a.start - pads).max(0.0),
        };
        let after = w
            .get(last + 1)
            .map_or((b.end + pads).min(duration), |n| n.start);
        // A cut ends kept speech before it; a keep starts kept speech.
        let start = snap_boundary(env, (before, a.start), !keep, s);
        let end = snap_boundary(env, (b.end, after), keep, s);
        Some((start, end.max(start)))
    }
}

/// The words for the frontend, compact for a 30 h transcript (hundreds of thousands of words):
/// texts joined by `\n` (a word never holds one), times in whole milliseconds.
#[derive(Debug, Serialize)]
pub struct TranscriptView {
    pub text: String,
    pub start: Vec<u64>,
    pub end: Vec<u64>,
}

impl TranscriptView {
    pub fn new(words: &Words) -> Self {
        let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
        let texts: Vec<&str> = words.0.iter().map(|w| w.text.trim()).collect();
        Self {
            text: texts.join("\n"),
            start: words.0.iter().map(|w| ms(w.start)).collect(),
            end: words.0.iter().map(|w| ms(w.end)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, end: f64, text: &str) -> Word {
        Word {
            start,
            end,
            text: text.into(),
            p: 1.0,
        }
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
        let w = sample();
        let v = TranscriptView::new(&w);
        assert_eq!(v.text, "uno\ndos\ntres");
        assert_eq!(v.start, vec![10_000, 10_500, 12_000]);
        assert_eq!(v.end, vec![10_400, 11_000, 12_500]);
    }

    #[test]
    fn spans_stay_in_the_pauses() {
        let w = sample();
        let s = CutSettings::default();
        let env = Envelope::default();
        // Cut "dos": from the pause after "uno" (only 0.1 s, so its far edge) to 0.25 s before "tres".
        let (a, b) = w.span(1, 1, false, &env, &s, 60.0).unwrap();
        assert!(
            (a - 10.5).abs() < 1e-9 && (b - 11.75).abs() < 1e-9,
            "{a} {b}"
        );
        // Keep "dos": starts in the same pause, ends 0.35 s after it.
        let (a, b) = w.span(1, 1, true, &env, &s, 60.0).unwrap();
        assert!(
            (a - 10.4).abs() < 1e-9 && (b - 11.35).abs() < 1e-9,
            "{a} {b}"
        );
        // The first and last words: neighbours both pads away, within the recording.
        let (a, b) = w.span(0, 2, false, &env, &s, 12.6).unwrap();
        assert!(a > 9.3 && a < 10.0 && (12.5..=12.6).contains(&b), "{a} {b}");
        // Reversed indices are the same span; out of range is none.
        assert_eq!(w.span(2, 0, false, &env, &s, 12.6), Some((a, b)));
        assert_eq!(w.span(0, 3, false, &env, &s, 60.0), None);
    }
}
