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
    let languages = Spoken::new(segments);
    for s in sentences {
        if !seen.insert(s.text.as_str()) {
            continue;
        }
        match languages.at(s.track, s.start) {
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
    let shared = Batches {
        cache_dir,
        project,
        to,
        total,
        progress,
        stop,
    };
    let result = translate(&mut cache, &work, &mut done, shared);
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

/// What every batch of a [`run`] shares: where the model and the translations are kept, the
/// language translated into, how many sentences the run has, and its progress and stop.
struct Batches<'a> {
    cache_dir: &'a Path,
    project: &'a Path,
    to: &'a str,
    total: usize,
    progress: &'a mut dyn FnMut(usize, usize),
    stop: &'a AtomicBool,
}

/// The batches of [`run`] over `work`, into `cache`; each row the model failed is tried again
/// alone, with the loop guard, after the batches.
fn translate(
    cache: &mut Translations,
    work: &[(&str, &str)],
    done: &mut Done,
    shared: Batches,
) -> Result<()> {
    let Batches {
        cache_dir,
        project,
        to,
        total,
        progress,
        stop,
    } = shared;
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

/// [`spoken`] for many sentences: each track's segments in transcript order, answered by binary
/// search when their starts and ends never go down (a transcript's normally do not), else by
/// scanning that track's segments.
pub struct Spoken<'a> {
    tracks: std::collections::HashMap<usize, Track<'a>>,
}

struct Track<'a> {
    segments: Vec<&'a Segment>,
    sorted: bool,
}

impl<'a> Spoken<'a> {
    pub fn new(segments: &'a [Segment]) -> Self {
        let mut tracks: std::collections::HashMap<usize, Track> = Default::default();
        for s in segments {
            tracks
                .entry(s.track)
                .or_insert_with(|| Track {
                    segments: Vec::new(),
                    sorted: true,
                })
                .segments
                .push(s);
        }
        for t in tracks.values_mut() {
            t.sorted = t
                .segments
                .windows(2)
                .all(|w| w[0].start <= w[1].start && w[0].end <= w[1].end)
                && t.segments
                    .iter()
                    .all(|s| !s.start.is_nan() && !s.end.is_nan());
        }
        Self { tracks }
    }

    /// What [`spoken`] says for `track` at `t`.
    pub fn at(&self, track: usize, t: f64) -> Option<&'a str> {
        let tr = self.tracks.get(&track)?;
        let seg = &tr.segments;
        if !tr.sorted || t.is_nan() {
            return seg
                .iter()
                .min_by(|a, b| distance(a, t).total_cmp(&distance(b, t)))
                .map(|s| s.lang.as_str());
        }
        // Before `p` every segment starts at or before `t`, so is `max(0, t - end)` away, which
        // never grows with the index: the least is the last one's, first reached at `q`. From
        // `p` on the distance never shrinks, so `p` is the nearest there. Ties go to the earlier.
        let p = seg.partition_point(|s| s.start <= t);
        let before = p.checked_sub(1).map(|last| {
            let d = distance(seg[last], t);
            (seg[..p].partition_point(|s| distance(s, t) > d), d)
        });
        let after = seg.get(p).map(|s| (p, distance(s, t)));
        let best = match (before, after) {
            (Some(b), Some(a)) => {
                if a.1 < b.1 {
                    a
                } else {
                    b
                }
            }
            (b, a) => b.or(a)?,
        };
        Some(seg[best.0].lang.as_str())
    }
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
mod tests;
