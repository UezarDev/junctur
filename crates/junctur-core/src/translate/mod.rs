//! Captions translated on this machine, a caption sentence at a time over the time it was said, so
//! timing, speakers and cuts carry over (not the audio: cuts are made on the transcript's words).
//! Made ahead in [`run`] and kept in [`cache`]; cue cutting only looks them up, never waiting.

mod cache;
mod languages;
pub mod model;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use cache::{path, Translations};
pub use languages::{from_whisper, known, LANGUAGES};

use crate::runtime::DevicePref;
use crate::transcribe::Segment;
use crate::{Error, Result};

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
    /// Sentences whose language could not be told: not kept, tried again by a later run.
    pub unknown: usize,
    /// Sentences the model gave nothing usable for, twice: not kept, tried again by a later run.
    pub failed: usize,
    /// Stopped before the end; what was done is saved.
    pub cancelled: bool,
}

/// Saved this often while translating, so a cancel or a crash keeps the work and the app's
/// preview picks it up.
const SAVE_EVERY: Duration = Duration::from_secs(3);

/// Sentences decoded together ([`model::Model::translate_batch`]): on CPU about four times faster a
/// sentence than one by one.
const BATCH: usize = 8;

/// Translates into `to` every sentence the project's translations lack (or hold from another
/// language than `segments` now say it was heard in), saving as it goes, even when the model fails.
/// `stop` ends it after the batch in hand; sentences of unknown language or failed twice are left
/// for a later run ([`Done::unknown`], [`Done::failed`]). An unreadable translations file errors.
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
        return Err(Error::Edit(format!(
            "captions cannot be translated into {to:?}"
        )));
    }
    if segments.is_empty() && !sentences.is_empty() {
        // The transcript read as nothing (a piece being written while it transcribes): every
        // sentence's language would be unknown.
        return Err(Error::Edit(
            "the transcript's languages could not be read; try again".into(),
        ));
    }
    let mut cache = Translations::read(project)?;
    let mut done = Done::default();
    // Each sentence still needing it once, with the language it was said in.
    let mut seen = std::collections::HashSet::new();
    let mut todo: Vec<(&str, &str)> = Vec::new();
    for s in sentences {
        if !seen.insert(s.text.as_str()) {
            continue;
        }
        match spoken(segments, s.track, s.start) {
            None if cache.get(to, &s.text).is_none() => done.unknown += 1,
            None => {}
            Some(heard) if cache.needs(heard, to, &s.text) => todo.push((s.text.as_str(), heard)),
            Some(_) => {}
        }
    }
    let total = todo.len();
    progress(0, total);
    // Already in the target language, or in one the model lacks: shown as said.
    let mut work: Vec<(&str, &str)> = Vec::new();
    for &(text, heard) in &todo {
        match from_whisper(heard).filter(|f| *f != to) {
            Some(from) => work.push((text, from)),
            None => {
                cache.insert(heard, to, text, text);
                done.kept += 1;
            }
        }
    }
    if done.kept > 0 {
        progress(done.kept, total);
    }
    // Like lengths together, so a batch pads little.
    work.sort_by_key(|(text, _)| text.len());
    let result = translate(
        &mut cache, cache_dir, project, to, &work, &mut done, total, progress, stop,
    );
    if total > 0 {
        cache.save(project)?;
    }
    match result {
        Err(Error::Cancelled) => {
            done.cancelled = true;
            Ok(done)
        }
        r => r.map(|()| done),
    }
}

/// The batches of [`run`], into `cache`; each row the model failed is tried again alone, with
/// the loop guard, after the batches.
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
    let mut again: Vec<(&str, &str)> = Vec::new();
    // Each batch and whether it is a second try.
    let mut batches: Vec<(Vec<(&str, &str)>, bool)> =
        work.chunks(BATCH).map(|b| (b.to_vec(), false)).collect();
    let mut i = 0;
    while i < batches.len() {
        if stop.load(Ordering::Relaxed) {
            done.cancelled = true;
            return Ok(());
        }
        let m = match &mut model {
            Some(m) => m,
            None => model.insert(model::Model::open(cache_dir, DevicePref::Cpu, stop)?),
        };
        let (batch, guard) = &batches[i];
        let shown = m.translate_batch(batch, to, *guard)?;
        for (&(text, from), shown) in batch.iter().zip(shown) {
            match shown {
                Some(t) => {
                    cache.insert(from, to, text, &t);
                    done.translated += 1;
                }
                None if !guard => again.push((text, from)),
                None => done.failed += 1,
            }
        }
        if saved.elapsed() >= SAVE_EVERY {
            // A failed save is tried again at the next and at the end; the run goes on.
            if let Err(e) = cache.save(project) {
                eprintln!("junctur: saving the translations: {e}");
            }
            saved = Instant::now();
        }
        progress(done.kept + done.translated + done.failed, total);
        i += 1;
        if i == batches.len() {
            batches.extend(again.drain(..).map(|w| (vec![w], true)));
        }
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
            // No segment on its track: its language is not known, so it is not kept.
            Sentence {
                track: 1,
                start: 2.0,
                text: "Who?".into(),
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
                kept: 2,
                unknown: 1,
                ..Done::default()
            }
        );
        assert_eq!(seen, [(0, 2), (2, 2)]);
        let t = Translations::load(&d);
        assert_eq!(t.get("en", "Aloha."), Some("Aloha."));
        assert_eq!(t.get("en", "Who?"), None);
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
        assert_eq!(
            again,
            Done {
                unknown: 1,
                ..Done::default()
            }
        );
        // A transcript that read as nothing is an error, not every sentence kept as said.
        assert!(run(
            &d,
            &none,
            "fr",
            &sentences,
            &[],
            &mut |_, _| {},
            &AtomicBool::new(false)
        )
        .is_err());
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
        // A translations file that does not read is an error, and left as it is.
        std::fs::write(d.join(cache::FILE), "{").unwrap();
        assert!(run(
            &d,
            &none,
            "en",
            &sentences,
            &segments,
            &mut |_, _| {},
            &AtomicBool::new(false)
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(d.join(cache::FILE)).unwrap(), "{");
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
