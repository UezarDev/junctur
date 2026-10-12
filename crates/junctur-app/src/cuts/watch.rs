//! Notices when the edit list changes on disk (a hand edit, `junctur cut`, later the app's own
//! editor) by polling its size and modification time. One small file does not justify a
//! file-system notification dependency, and a stat every few hundred ms costs nothing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const POLL: Duration = Duration::from_millis(400);

/// What a poll sees: `None` while the file does not exist.
type Stamp = Option<(u64, Option<SystemTime>)>;

fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

/// Decides when a change is ready to read: only once two polls in a row agree, so a file still
/// being written is not read half-done.
#[derive(Debug)]
struct Settle {
    seen: Stamp,
    pending: Option<Stamp>,
}

impl Settle {
    fn new(seen: Stamp) -> Self {
        Self {
            seen,
            pending: None,
        }
    }

    /// Feeds one poll; true when a settled change should be read.
    fn poll(&mut self, now: Stamp) -> bool {
        if now == self.seen {
            self.pending = None;
            return false;
        }
        if self.pending == Some(now) {
            self.seen = now;
            self.pending = None;
            return true;
        }
        self.pending = Some(now);
        false
    }
}

/// A running watch; dropping it stops the thread. The thread is not joined: its callback may be
/// waiting for the lock whoever drops it holds.
pub struct Watch {
    stop: Arc<AtomicBool>,
}

impl Watch {
    /// Calls `changed` on its own thread after each settled change to `path`, starting from how
    /// `path` is now (the caller has just read it).
    pub fn start(path: PathBuf, changed: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        // Stamped here, not on the thread, so a change right after the caller's read is not taken
        // as the starting point.
        let mut settle = Settle::new(stamp(&path));
        let spawned = std::thread::Builder::new()
            .name("edit-watch".into())
            .spawn(move || loop {
                std::thread::sleep(POLL);
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                if settle.poll(stamp(&path)) && !flag.load(Ordering::SeqCst) {
                    changed();
                }
            });
        if let Err(e) = spawned {
            eprintln!("junctur-app: no edit-list watch: {e}");
        }
        Self { stop }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(len: u64) -> Stamp {
        Some((len, None))
    }

    #[test]
    fn a_change_is_read_once_it_settles() {
        let mut s = Settle::new(at(10));
        assert!(!s.poll(at(10)));
        assert!(!s.poll(at(20)), "first sight of a change waits");
        assert!(!s.poll(at(30)), "still being written");
        assert!(s.poll(at(30)));
        assert!(!s.poll(at(30)), "read once");
        // Deleted, then back.
        assert!(!s.poll(None));
        assert!(s.poll(None));
        assert!(!s.poll(at(5)));
        assert!(s.poll(at(5)));
    }

    #[test]
    fn calls_back_on_a_real_change() {
        let d = std::env::temp_dir().join(format!("junctur-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("edit.junctur");
        std::fs::write(&p, "a").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let w = Watch::start(p.clone(), move || {
            let _ = tx.send(());
        });
        std::fs::write(&p, "longer").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        drop(w);
        let _ = std::fs::remove_dir_all(&d);
    }
}
