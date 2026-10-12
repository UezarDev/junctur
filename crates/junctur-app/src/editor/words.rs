//! The transcript as the editor sees it: every word of every track in one list, sorted by start, so
//! the frontend and the editor name a word by the same index. Each word keeps the track it was heard
//! on, which is what names its speaker when speakers are tracks (5a).

use std::path::Path;

use junctur_core::edit::{word_span, CutSettings, Envelope, Speakers};
use junctur_core::transcribe::{self, Word};
use serde::Serialize;

#[derive(Debug, Default)]
pub struct Words {
    pub list: Vec<Word>,
    /// The audio track (0-based ordinal) each word was heard on.
    pub tracks: Vec<usize>,
    /// The latest end among words `0..=i`. With several tracks a long word can outlast the words
    /// that start after it, so speech up to word `i` ends here, not at word `i`'s own end.
    ends_by: Vec<f64>,
}

impl Words {
    /// The project's `transcript.json`, or `None` when there is none (or it does not read).
    pub fn load(project_dir: &Path) -> Option<Self> {
        let t = transcribe::load(project_dir)?;
        let words = t
            .segments
            .iter()
            .flat_map(|s| s.words.iter().map(|w| (s.track, w.clone())))
            .collect();
        Some(Self::from_tracked(words))
    }

    /// Words all heard on track 0.
    #[cfg(test)]
    pub fn from_words(list: Vec<Word>) -> Self {
        Self::from_tracked(list.into_iter().map(|w| (0, w)).collect())
    }

    pub fn from_tracked(mut words: Vec<(usize, Word)>) -> Self {
        // Stable: words of one chunk keep their order when two start together.
        words.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
        let (tracks, list): (Vec<usize>, Vec<Word>) = words.into_iter().unzip();
        let ends_by = list
            .iter()
            .scan(f64::NEG_INFINITY, |m, w| {
                *m = m.max(w.end);
                Some(*m)
            })
            .collect();
        Self {
            list,
            tracks,
            ends_by,
        }
    }

    /// The source span that cutting (`keep` false) or keeping words `first..=last` changes, placed
    /// by [`word_span`]: speech before the run ends at the latest end of any earlier word (any
    /// track), the run lasts until the latest end up to its last word, and speech after it starts
    /// with the next word (sorted by start, so the earliest). `None` for an index out of range.
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
        let start = self.list.get(first)?.start;
        let end = *self.ends_by.get(last)?;
        let before = first.checked_sub(1).map(|i| self.ends_by[i]);
        let after = self.list.get(last + 1).map(|w| w.start);
        Some(word_span(
            before,
            (start, end),
            after,
            keep,
            env,
            s,
            duration,
        ))
    }
}

/// The words for the frontend, compact for a 30 h transcript (hundreds of thousands of words):
/// times in whole milliseconds, and each word's speaker id (0: none) from the edit list's turns.
#[derive(Debug, Serialize)]
pub struct TranscriptView {
    pub text: Vec<String>,
    pub start: Vec<u64>,
    pub end: Vec<u64>,
    pub speaker: Vec<u32>,
}

impl TranscriptView {
    pub fn new(words: &Words, speakers: &Speakers) -> Self {
        let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
        let w = &words.list;
        let label = speakers.labeller();
        Self {
            text: w.iter().map(|w| w.text.trim().to_string()).collect(),
            start: w.iter().map(|w| ms(w.start)).collect(),
            end: w.iter().map(|w| ms(w.end)).collect(),
            speaker: w
                .iter()
                .zip(&words.tracks)
                .map(|(w, &t)| label.of_word(t, w.start, w.end).unwrap_or(0))
                .collect(),
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
        let v = TranscriptView::new(&sample(), &Speakers::default());
        assert_eq!(v.text, vec!["uno", "dos", "tres"]);
        assert_eq!(v.start, vec![10_000, 10_500, 12_000]);
        assert_eq!(v.end, vec![10_400, 11_000, 12_500]);
        assert_eq!(v.speaker, vec![0, 0, 0]);
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
            TranscriptView::new(&sample(), &voices).speaker,
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
        assert_eq!(TranscriptView::new(&w, &tracks).speaker, vec![2, 1]);
    }

    #[test]
    fn spans_stay_in_the_pauses() {
        let w = sample();
        let s = CutSettings::default();
        let env = Envelope::default();
        // Cut "dos": from the pause after "uno" (only 0.1 s, so its far edge) to 0.25 s before
        // "tres".
        let (a, b) = w.span(1, 1, false, &env, &s, 60.0).unwrap();
        assert!(
            (a - 10.5).abs() < 1e-9 && (b - 11.75).abs() < 1e-9,
            "{a} {b}"
        );
        let (a, b) = w.span(1, 1, true, &env, &s, 60.0).unwrap();
        assert!(
            (a - 10.4).abs() < 1e-9 && (b - 11.35).abs() < 1e-9,
            "{a} {b}"
        );
        // Reversed indices are the same span; out of range is none.
        assert_eq!(
            w.span(2, 1, false, &env, &s, 60.0),
            w.span(1, 2, false, &env, &s, 60.0)
        );
        assert_eq!(w.span(0, 3, false, &env, &s, 60.0), None);
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
        let (a, b) = w.span(2, 2, false, &Envelope::default(), &s, 60.0).unwrap();
        assert_eq!(
            a, 12.0,
            "no pause before: the selection's own edge, said as such"
        );
        // The trailing boundary goes in the real pause after "long" ends, not inside it.
        assert!((15.0..=16.0).contains(&b), "{b}");
    }
}
