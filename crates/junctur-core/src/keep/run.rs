//! One judging run of Laya's (3b; smart keep runs [`super::rules`] now): every part's state built, the parts whose state, question and model were judged
//! before taken from `keep.json`, and only the rest given to Laya.

use std::collections::HashMap;
use std::path::Path;

use super::fit::Fit;
use super::laya::{self, Laya, Vocab};
use super::parts::{self, Part};
use super::state::{self, Context, Spoken};
use super::{read, save, Judged, Judgement};
use crate::edit::{Speakers, Window};
use crate::hash::Fnv;
use crate::labels::Labels;
use crate::runtime::DevicePref;
use crate::transcribe::Transcript;
use crate::vad::Region;
use crate::{models, Result};

/// What a run judges.
pub struct Job<'a> {
    /// The project folder.
    pub dir: &'a Path,
    pub cache_dir: &'a Path,
    pub prompt: &'a str,
    /// Every speech track's regions together.
    pub speech: &'a [Region],
    pub duration: f64,
    pub transcript: &'a Transcript,
    /// Who speaks when (the edit list's turns and names).
    pub speakers: &'a Speakers,
    pub labels: Option<&'a Labels>,
    pub device: DevicePref,
    /// Judge every part again (`--force`).
    pub force: bool,
}

/// Judges `job`'s recording and writes `keep.json`. The model is opened only when some part was not
/// judged before with this state.
pub fn run(job: &Job, log: &mut dyn FnMut(&str)) -> Result<Judged> {
    let parts = parts::parts(job.speech, job.duration);
    let words = spoken(job.transcript, job.speakers);
    let c = Context {
        prompt: job.prompt,
        words: &words,
        labels: job.labels,
    };
    let q = state::question();
    // Read even when every part is reused: a state is fitted to Laya's budget in tokens.
    let vocab = Vocab::shared(job.cache_dir)?;
    let head = vocab.head(&q)?;
    let fits = |s: &str| vocab.fits(&head, s).unwrap_or(true);
    let states: Vec<String> = parts.iter().map(|p| state::fitted(&c, p, &fits)).collect();
    let hashes: Vec<String> = states.iter().map(|s| hash(&q, s)).collect();
    let before: HashMap<String, f64> = read(job.dir)
        .filter(|_| !job.force)
        .filter(|j| j.model == laya::NAME && j.version == state::VERSION)
        .map(|j| j.parts.into_iter().map(|p| (p.hash, p.margin)).collect())
        .unwrap_or_default();
    let todo: Vec<usize> = (0..parts.len())
        .filter(|&i| !before.contains_key(&hashes[i]))
        .collect();
    let mut fresh: HashMap<usize, f64> = HashMap::new();
    if todo.is_empty() {
        log(&format!(
            "keep: all {} parts judged before, reused",
            parts.len()
        ));
    } else {
        log(&format!(
            "keep: {} of {} parts to judge",
            todo.len(),
            parts.len()
        ));
        let opened = std::time::Instant::now();
        let mut model = Laya::open(job.cache_dir, &vocab, job.device)?;
        log(&format!(
            "keep: Laya on {:?}, opened in {:.1} s",
            model.device,
            opened.elapsed().as_secs_f64()
        ));
        let judging = std::time::Instant::now();
        let rows = todo
            .iter()
            .map(|&i| vocab.sequence(&head, &states[i]))
            .collect::<Result<Vec<_>>>()?;
        let total = rows.len();
        let mut told = 0;
        let margins = model.margins(&rows, &mut |done| {
            let tenth = done * 10 / total.max(1);
            if tenth > told {
                told = tenth;
                log(&format!("keep: {}% judged", tenth * 10));
            }
        })?;
        log(&format!(
            "keep: {total} parts judged in {:.1} s",
            judging.elapsed().as_secs_f64()
        ));
        fresh.extend(todo.iter().copied().zip(margins.into_iter().map(f64::from)));
    }
    let judged = Judged {
        model: laya::NAME.into(),
        prompt: job.prompt.into(),
        version: state::VERSION,
        parts: parts
            .iter()
            .enumerate()
            .map(|(i, p)| Judgement {
                start: p.start,
                end: p.end,
                kind: p.kind,
                margin: fresh.get(&i).copied().unwrap_or_else(|| before[&hashes[i]]),
                state: states[i].clone(),
                hash: hashes[i].clone(),
            })
            .collect(),
    };
    save(job.dir, &judged)?;
    Ok(judged)
}

/// The words in time order, each with its speaker's name (the track's number when nobody is near).
fn spoken(t: &Transcript, s: &Speakers) -> Vec<Spoken> {
    let who = s.labeller();
    t.in_order()
        .into_iter()
        .map(|(track, w)| Spoken {
            start: w.start,
            speaker: who
                .of_word(track, w.start, w.end)
                .and_then(|id| s.name(id))
                .map_or_else(|| format!("Track {}", track + 1), str::to_string),
            text: w.text.clone(),
        })
        .collect()
}

/// What a margin depends on: the model and tokenizer files (by their pins), the layout's limits, the
/// question and state's version and text.
fn hash(q: &laya::Question, state: &str) -> String {
    let mut h = Fnv::new();
    h.u64(state::VERSION as u64)
        .u64(laya::MAX_LEN as u64)
        .u64(laya::HEAD_MAX_LEN as u64)
        .u64(laya::OPTION_MAX as u64);
    let pins = [models::LAYA_MODEL.sha256, models::LAYA_TOKENIZER.sha256];
    for s in pins
        .into_iter()
        .chain([laya::NAME, &q.instructions, &q.keep, &q.cut, state])
    {
        h.bytes(s.as_bytes()).u64(s.len() as u64);
    }
    h.hex()
}

/// Each window's score, P(keep) of the parts it covers weighted by how much of it each covers; a
/// window no part covers keeps its score. Windows are `len` seconds from their start.
pub fn score_windows(windows: &mut [Window], len: f64, j: &Judged, fit: &Fit) {
    let parts: Vec<(Part, f64)> = j
        .parts
        .iter()
        .map(|p| (p.part(), fit.p(p.margin)))
        .collect();
    for w in windows {
        let (a, b) = (w.start, w.start + len);
        let from = parts.partition_point(|(p, _)| p.end <= a);
        let (mut sum, mut cover) = (0.0, 0.0);
        for (p, chance) in parts[from..].iter().take_while(|(p, _)| p.start < b) {
            let over = (p.end.min(b) - p.start.max(a)).max(0.0);
            sum += over * chance;
            cover += over;
        }
        if cover > 0.0 {
            // Three places, as the file writes it.
            w.score = (sum / cover * 1000.0).round() / 1000.0;
        }
    }
}
