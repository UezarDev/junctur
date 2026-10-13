//! Translating the open recording's captions in the background (5k): the model takes about a
//! quarter of a second a sentence on the CPU, so a long recording takes minutes to an hour, which
//! must never hold up the editor. One run at a time, on its own thread, saving as it goes
//! (`translate::run`), its progress sent to the page as [`EVENT`] and cancellable; the preview
//! shows what is translated so far, and an export or a send waits for all of it
//! (`export::untranslated`).
//!
//! Its own module because the run outlives the command that started it, as an analysis run does
//! (`analysis.rs`), but needs none of that lane's queueing: a new run replaces the one going.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use junctur_core::transcribe::Segment;
use junctur_core::translate::{self, Sentence};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::lock::lock;

/// The event every change of the run is sent as.
pub const EVENT: &str = "translation";

/// Where the latest run is.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// The recording, as it was opened.
    pub file: String,
    /// The target language.
    pub to: String,
    /// Sentences done of the ones that needed translating when it started.
    pub done: usize,
    pub total: usize,
    pub running: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    /// Which run this is: a run ended or replaced never writes over a later one's state.
    #[serde(skip)]
    id: u64,
}

/// What a run translates, gathered by the player from the list in play.
pub struct Job {
    pub file: String,
    pub to: String,
    pub project: PathBuf,
    pub cache_dir: PathBuf,
    pub sentences: Vec<Sentence>,
    pub segments: Vec<Segment>,
}

#[derive(Default)]
pub struct Translation {
    run: Arc<Mutex<Run>>,
    stop: Mutex<Arc<AtomicBool>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Translation {
    pub fn run(&self) -> Run {
        lock(&self.run).clone()
    }

    /// Starts translating `job`, unless the same recording into the same language is already
    /// going. A run going for anything else is stopped, and the new one waits for it to end
    /// before it starts, so two never write the translations at once.
    pub fn start(&self, app: AppHandle, job: Job) -> Run {
        {
            let r = lock(&self.run);
            if r.running && r.file == job.file && r.to == job.to {
                return r.clone();
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let before = std::mem::replace(&mut *lock(&self.stop), stop.clone());
        before.store(true, Ordering::Relaxed);
        let previous = lock(&self.thread).take();
        let started = Run {
            file: job.file.clone(),
            to: job.to.clone(),
            running: true,
            id: lock(&self.run).id + 1,
            ..Default::default()
        };
        let id = started.id;
        *lock(&self.run) = started.clone();
        let shared = self.run.clone();
        let handle = std::thread::Builder::new()
            .name("translation".into())
            .spawn(move || {
                if let Some(p) = previous {
                    let _ = p.join();
                }
                let send = |r: &Run| {
                    let _ = app.emit(EVENT, r);
                };
                send(&started);
                let mut progress = |done: usize, total: usize| {
                    let mut r = lock(&shared);
                    if r.id == id {
                        (r.done, r.total) = (done, total);
                        send(&r);
                    }
                };
                let result = translate::run(
                    &job.project,
                    &job.cache_dir,
                    &job.to,
                    &job.sentences,
                    &job.segments,
                    &mut progress,
                    &stop,
                );
                let mut r = lock(&shared);
                // A newer run took over: its state is not this one's to end.
                if r.id != id {
                    return;
                }
                r.running = false;
                match result {
                    Ok(d) => r.cancelled = d.cancelled,
                    Err(e) => r.error = Some(e.to_string()),
                }
                send(&r);
            });
        match handle {
            Ok(h) => *lock(&self.thread) = Some(h),
            Err(e) => {
                let mut r = lock(&self.run);
                r.running = false;
                r.error = Some(format!("could not start translating: {e}"));
            }
        }
        self.run()
    }

    /// Asks the run going to stop after the sentences in hand; what it did is kept.
    pub fn cancel(&self) {
        lock(&self.stop).store(true, Ordering::Relaxed);
    }
}
