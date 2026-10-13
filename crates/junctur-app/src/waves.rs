//! The open recording's waveforms, one per audio track, for the track lanes (2g).
//!
//! Core's `waves` reads them (one ffmpeg for every track) and keeps them in the project folder; this
//! is the app's side: read the kept ones when a recording opens, or compute them on a thread of its
//! own while the recording already plays, telling the page how far it got about once a second, so
//! the lanes fill in from the left. The page never holds the peaks: it asks for one column per
//! pixel of the window it draws ([`Store::columns`]).
//!
//! A module of its own rather than part of the player or the analysis: it outlives neither the
//! recording being open (opening another stops it, and the next open starts again; nothing is kept
//! half-read) nor needs the analysis's gate (an 8 kHz decode is not the GPU work that serialises).
//! It starts at open and does not wait for the `tracks` stage: the read needs only the file's audio
//! streams, so the lanes show before the analysis gets anywhere.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use junctur_core::ffmpeg;
use junctur_core::waves::{self, Waves};
use serde::Serialize;

use crate::lock::lock;
use crate::mixer::MAX_TRACKS;

/// The event each step of a read is sent as.
pub const EVENT: &str = "waves";
/// At most this often while reading.
const EVERY: Duration = Duration::from_secs(1);

/// How far the open recording's waveforms are.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WavesState {
    /// The recording, absolute.
    pub file: String,
    pub tracks: usize,
    /// Seconds read so far (all of it once `done`).
    pub seconds: f64,
    /// The recording's length when known, for a percentage.
    pub duration: Option<f64>,
    pub done: bool,
    pub error: Option<String>,
}

/// What a lane draws: each track's peak per column, track by track (0..255, -60..0 dBFS).
#[derive(Debug, Clone)]
pub struct WavesView {
    pub state: WavesState,
    pub columns: usize,
    pub peaks: Vec<u8>,
}

impl WavesView {
    /// As the page reads it (api.ts `waves`): the column count and the track count as
    /// little-endian u32s, then the peaks.
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + self.peaks.len());
        b.extend_from_slice(&(self.columns as u32).to_le_bytes());
        b.extend_from_slice(&(self.state.tracks as u32).to_le_bytes());
        b.extend_from_slice(&self.peaks);
        b
    }
}

/// Told of every step of a read (the app sends it to the page).
pub type Notify = Arc<dyn Fn(&WavesState) + Send + Sync>;

struct Shared {
    waves: Mutex<Waves>,
    state: Mutex<WavesState>,
    stop: AtomicBool,
}

#[derive(Default)]
pub struct Store {
    current: Mutex<Option<Arc<Shared>>>,
}

impl Store {
    /// The waveforms of `file` (absolute), whose project folder is `dir` (`None`: computed, not
    /// kept): read from the folder, or computed in the background. Stops a read still going for
    /// the recording open before.
    pub fn open(&self, file: &Path, dir: Option<PathBuf>, notify: Notify) {
        let shared = Arc::new(Shared {
            waves: Mutex::new(Waves::default()),
            state: Mutex::new(WavesState {
                file: file.display().to_string(),
                ..Default::default()
            }),
            stop: AtomicBool::new(false),
        });
        if let Some(old) = lock(&self.current).replace(shared.clone()) {
            old.stop.store(true, Ordering::Relaxed);
        }
        let file = file.to_path_buf();
        let (s, n) = (shared.clone(), notify.clone());
        let spawned = std::thread::Builder::new()
            .name("waves".into())
            .spawn(move || compute(&file, dir.as_deref(), &s, &n));
        // Said where the page looks (Properties), not only on stderr.
        if let Err(e) = spawned {
            let state = {
                let mut st = lock(&shared.state);
                st.error = Some(format!("no thread to read them on: {e}"));
                st.clone()
            };
            notify(&state);
        }
    }

