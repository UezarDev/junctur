//! The training pairs: every part the user answered, and every part they cut or kept by hand against
//! the automatic reduction, with what Laya read and made of it. Kept across recordings in the user's
//! config folder, since the fit is the user's taste rather than one recording's; replaced per
//! recording each time they are collected, so an answer changed is not counted twice.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::fit::{self, Fit};
use super::parts::Kind;
use super::{Choices, How, Judged};
use crate::edit::Range;
use crate::Result;

/// The pairs file in the config folder; `JUNCTUR_KEEP_PAIRS` points elsewhere.
pub const FILE: &str = "keep_pairs.json";
pub const ENV: &str = "JUNCTUR_KEEP_PAIRS";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    /// The recording, as its project names it.
    pub recording: PathBuf,
    pub start: f64,
    pub end: f64,
    pub kind: Kind,
    pub margin: f64,
    /// The user's answer.
    pub keep: bool,
    /// `accepted`, `dismissed` or `hand`.
    pub how: String,
    pub state: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Pairs {
    pairs: Vec<Pair>,
}

/// Where the pairs are kept; `None` without a config folder.
pub fn pairs_path() -> Option<PathBuf> {
    match std::env::var_os(ENV) {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => crate::config::folder().map(|f| f.join(FILE)),
    }
}

/// `recording`'s pairs as they stand: each answered part, and each unanswered part the list (`now`)
/// does otherwise than the reduction (`auto`) does with it.
pub fn collect(
    recording: &Path,
    j: &Judged,
    now: &[Range],
    auto: &[Range],
    choices: &Choices,
) -> Vec<Pair> {
    let mut out = Vec::new();
    for x in &j.parts {
        let part = x.part();
        let (keep, how) = match choices.standing(&part, now) {
            Some(a) => (
                a.keep,
                match a.how {
                    How::Accepted => "accepted",
                    How::Dismissed => "dismissed",
                },
            ),
            None => {
                let kept = part.kept_by(now);
                if kept == part.kept_by(auto) {
                    continue;
                }
                (kept, "hand")
            }
        };
        out.push(Pair {
            recording: recording.to_path_buf(),
            start: x.start,
            end: x.end,
            kind: x.kind,
            margin: x.margin,
            keep,
            how: how.into(),
            state: x.state.clone(),
        });
    }
    out
}

/// Every pair in `path`; none when there is no file or it does not read (a broken file is logged by
/// the writer, which then starts it again rather than failing every edit).
pub fn load_pairs(path: &Path) -> Vec<Pair> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Pairs>(&b).ok())
        .map(|p| p.pairs)
        .unwrap_or_default()
}

/// Replaces `recording`'s pairs in `path` with `pairs`; returns every pair now in it.
pub fn store_pairs(path: &Path, recording: &Path, pairs: Vec<Pair>) -> Result<Vec<Pair>> {
    let mut all = load_pairs(path);
    all.retain(|p| p.recording != recording);
    all.extend(pairs);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = Pairs { pairs: all };
    crate::output::write_replacing(path, serde_json::to_vec(&file)?)?;
    Ok(file.pairs)
}

/// The fit of every pair in `path` (the defaults when there are too few).
pub fn fitted(path: Option<&Path>) -> Fit {
    path.map_or_else(Fit::default, |p| fit_of(&load_pairs(p)))
}

pub(super) fn fit_of(pairs: &[Pair]) -> Fit {
    let v: Vec<(f64, bool)> = pairs.iter().map(|p| (p.margin, p.keep)).collect();
    fit::fit(&v)
}
