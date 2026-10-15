//! Edits in progress per recording: a run of one waits for them to end before its first half, and
//! an edit is refused while a run would replace the list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use super::progress::Progress;
use super::{key, Analysis};
use crate::lock::lock;

/// How many edits of each recording are in progress, and a way to wait for them to end.
#[derive(Default)]
pub(super) struct Edits {
    count: Mutex<HashMap<PathBuf, usize>>,
    ended: Condvar,
}

/// An edit of a recording in progress ([`Analysis::edit`]): until it drops, no run of that
/// recording starts its first half.
pub struct Editing {
    edits: Arc<Edits>,
    k: PathBuf,
}

impl Drop for Editing {
    fn drop(&mut self) {
        let mut count = lock(&self.edits.count);
        if let Some(n) = count.get_mut(&self.k) {
            *n -= 1;
            if *n == 0 {
                count.remove(&self.k);
            }
        }
        self.edits.ended.notify_all();
    }
}

/// A recording whose run is `r` may be edited: no run, or its run has written the list, which
/// nothing later in the run replaces.
pub(super) fn may_edit(r: Option<&Progress>) -> bool {
    r.is_none_or(|p| !p.phase.busy() || p.listed)
}

impl Analysis {
    /// `file`'s edit list may be edited ([`may_edit`]); the app asks through [`Self::edit`], which
    /// also holds a run off while the edit works.
    #[cfg(test)]
    pub(super) fn editable(&self, file: &Path) -> bool {
        may_edit(self.progress(file).as_ref())
    }

    /// Marks an edit of `file` in progress when it may be edited ([`may_edit`]), else `None`; a run
    /// asked for meanwhile waits before its first half. Check and count are one step under `runs`,
    /// so an edit (an enrolled pin) and a run's speakers stage never overlap.
    pub fn edit(&self, file: &Path) -> Option<Editing> {
        let k = key(file);
        let runs = lock(&self.runs);
        if !may_edit(runs.get(&k)) {
            return None;
        }
        *lock(&self.editing.count).entry(k.clone()).or_default() += 1;
        drop(runs);
        Some(Editing {
            edits: self.editing.clone(),
            k,
        })
    }

    /// Blocks until no edit of `k` is in progress ([`Self::edit`]).
    pub(super) fn wait_for_edits(&self, k: &Path) {
        let mut count = lock(&self.editing.count);
        while count.contains_key(k) {
            count = self
                .editing
                .ended
                .wait(count)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}
