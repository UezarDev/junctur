//! "Ask, scrub, accept" in the app (7c, a prototype): what the Ask tab and lane show of a
//! recording's requests ([`junctur_core::ask`]), a request read (by the model, held between requests
//! and let go when idle, or by the keywords) and run over the selection, the ghosts previewed in
//! play, and the answers turned into edits of the list in play. A module because it holds the model
//! between requests and its answers edit through the player, as like.rs's do.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use junctur_core::ask::{self, Ask};
use junctur_core::llm::{self, Size, Translator};
use junctur_core::pipeline::Project;
use junctur_core::recipe;
use junctur_core::runtime::DevicePref;
use serde::{Deserialize, Serialize};

use crate::lock::lock;
use crate::player::Core;

/// The model is let go after this long without a request, so it does not hold the GPU's memory
/// while the analysis or another model wants it.
const IDLE: Duration = Duration::from_secs(120);

/// The translator between requests, and when it last read one.
static MODEL: Mutex<Option<(Translator, Instant)>> = Mutex::new(None);

/// Every read-change-write of an ask file, one at a time (a request runs for seconds while an
/// answer may come in).
static FILES: Mutex<()> = Mutex::new(());

/// A model as the tab tells it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub size: Size,
    pub name: &'static str,
    /// In the cache: a request does not download it first.
    pub downloaded: bool,
    /// Open now: a request does not load it first.
    pub loaded: bool,
    /// Its size, for the first download's warning.
    pub megabytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskView {
    #[serde(flatten)]
    pub ask: Ask,
    pub duration: f64,
    pub speakers: Vec<String>,
    /// Examples marked in the Like this tab: what `like_this` looks for.
    pub examples: usize,
    /// The small model and the bigger one.
    pub models: Vec<ModelView>,
}

fn open(file: &Path, cache_dir: Option<PathBuf>) -> Result<Project, String> {
    Project::open(file, cache_dir, None).map_err(|e| e.to_string())
}

fn model_views(cache_dir: &Path) -> Vec<ModelView> {
    // Not waited for: a request holds the slot while it reads (minutes on a first download), and
    // the tab's view must not wait on it. Busy reads as not open.
    let held = match MODEL.try_lock() {
        Ok(m) => m.as_ref().map(|(t, _)| t.size),
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().as_ref().map(|(t, _)| t.size),
        Err(std::sync::TryLockError::WouldBlock) => None,
    };
    [Size::Small, Size::Large]
        .into_iter()
        .map(|size| ModelView {
            size,
            name: size.name(),
            downloaded: size.downloaded(cache_dir),
            loaded: held == Some(size),
            megabytes: size.megabytes(),
        })
        .collect()
}

fn of(p: &Project, ask: Ask) -> AskView {
    let list = p.edit().ok();
    let examples = junctur_core::like::load(&p.dir)
        .map(|l| l.examples.iter().filter(|e| e.positive).count())
        .unwrap_or(0);
    AskView {
        ask,
        duration: list.as_ref().map_or(0.0, |l| l.duration),
        speakers: list.as_ref().map(ask::speakers).unwrap_or_default(),
        examples,
        models: model_views(&p.cache_dir),
    }
}

fn change(p: &Project, f: impl FnOnce(&mut Ask) -> Result<(), String>) -> Result<AskView, String> {
    let _held = lock(&FILES);
    let mut a = ask::load(&p.dir).map_err(|e| e.to_string())?;
    f(&mut a)?;
    ask::save(&p.dir, &a).map_err(|e| e.to_string())?;
    Ok(of(p, a))
}

pub fn view(file: &Path, cache_dir: Option<PathBuf>) -> Result<AskView, String> {
    let p = open(file, cache_dir)?;
    let a = {
        let _held = lock(&FILES);
        ask::load(&p.dir).map_err(|e| e.to_string())?
    };
    Ok(of(&p, a))
}

