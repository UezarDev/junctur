//! The transcription lane, one shared background job, as an ordered queue that can be paused: the
//! page shows who waits in order, which a bare mutex cannot. A pause holds the whole lane, so the
//! machine is the user's again rather than the next recording's.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

#[derive(Default)]
struct State {
    /// Every place on the lane, the head transcribing, the rest in the order they came. Each place
    /// has its own number, so a recording queued twice still waits its own turn.
    queue: VecDeque<(u64, PathBuf)>,
    next: u64,
    paused: bool,
    /// The head is held at a chunk boundary by the pause (it may still be finishing a chunk while
    /// `paused` and not `stopped`).
    stopped: bool,
}

/// Who is on the lane and whether it is paused, for the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub paused: bool,
    pub stopped: bool,
    pub queue: Vec<PathBuf>,
}

#[derive(Default)]
pub struct Lane {
    state: Mutex<State>,
    moved: Condvar,
}

/// A recording's place on the lane, from the moment it is queued; leaving it (a run ending, or
/// unwinding while it waits) takes the place off and hands the lane to the next.
pub struct Turn<'a> {
    lane: &'a Lane,
    id: u64,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut s = self.lane.state();
        let head = s.queue.front().is_some_and(|e| e.0 == self.id);
        s.queue.retain(|e| e.0 != self.id);
        // A pause is for the jobs it was given on: one left on an empty lane would start the next
        // recording paused, possibly much later and with no panel open to say so.
        if s.queue.is_empty() {
            s.paused = false;
        }
        if head {
            s.stopped = false;
        }
        self.lane.moved.notify_all();
    }
}

impl Lane {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait<'a>(&self, s: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.moved.wait(s).unwrap_or_else(|e| e.into_inner())
    }

    /// Queues `file` and returns once it is the head. `on_wait` is called once, outside the lane's
    /// lock, when it has to wait (so it may ask the lane for its [`View`]). The place is a [`Turn`]
    /// before `on_wait` runs, so a panic there takes it off rather than leaving it at the head of
    /// a lane nobody will ever leave.
    pub fn enter(&self, file: &Path, on_wait: impl FnOnce()) -> Turn<'_> {
        let mut s = self.state();
        let id = s.next;
        s.next += 1;
        s.queue.push_back((id, file.to_path_buf()));
        let turn = Turn { lane: self, id };
        let head = |s: &State| s.queue.front().is_some_and(|e| e.0 == id);
        if !head(&s) {
            drop(s);
            on_wait();
            s = self.state();
            while !head(&s) {
                s = self.wait(s);
            }
        }
        drop(s);
        turn
    }

    /// Called by the head between chunks: returns at once unless paused, else blocks until
    /// resumed, calling `on_stop` (outside the lock) each time it is held anew: once, unless a
    /// resume and a pause both came before it woke, which leaves it held but not `stopped`.
    pub fn hold(&self, mut on_stop: impl FnMut()) {
        let mut s = self.state();
        while s.paused {
            if !s.stopped {
                s.stopped = true;
                drop(s);
                on_stop();
                s = self.state();
                continue;
            }
            s = self.wait(s);
        }
    }

    /// Pauses or resumes the lane; a pause with nothing on it is ignored (see [`Turn`]'s drop).
    /// Whether anything changed.
    pub fn pause(&self, paused: bool) -> bool {
        let mut s = self.state();
        let paused = paused && !s.queue.is_empty();
        if s.paused == paused {
            return false;
        }
        s.paused = paused;
        s.stopped = false;
        self.moved.notify_all();
        true
    }

    pub fn view(&self) -> View {
        let s = self.state();
        View {
            paused: s.paused,
            stopped: s.stopped,
            queue: s.queue.iter().map(|e| e.1.clone()).collect(),
        }
    }
}

#[cfg(test)]
mod tests;
