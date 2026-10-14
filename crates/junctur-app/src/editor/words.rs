//! The transcript as the editor sees it: every word of every track in one list, sorted by start, so
//! the frontend and the editor name a word by the same index. Each word keeps the track it was heard
//! on, which is what names its speaker when speakers are tracks (5a).

use std::path::Path;

use junctur_core::edit::{word_span, CutSettings, Envelope, Fix, Fixes, Speakers};
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
    /// The words so far of a recording still transcribing (2j): every word before this many
    /// seconds is here, and words not yet transcribed start at or after it.
    until: Option<f64>,
}

impl Words {
    /// The project's `transcript.json`, or `None` when there is none (or it does not read).
    pub fn load(project_dir: &Path) -> Option<Self> {
        let t = transcribe::load(project_dir)?;
        Some(Self::from_segments(&t.segments))
    }

    /// The words so far of a recording still transcribing (2j): those of the run's finished chunks
    /// that start before `through`, where every track is done. Later words a track already has are
    /// left for the piece they belong to: every word still to come starts at or after `through`,
    /// so the next read only adds words at the end and the indices the page holds stay valid. With
    /// no run's chunks yet, none: never the `transcript.json` of an earlier run, whose words the
    /// new ones would replace under those indices (2j repair). The run's end reads the finished
    /// transcript ([`Self::load`]).
    pub fn load_so_far(project_dir: &Path, through: f64) -> Self {
        let segs = transcribe::so_far(project_dir).unwrap_or_default();
        let mut w = Self::from_segments(&segs);
        let n = w.list.partition_point(|x| x.start < through);
        w.list.truncate(n);
        w.tracks.truncate(n);
        w.ends_by.truncate(n);
        w.until = Some(through);
        w
    }

    fn from_segments(segs: &[transcribe::Segment]) -> Self {
        let words = segs
            .iter()
            .flat_map(|s| s.words.iter().map(|w| (s.track, w.clone())))
            .collect();
        Self::from_tracked(words)
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
            until: None,
        }
    }

    /// The source seconds words `first..=last` are said in: the first one's start to the latest
    /// end up to the last (5b: a voice is enrolled from this). `None` for an index out of range.
    pub fn stretch(&self, first: usize, last: usize) -> Option<(f64, f64)> {
        let (first, last) = (first.min(last), first.max(last));
        Some((self.list.get(first)?.start, *self.ends_by.get(last)?))
    }

    /// The source span that cutting (`keep` false) or keeping words `first..=last` changes, placed
    /// by [`word_span`]: speech before the run ends at the latest end of any earlier word (any
    /// track), the run lasts until the latest end up to its last word, and speech after it starts
    /// with the next word (sorted by start, so the earliest; for the last word so far of a
    /// recording still transcribing, where the transcript stops, 2j: what follows is not known
    /// yet, so the span never reaches past it). `None` for an index out of range.
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
        // A word started before the transcript stops may end after it.
        let after = self.list.get(last + 1).map(|w| w.start);
        let after = after.or(self.until.map(|u| u.max(end)));
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
/// times in whole milliseconds, each word's speaker id (0: none) from the edit list's turns, and its
/// text as the user fixed it (5h), else as heard.
#[derive(Debug, Serialize)]
pub struct TranscriptView {
    pub text: Vec<String>,
    pub start: Vec<u64>,
    pub end: Vec<u64>,
    pub speaker: Vec<u32>,
    /// Someone else who says the word too (5b: two people talking at once), 0 for none.
    pub also: Vec<u32>,
    /// How sure Whisper was of the word (5o): its lowest token probability in whole percent, rounded
    /// down, which the panel flags under the user's threshold. [`NOT_FLAGGED`] when there is nothing
    /// to flag it by: a transcript from before 5o, or a word the user fixed or confirmed (5h's fix
    /// lines). The threshold is the page's to apply, so changing it needs no new words.
    pub sure: Vec<u8>,
}

/// [`TranscriptView::sure`] of a word that is never flagged, above every percentage. The page has
/// the same number as `NOT_FLAGGED` (`ui/src/lib/transcript.ts`).
pub const NOT_FLAGGED: u8 = u8::MAX;

/// `p` (0..1) in whole percent, rounded down, so a word at 39.9 % is under a threshold of 40. In
/// f64 with a hair added (repair): `0.35_f32` is 0.34999999 and must read as 35, not 34, while a real
/// 34.99 % stays 34.
fn percent(p: f32) -> u8 {
    (f64::from(p.clamp(0.0, 1.0)) * 100.0 + 1e-4).floor() as u8
}

impl TranscriptView {
    pub fn new(words: &Words, speakers: &Speakers, fixes: &[Fix]) -> Self {
        let ms = |s: f64| (s.max(0.0) * 1000.0).round() as u64;
        let fixed = Fixes::new(fixes);
        let w = &words.list;
        let label = speakers.labeller();
        let (speaker, also) = w
            .iter()
            .zip(&words.tracks)
            .map(|(w, &t)| {
                let all = label.speakers_of_word(t, w.start, w.end);
                (
                    all.first().copied().unwrap_or(0),
                    all.get(1).copied().unwrap_or(0),
                )
            })
            .unzip();
        let sure = w
            .iter()
            .zip(&words.tracks)
            .map(|(w, &t)| match w.p_min {
                Some(p) if fixed.of(t, w).is_none() => percent(p),
                _ => NOT_FLAGGED,
            })
            .collect();
        Self {
            text: w
                .iter()
                .zip(&words.tracks)
                .map(|(w, &t)| fixed.text(t, w).trim().to_string())
                .collect(),
            start: w.iter().map(|w| ms(w.start)).collect(),
            end: w.iter().map(|w| ms(w.end)).collect(),
            speaker,
            also,
            sure,
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
            .span(3, 3, false, &Envelope::default(), &s, 1000.0)
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
        let v = TranscriptView::new(&sample(), &Speakers::default(), &[]);
        assert_eq!(v.text, vec!["uno", "dos", "tres"]);
        assert_eq!(v.start, vec![10_000, 10_500, 12_000]);
        assert_eq!(v.end, vec![10_400, 11_000, 12_500]);
        assert_eq!(v.speaker, vec![0, 0, 0]);
        // A fixed word shows as fixed (5h).
        let fix = Fix {
            track: 0,
            start: 10.5,
            text: "Uezar".into(),
        };
        let v = TranscriptView::new(&sample(), &Speakers::default(), &[fix]);
        assert_eq!(v.text, vec!["uno", "Uezar", "tres"]);
    }

    /// 5o: each word's confidence in percent, none for a word from an older transcript or one the
    /// user fixed or confirmed (a fix line with the heard text).
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
        let v = TranscriptView::new(&w, &none, &[]);
        assert_eq!(v.sure, vec![39, 100, NOT_FLAGGED, 5, 20]);
        let fix = |start: f64, text: &str| Fix {
            track: 0,
            start,
            text: text.into(),
        };
        // "Tsunejo" fixed, "bien" confirmed (its own text): neither is flagged any more.
        let v = TranscriptView::new(&w, &none, &[fix(11.5, "Tsunheto"), fix(12.0, "bien")]);
        assert_eq!(v.sure, vec![39, 100, NOT_FLAGGED, NOT_FLAGGED, NOT_FLAGGED]);
        assert_eq!(v.text[3..], ["Tsunheto", "bien"]);
    }

    /// Repair: a probability of exactly k % reads as k, so it is not under a threshold of k.
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
            TranscriptView::new(&sample(), &voices, &[]).speaker,
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
        assert_eq!(TranscriptView::new(&w, &tracks, &[]).speaker, vec![2, 1]);
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