/// Lets the model go once it has been idle [`IDLE`]; one watcher for the process.
fn watch_idle() {
    static WATCH: Once = Once::new();
    WATCH.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("ask-model-idle".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_secs(10));
                let mut m = lock(&MODEL);
                if m.as_ref().is_some_and(|(_, used)| used.elapsed() >= IDLE) {
                    *m = None;
                }
            });
    });
}

/// `request` read: by the model of `size` (opened, and downloaded the first time, when it is not
/// the one held; one model at a time), else, or when it cannot be opened or misreads twice, by the
/// keywords.
fn read(cache_dir: &Path, request: &str, speakers: &[String], size: Option<Size>) -> llm::Read {
    let Some(size) = size else {
        return llm::read(None, request, speakers, Some("the model is off".into()));
    };
    let mut held = lock(&MODEL);
    if held.as_ref().is_some_and(|(t, _)| t.size != size) {
        // The other one goes first: two would hold the GPU's memory twice.
        *held = None;
    }
    if held.is_none() {
        match Translator::open(cache_dir, size, DevicePref::Auto, &AtomicBool::new(false)) {
            Ok(t) => *held = Some((t, Instant::now())),
            Err(e) => {
                let why = format!("the model could not be opened: {e}");
                return llm::read(None, request, speakers, Some(why));
            }
        }
        watch_idle();
    }
    let (t, used) = held.as_mut().expect("just opened");
    let r = llm::read(Some(t), request, speakers, None);
    *used = Instant::now();
    // A model that failed to run is let go, not kept broken until it is idle.
    if r.reading
        .fallback
        .as_deref()
        .is_some_and(|w| w.starts_with("the model failed"))
    {
        *held = None;
    }
    r
}

/// Asks `request` of `file` over `range` (the whole recording when `None`): read, run over the edit
/// list as it is now, and added to the history with its ghosts waiting.
pub fn request(
    file: &Path,
    cache_dir: Option<PathBuf>,
    request: &str,
    range: Option<(f64, f64)>,
    model: Option<Size>,
) -> Result<AskView, String> {
    let request = request.trim();
    if request.is_empty() {
        return Err("type what to do first".into());
    }
    let p = open(file, cache_dir)?;
    let list = p
        .edit()
        .map_err(|e| format!("there is no edit list to ask about yet: {e}"))?;
    let speakers = ask::speakers(&list);
    let read = read(&p.cache_dir, request, &speakers, model);
    let step = ask::propose(&p, &list, request, range, read);
    change(&p, |a| {
        a.push(step);
        Ok(())
    })
}

/// Step `id` asked again with `recipe` (the plain words' numbers edited, or the JSON itself): a new
/// step, read as "edited", and the old one's waiting ghosts turned down.
pub fn rerun(
    file: &Path,
    cache_dir: Option<PathBuf>,
    id: u64,
    recipe: serde_json::Value,
) -> Result<AskView, String> {
    let p = open(file, cache_dir)?;
    let list = p.edit().map_err(|e| e.to_string())?;
    let r = recipe::from_json(&recipe).map_err(|e| e.to_string())?;
    let (request, range) = {
        let _held = lock(&FILES);
        let a = ask::load(&p.dir).map_err(|e| e.to_string())?;
        let s = a
            .step(id)
            .ok_or("that request is no longer in the history")?;
        (s.request.clone(), s.range)
    };
    let step = ask::propose(
        &p,
        &list,
        &request,
        range,
        llm::Read {
            recipe: Ok(r),
            reading: llm::Reading::by("edited"),
        },
    );
    change(&p, |a| {
        a.reject(id, None).map_err(|e| e.to_string())?;
        a.push(step);
        Ok(())
    })
}

/// What the user does with a step's ghosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Answer {
    /// The ghosts (those named, or all) made edits, as one undo step; marks and clips sent to
    /// Moments.
    Accept,
    Reject,
    /// An accepted batch put back as the list had it, or sent marks taken back.
    Undo,
}

