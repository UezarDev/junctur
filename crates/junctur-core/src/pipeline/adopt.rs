//! Takes the results a packaged app's run (Claude desktop) left in a hidden copy of the cache
//! ([`crate::hidden`]) into the real project folder as one unit: only into a folder with no run of
//! its own, only from the newest settled copy, `jobs.json` last, and once per recording.

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
mod tests {
    use super::*;
    use crate::hidden::tests::{appdata, scratch, write};

    const KEY: &str = "cache/projects/0123456789abcdef";

    fn jobs(state: &str) -> String {
        format!(
            r#"{{"file":"x.mp4","stages":[{{"name":"tracks","state":"done","seconds":1.0,"note":null}},{{"name":"transcribe","state":"{state}","seconds":null,"note":null}}]}}"#
        )
    }

    /// A hidden run's folder: its stage files, then `jobs.json` with the last stage `state`.
    fn run(at: &Path, words: &str, state: &str, fresh: bool) {
        write(&at.join("transcript.json"), words, fresh);
        write(&at.join("jobs.json"), &jobs(state), fresh);
    }

    fn read(p: &Path) -> Option<String> {
        fs::read_to_string(p).ok()
    }

    #[test]
    fn takes_the_newest_finished_copy_whole_and_only_once() {
        let d = scratch("project");
        let (l, local, a, b) = appdata(&d);
        let real = local.join(KEY);
        fs::create_dir_all(&real).unwrap();
        run(&a.join(KEY), "a's words", "done", false);
        write(&a.join(KEY).join("speakers.json"), "a's speakers", false);
        run(&b.join(KEY), "b's words", "done", false);
        // B's run ended later.
        let later = SystemTime::now() - std::time::Duration::from_secs(30);
        fs::File::options()
            .write(true)
            .open(b.join(KEY).join("jobs.json"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        write(&real.join("mutes.json"), "the user's mutes", false);

        assert_eq!(
            project_with(&l, &real, SystemTime::now()),
            Some(b.join(KEY))
        );
        assert_eq!(
            read(&real.join("transcript.json")).as_deref(),
            Some("b's words")
        );
        assert_eq!(
            read(&real.join("speakers.json")),
            None,
            "nothing of A's run"
        );
        assert_eq!(
            read(&real.join("mutes.json")).as_deref(),
            Some("the user's mutes")
        );
        assert!(real.join("jobs.json").exists());

        // Forgotten (the folder emptied): the hidden analysis does not come back.
        fs::remove_dir_all(&real).unwrap();
        fs::create_dir_all(&real).unwrap();
        assert_eq!(project_with(&l, &real, SystemTime::now()), None);
        assert!(hidden::files(&real).is_empty());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_run_still_going_or_just_written_is_not_taken() {
        let d = scratch("project-running");
        let (l, local, a, b) = appdata(&d);
        let real = local.join(KEY);
        fs::create_dir_all(&real).unwrap();
        run(&a.join(KEY), "half", "running", false);
        run(&b.join(KEY), "fresh", "done", true);
        assert_eq!(project_with(&l, &real, SystemTime::now()), None);
        assert!(hidden::files(&real).is_empty());
        // Once B has settled it is taken; A, still running, never.
        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        assert_eq!(project_with(&l, &real, later), Some(b.join(KEY)));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_with_a_run_of_its_own_takes_nothing() {
        let d = scratch("project-own");
        let (l, local, a, _) = appdata(&d);
        let real = local.join(KEY);
        run(&a.join(KEY), "hidden words", "done", false);
        write(&a.join(KEY).join("speech.json"), "hidden speech", false);
        // A crashed real run left a stage file and no `jobs.json`.
        write(&real.join("speech.json"), "real speech", false);
        assert_eq!(project_with(&l, &real, SystemTime::now()), None);
        assert_eq!(read(&real.join("transcript.json")), None);
        assert_eq!(
            read(&real.join("speech.json")).as_deref(),
            Some("real speech")
        );
        fs::remove_dir_all(&d).unwrap();
    }
}
