//! Captions in another language than the one spoken (5k), made on this machine.
//!
//! What is translated is caption text, a sentence at a time ([`crate::export::srt::sentences`]: the
//! kept words of one speaker up to a sentence end, a pause or a word limit), never the audio and
//! never the transcript: the translated words are spread over the time their sentence was said, so
//! timing, speakers and cuts carry over, and the transcript and the edit list keep the words as
//! spoken. Rejected: Whisper's own translate task, which only writes English and is a decoding of
//! the audio, so it would mean transcribing the recording a second time, its words not the
//! transcript's ones the cuts are made on.
//!
//! One local model for every pair of languages, run through ONNX Runtime like the speech models
//! ([`model`]), fetched on first use. Translations are kept in the project folder ([`cache`]) and
//! made in [`run`], which the CLI calls before writing captions and the app runs in the background;
//! cue cutting only looks them up, so the preview never waits on the model.

mod cache;
mod languages;
pub mod model;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use cache::Translations;
pub use languages::{from_whisper, known, LANGUAGES};

use crate::runtime::DevicePref;
use crate::transcribe::Segment;
use crate::Result;

/// A caption sentence: the kept words of one speaker on one track, as captions show them
/// untranslated.
#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    pub track: usize,
    /// Source seconds of its first word, which finds the language it was said in.
    pub start: f64,
    pub text: String,
}

/// What [`run`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Done {
    /// Sentences the model translated.
    pub translated: usize,
    /// Sentences kept as said: already in the target language, or in one the model lacks.
    pub kept: usize,
    /// Stopped before the end; what was done is saved.
    pub cancelled: bool,
}

/// Saved this often while translating, so a cancel or a crash keeps the work and the app's
/// preview picks it up.
const SAVE_EVERY: Duration = Duration::from_secs(3);

/// Sentences decoded together ([`model::Model::translate_batch`]): measured on CPU, 8 at a time
/// take 0.29 s a sentence against 1.21 s one by one.
const BATCH: usize = 8;