    /// No recording is open any more (2i): a read still going stops, and nothing is served.
    pub fn close(&self) {
        if let Some(old) = lock(&self.current).take() {
            old.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn state(&self) -> Option<WavesState> {
        lock(&self.current).as_ref().map(|s| lock(&s.state).clone())
    }

    /// Each track's peak in `columns` equal stretches of `[from, to)` seconds of the open recording.
    pub fn columns(&self, from: f64, to: f64, columns: usize) -> Option<WavesView> {
        let shared = lock(&self.current).clone()?;
        // A lane is a few thousand pixels at most; this bounds a page asking for something absurd.
        let columns = columns.min(8192);
        let peaks = lock(&shared.waves).columns(from, to, columns);
        let state = lock(&shared.state).clone();
        Some(WavesView {
            state,
            columns,
            peaks,
        })
    }
}

fn compute(file: &Path, dir: Option<&Path>, shared: &Shared, notify: &Notify) {
    // Another recording may have opened before this thread got going: no probe, no decoder.
    let stopped = || shared.stop.load(Ordering::Relaxed);
    if stopped() {
        return;
    }
    let set = |f: &dyn Fn(&mut WavesState)| {
        let s = {
            let mut s = lock(&shared.state);
            f(&mut s);
            s.clone()
        };
        if !shared.stop.load(Ordering::Relaxed) {
            notify(&s);
        }
    };
    let probe = match ffmpeg::probe(file) {
        Ok(p) => p,
        Err(e) => return set(&|s| s.error = Some(e.to_string())),
    };
    let tracks = probe.audio.len().min(MAX_TRACKS);
    let duration = (probe.duration > 0.0).then_some(probe.duration);
    if stopped() {
        return;
    }
    if let Some(w) = dir.and_then(|d| waves::load(d, tracks)) {
        let seconds = w.seconds();
        *lock(&shared.waves) = w;
        return set(&|s| {
            s.tracks = tracks;
            s.duration = duration;
            s.seconds = seconds;
            s.done = true;
        });
    }
    *lock(&shared.waves) = Waves::new(tracks);
    set(&|s| {
        s.tracks = tracks;
        s.duration = duration;
    });
    let t0 = Instant::now();
    let mut told = Instant::now();
    // The peaks go straight into the shared copy, the only one: the read keeps none, and the save
    // writes from it.
    let read = waves::read(file, tracks, duration, &shared.stop, &mut |more| {
        let seconds = {
            let mut w = lock(&shared.waves);
            w.data.extend_from_slice(more);
            w.seconds()
        };
        lock(&shared.state).seconds = seconds;
        if told.elapsed() >= EVERY {
            told = Instant::now();
            set(&|_| {});
        }
    });
    match read {
        Ok(Some(_)) => {
            eprintln!(
                "junctur-app: waveforms of {tracks} tracks read in {:.1} s",
                t0.elapsed().as_secs_f64()
            );
            // Under the lock: the lanes wait for the write (once, tens of milliseconds at 30 h).
            let seconds = {
                let w = lock(&shared.waves);
                if let Some(d) = dir {
                    if let Err(e) = waves::save(d, &w) {
                        eprintln!("junctur-app: keeping the waveforms in {}: {e}", d.display());
                    }
                }
                w.seconds()
            };
            set(&|s| {
                s.seconds = seconds;
                s.done = true;
            });
        }
        // Stopped: another recording opened.
        Ok(None) => {}
        Err(e) => set(&|s| s.error = Some(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn wait_done(rx: &mpsc::Receiver<WavesState>) -> WavesState {
        loop {
            let s = rx.recv_timeout(Duration::from_secs(30)).expect("a step");
            if s.done || s.error.is_some() {
                return s;
            }
        }
    }

    #[test]
    fn computed_once_then_read_from_the_project_folder() {
        let dir = std::env::temp_dir().join(format!("junctur-app-waves-{}", std::process::id()));
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let f = dir.join("two.mka");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=2"])
            .args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=mono", "-t", "2"])
            .args(["-map", "0:a", "-map", "1:a", "-c:a", "flac"])
            .arg(&f)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let store = Store::default();
        assert!(store.columns(0.0, 1.0, 4).is_none(), "nothing open");
        let (tx, rx) = mpsc::channel();
        let notify: Notify = Arc::new(move |s: &WavesState| {
            let _ = tx.send(s.clone());
        });
        store.open(&f, Some(project.clone()), notify.clone());
        let s = wait_done(&rx);
        assert_eq!((s.tracks, s.done, s.error.clone()), (2, true, None));
        assert!((s.seconds - 2.0).abs() < 0.05, "{s:?}");
        assert!(project.join(waves::FILE).is_file());
        let v = store.columns(0.0, 2.0, 2).unwrap();
        assert!(
            v.peaks[0] > 150 && v.peaks[1] > 150,
            "the tone: {:?}",
            v.peaks
        );
        assert_eq!(&v.peaks[2..], &[0, 0], "silence");
        assert_eq!(&v.bytes()[..8], &[2, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(&v.bytes()[8..], &v.peaks[..]);
        // Opened again: taken from the folder.
        store.open(&f, Some(project), notify);
        let again = wait_done(&rx);
        assert_eq!(again.seconds, s.seconds);
        assert_eq!(store.state().unwrap(), again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
