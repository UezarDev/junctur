//! Takes the results a packaged app's run (Claude desktop) left in a hidden copy of the cache
//! ([`crate::hidden`]) into the real project folder as one unit: only into a folder with no run of
//! its own, only from the newest settled copy, `jobs.json` last, and once per recording.
//! So one analysis never mixes with another's files, a half-written run is never taken, a folder
//! never says done before its results are in, and a forgotten analysis does not return.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::jobs::{self, Jobs, StageState};
use crate::hidden::{self, Adopted, Ledger, Lookup};

/// What a project folder holds before any run: the mixer's mutes, saved when the recording is
/// first played.
const BENIGN: [&str; 2] = ["mutes.json", hidden::LEDGER];

/// [`project_with`] on this system.
pub(super) fn project(dir: &Path) {
    if let Some(lookup) = Lookup::system() {
        project_with(lookup, dir, SystemTime::now());
    }
}

/// Takes the newest finished hidden copy of project folder `dir`; returns where it came from.
fn project_with(lookup: &Lookup, dir: &Path, now: SystemTime) -> Option<PathBuf> {
    let has_run = fs::read_dir(dir)
        .ok()?
        .flatten()
        .any(|e| !BENIGN.iter().any(|b| e.file_name() == *b));
    if has_run {
        return None;
    }
    let (projects, key) = (
        dir.parent()?,
        dir.file_name()?.to_string_lossy().into_owned(),
    );
    let mut ledger = Ledger::load(projects);
    if ledger.has(&key) {
        return None;
    }
    let copies = lookup.copies(dir);
    let from = copies
        .iter()
        .filter(|h| finished(h, now))
        .max_by_key(|h| hidden::stamp(&h.join(jobs::FILE)).map(|s| s.1))?
        .clone();

    let mut files = hidden::files(&from);
    files.sort_by_key(|f| f == Path::new(jobs::FILE));
    let mut taken = Vec::new();
    let mut done = Adopted::default();
    for rel in files {
        let dst = dir.join(&rel);
        if dst.exists() {
            continue;
        }
        match hidden::copy_in(&from.join(&rel), &dst) {
            Ok(true) => taken.push(dst),
            _ => {
                // Changed while copied, or a run started here meanwhile: take none of it now.
                for f in taken {
                    let _ = fs::remove_file(f);
                }
                done.pending = 1;
                hidden::report(dir, &[from], done);
                return None;
            }
        }
    }
    done.copied = taken.len();
    ledger.add(key);
    if ledger.save().is_err() {
        eprintln!("junctur: could not record what {} took", projects.display());
    }
    hidden::report(dir, std::slice::from_ref(&from), done);
    Some(from)
}

/// A copy whose run ended: `jobs.json` readable with a stage done and none running, and nothing
/// written lately.
fn finished(h: &Path, now: SystemTime) -> bool {
    let jobs = fs::read(h.join(jobs::FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Jobs>(&b).ok());
    jobs.is_some_and(|j| {
        j.stages.iter().all(|s| s.state != StageState::Running)
            && j.stages.iter().any(|s| s.state == StageState::Done)
    }) && hidden::settled(h, now)
}

#[cfg(test)]
mod tests;