/// Translates into `to` every sentence of `sentences` the project's translations lack, saving them
/// in `project` as it goes. `segments` (the transcript's) say which language each was said in.
/// `progress` hears `(done, to do)` as sentences are done; `stop` set ends it after the batch in
/// hand. The model is opened only when a sentence needs it. What was done is saved even when the
/// model fails part way.
pub fn run(
    project: &Path,
    cache_dir: &Path,
    to: &str,
    sentences: &[Sentence],
    segments: &[Segment],
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<Done> {
    if !known(to) {
        return Err(crate::Error::Edit(format!(
            "captions cannot be translated into {to:?}"
        )));
    }
    let mut cache = Translations::load(project);
    // Each missing sentence once, with the language it was said in.
    let mut seen = std::collections::HashSet::new();
    let todo: Vec<(&str, Option<&str>)> = sentences
        .iter()
        .filter(|s| cache.get(to, &s.text).is_none() && seen.insert(s.text.as_str()))
        .map(|s| {
            let from = spoken(segments, s.track, s.start).and_then(from_whisper);
            (s.text.as_str(), from.filter(|f| *f != to))
        })
        .collect();
    let total = todo.len();
    let mut done = Done::default();
    progress(0, total);
    // Already in the target language, or in one the model lacks: shown as said.
    for (text, _) in todo.iter().filter(|(_, from)| from.is_none()) {
        cache.insert(to, text, text);
        done.kept += 1;
    }
    if done.kept > 0 {
        progress(done.kept, total);
    }
    let mut work: Vec<(&str, &str)> = todo
        .iter()
        .filter_map(|&(text, from)| Some((text, from?)))
        .collect();
    // Like lengths together, so a batch pads little.
    work.sort_by_key(|(text, _)| text.len());
    let result = translate(
        &mut cache, cache_dir, project, to, &work, &mut done, total, progress, stop,
    );
    if total > 0 {
        cache.save(project)?;
    }
    result.map(|()| done)
}

/// The batches of [`run`], into `cache`.
#[allow(clippy::too_many_arguments)]
fn translate(
    cache: &mut Translations,
    cache_dir: &Path,
    project: &Path,
    to: &str,
    work: &[(&str, &str)],
    done: &mut Done,
    total: usize,
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<()> {
    let mut model: Option<model::Model> = None;
    let mut saved = Instant::now();
    for batch in work.chunks(BATCH) {
        if stop.load(Ordering::Relaxed) {
            done.cancelled = true;
            return Ok(());
        }
        let m = match &mut model {
            Some(m) => m,
            None => model.insert(model::Model::open(cache_dir, DevicePref::Cpu)?),
        };
        for (&(text, _), shown) in batch.iter().zip(m.translate_batch(batch, to)?) {
            // An empty translation is the model failing, not a sentence to hide: shown as said.
            let shown = if shown.is_empty() { text } else { &shown };
            cache.insert(to, text, shown);
            done.translated += 1;
        }
        if saved.elapsed() >= SAVE_EVERY {
            cache.save(project)?;
            saved = Instant::now();
        }
        progress(done.kept + done.translated, total);
    }
    Ok(())
}

/// The language a word at source second `t` on `track` was heard in: its segment's, else the
/// nearest segment's on that track.
pub fn spoken(segments: &[Segment], track: usize, t: f64) -> Option<&str> {
    segments
        .iter()
        .filter(|s| s.track == track)
        .min_by(|a, b| distance(a, t).total_cmp(&distance(b, t)))
        .map(|s| s.lang.as_str())
}

fn distance(s: &Segment, t: f64) -> f64 {
    if t < s.start {
        s.start - t
    } else if t > s.end {
        t - s.end
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(track: usize, start: f64, end: f64, lang: &str) -> Segment {
        Segment {
            track,
            start,
            end,
            lang: lang.into(),
            model: String::new(),
            words: Vec::new(),
        }
    }

    #[test]
    fn a_sentence_takes_its_segments_language() {
        let s = [
            seg(0, 0.0, 10.0, "es"),
            seg(0, 10.0, 20.0, "en"),
            seg(1, 0.0, 30.0, "fr"),
        ];
        assert_eq!(spoken(&s, 0, 5.0), Some("es"));
        assert_eq!(spoken(&s, 0, 15.0), Some("en"));
        assert_eq!(spoken(&s, 0, 25.0), Some("en"), "nearest");
        assert_eq!(spoken(&s, 1, 5.0), Some("fr"));
        assert_eq!(spoken(&s, 2, 5.0), None);
    }

    #[test]
    fn sentences_already_in_the_target_language_are_kept_without_the_model() {
        let d = std::env::temp_dir().join(format!("junctur-tr-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let segments = [seg(0, 0.0, 10.0, "en"), seg(0, 10.0, 20.0, "haw")];
        let sentences = [
            Sentence {
                track: 0,
                start: 1.0,
                text: "Hello there.".into(),
            },
            Sentence {
                track: 0,
                start: 12.0,
                text: "Aloha.".into(),
            },
            Sentence {
                track: 0,
                start: 2.0,
                text: "Hello there.".into(),
            },
        ];
        let mut seen = Vec::new();
        // A cache dir with no model in it: opening the model would fail the run.
        let none = d.join("no-models");
        let done = run(
            &d,
            &none,
            "en",
            &sentences,
            &segments,
            &mut |a, b| seen.push((a, b)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            done,
            Done {
                translated: 0,
                kept: 2,
                cancelled: false
            }
        );
        assert_eq!(seen, [(0, 2), (2, 2)]);
        let t = Translations::load(&d);
        assert_eq!(t.get("en", "Aloha."), Some("Aloha."));
        // Nothing left: a second run does nothing.
        let again = run(
            &d,
            &none,
            "en",
            &sentences,
            &segments,
            &mut |_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(again, Done::default());
        // Stopped before the first: nothing done, nothing opened.
        let stopped = run(
            &d,
            &none,
            "fr",
            &sentences,
            &segments,
            &mut |_, _| {},
            &AtomicBool::new(true),
        )
        .unwrap();
        assert!(stopped.cancelled && stopped.translated == 0);
        assert!(run(
            &d,
            &none,
            "xx",
            &[],
            &[],
            &mut |_, _| {},
            &AtomicBool::new(false)
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
