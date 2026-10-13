//! The transcription lane (2j's one shared background job) as a queue that can be paused (2k).
//!
//! A module rather than the bare mutex 2j had: the page shows who transcribes and who waits, in
//! the order they will run, and `std::sync::Mutex` hands a released lock to any waiter, so it has
//! no order to show. Pausing is the lane's, not a run's: the head stops at its next chunk and the
//! recordings behind it keep waiting, so the machine is the user's again (Uezar wanted the job
//! paused from any recording; letting the next one start would not give the machine back).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

#[derive(Default)]
struct State {
    /// Every recording on the lane, the head transcribing, the rest in the order they came.
    queue: VecDeque<PathBuf>,
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

/// A recording's place at the head of the lane; leaving it hands the lane to the next.
pub struct Turn<'a> {
    lane: &'a Lane,
    file: PathBuf,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut s = self.lane.state();
        if let Some(i) = s.queue.iter().position(|f| *f == self.file) {
            s.queue.remove(i);
        }
        // A pause is for the jobs it was given on: one left on an empty lane would start the next
        // recording paused, possibly much later and with no panel open to say so.
        if s.queue.is_empty() {
            s.paused = false;
        }
        s.stopped = false;
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
    /// lock, when it has to wait (so it may ask the lane for its [`View`]).
    pub fn enter(&self, file: &Path, on_wait: impl FnOnce()) -> Turn<'_> {
        let mut s = self.state();
        s.queue.push_back(file.to_path_buf());
        if s.queue.front().map(PathBuf::as_path) != Some(file) {
            drop(s);
            on_wait();
            s = self.state();
            while s.queue.front().map(PathBuf::as_path) != Some(file) {
                s = self.wait(s);
            }
        }
        Turn {
            lane: self,
            file: file.to_path_buf(),
        }
    }

    /// Called by the head between chunks: returns at once unless paused, else blocks until
    /// resumed, calling `on_stop` once (outside the lock) when it starts holding.
    pub fn hold(&self, on_stop: impl FnOnce()) {
        let mut s = self.state();
        if !s.paused {
            return;
        }
        s.stopped = true;
        drop(s);
        on_stop();
        s = self.state();
        while s.paused {
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
            queue: s.queue.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn wait_for(what: &str, ok: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ok() {
            assert!(Instant::now() < deadline, "never: {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn recordings_take_the_lane_in_the_order_they_came() {
        let lane = Arc::new(Lane::default());
        let order = Arc::new(Mutex::new(Vec::new()));
        let waited = Arc::new(AtomicUsize::new(0));
        let first = lane.enter(Path::new("a"), || panic!("a lone recording never waits"));
        let mut threads = Vec::new();
        for name in ["b", "c", "d"] {
            let (mine, order, waited) = (lane.clone(), order.clone(), waited.clone());
            threads.push(std::thread::spawn(move || {
                let _turn = mine.enter(Path::new(name), || {
                    waited.fetch_add(1, Ordering::SeqCst);
                });
                order.lock().unwrap().push(name);
            }));
            // Each queued before the next comes.
            let n = threads.len() + 1;
            wait_for("queued", || lane.view().queue.len() == n);
        }
        let names = |v: View| {
            v.queue
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(lane.view()), ["a", "b", "c", "d"]);
        drop(first);
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(*order.lock().unwrap(), ["b", "c", "d"]);
        assert_eq!(waited.load(Ordering::SeqCst), 3);
        assert!(lane.view().queue.is_empty());
    }

    #[test]
    fn a_paused_head_holds_between_chunks_until_resumed() {
        let lane = Arc::new(Lane::default());
        assert!(!lane.pause(true), "nothing on the lane: nothing to pause");
        let turn = lane.enter(Path::new("a"), || {});
        // Not paused: a chunk boundary passes straight through.
        lane.hold(|| panic!("not paused"));
        assert!(lane.pause(true));
        assert!(!lane.pause(true), "already paused");
        assert_eq!((lane.view().paused, lane.view().stopped), (true, false));
        let stops = Arc::new(AtomicUsize::new(0));
        let passed = Arc::new(AtomicUsize::new(0));
        let head = {
            let (lane, stops, passed) = (lane.clone(), stops.clone(), passed.clone());
            std::thread::spawn(move || {
                lane.hold(|| {
                    stops.fetch_add(1, Ordering::SeqCst);
                });
                passed.fetch_add(1, Ordering::SeqCst);
            })
        };
        wait_for("stopped", || lane.view().stopped);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(passed.load(Ordering::SeqCst), 0, "held while paused");
        assert!(lane.pause(false));
        head.join().unwrap();
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!((lane.view().paused, lane.view().stopped), (false, false));
        drop(turn);
    }

    #[test]
    fn a_pause_is_forgotten_once_the_lane_is_empty() {
        let lane = Lane::default();
        let turn = lane.enter(Path::new("a"), || {});
        lane.pause(true);
        drop(turn);
        assert!(!lane.view().paused);
        // The next recording is not held.
        let _turn = lane.enter(Path::new("b"), || {});
        lane.hold(|| panic!("a pause from an earlier job"));
    }
}