/// Applies `answer` to step `id` (ghosts `which`, or all; accepted batch `applied` for an undo) on
/// the open recording as `core` plays it: the edits first, then the file, so a refused edit records
/// nothing. A preview playing goes on with the ghosts still waiting, or stops when none is left.
pub fn answer(
    core: &Core,
    file: Option<&str>,
    cache_dir: Option<PathBuf>,
    answer: Answer,
    id: u64,
    which: Option<Vec<usize>>,
    applied: Option<u64>,
) -> Result<AskView, String> {
    let path = file.ok_or("no recording is open")?;
    let p = open(Path::new(path), cache_dir)?;
    let previewing = core.edit_state().preview;
    let v = change(&p, |a| {
        let edits = match answer {
            Answer::Accept => {
                let list = p.edit().map_err(|e| e.to_string())?;
                a.accept(id, which.as_deref(), &list.keep)
            }
            Answer::Reject => a.reject(id, which.as_deref()).map(|_| Vec::new()),
            Answer::Undo => a.undo(id, applied),
        }
        .map_err(|e| e.to_string())?;
        crate::moments::apply(core, file, &edits)
    })?;
    // The answer is saved: a preview that cannot follow (it would keep nothing) stops instead of
    // failing the answer, which the page would show over a stale list.
    if previewing && preview(core, &p, v.ask.waiting()).is_err() {
        let _ = core.set_preview(None);
    }
    Ok(v)
}

/// Plays the list as every waiting ghost would make it (`on`, when one waits), or the list itself
/// again.
fn preview(core: &Core, p: &Project, on: bool) -> Result<(), String> {
    if !on {
        return core.set_preview(None);
    }
    let list = p.edit().map_err(|e| e.to_string())?;
    let a = {
        let _held = lock(&FILES);
        ask::load(&p.dir).map_err(|e| e.to_string())?
    };
    // Nothing waiting (all answered, or a request that made none): the list itself plays.
    if !a.waiting() {
        return core.set_preview(None);
    }
    core.set_preview(Some(&a.preview(&list.keep)))
}

/// The preview turned on or off for the open recording.
pub fn set_preview(
    core: &Core,
    file: Option<&str>,
    cache_dir: Option<PathBuf>,
    on: bool,
) -> Result<(), String> {
    let path = file.ok_or("no recording is open")?;
    let p = open(Path::new(path), cache_dir)?;
    preview(core, &p, on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_without_the_model_is_read_by_the_keywords_and_kept() {
        let dir = std::env::temp_dir().join(format!("junctur-app-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("rec.mkv");
        std::fs::write(&source, b"").unwrap();
        let cache = dir.join("cache");
        let p = Project::open(&source, Some(cache.clone()), None).unwrap();
        // No edit list yet: refused, nothing kept.
        assert!(request(&source, Some(cache.clone()), "cut silences", None, None).is_err());
        let list = junctur_core::edit::EditList {
            source: source.clone(),
            duration: 30.0,
            settings: Default::default(),
            keep: vec![junctur_core::edit::Range {
                start: 0.0,
                end: 30.0,
            }],
            speakers: Default::default(),
            captions: None,
            fixes: Vec::new(),
            times: Vec::new(),
            windows: Vec::new(),
        };
        std::fs::write(p.edit_path(), list.to_text()).unwrap();
        let v = request(&source, Some(cache.clone()), "cut silences", None, None).unwrap();
        assert_eq!(v.ask.steps.len(), 1);
        let s = &v.ask.steps[0];
        assert_eq!(s.reading.reader, recipe::keywords::NAME);
        assert_eq!(s.reading.fallback.as_deref(), Some("the model is off"));
        // No speech found yet: the step says so rather than failing.
        assert!(
            s.error.as_deref().unwrap().contains("speech"),
            "{:?}",
            s.error
        );
        assert!(v.models.iter().all(|m| !m.loaded && !m.downloaded));
        // Asked again with a recipe of its own.
        let id = s.id;
        let r = serde_json::json!({"action": "cut_where", "when": {"speaker": "anyone"}});
        let v = rerun(&source, Some(cache.clone()), id, r).unwrap();
        assert_eq!(v.ask.steps.len(), 2);
        assert_eq!(v.ask.steps[1].reading.reader, "edited");
        assert_eq!(view(&source, Some(cache)).unwrap().ask, v.ask);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
