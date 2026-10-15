//! A launch check that disturbs nobody: `JUNCTUR_TEST_OFFSCREEN=<seconds>` opens the window far
//! off-screen, unfocused and silent, prints what the player reached as one `junctur-app test:`
//! JSON line on stdout, and exits. Embedding can only be checked in a real window.
//!
//! `JUNCTUR_TEST_STEPS="<seconds>:<step>;..."` runs steps at those seconds, each reported with the
//! state it led to: `seek=<s>`, `kseek=<s>` (keyframe), `skip=on|off`, `pause`,
//! `cutw|keepw=<a>-<b>` (word indices), `cutf|keepf=<a>-<b>` (frames), `undo`, `redo`,
//! `rename=<id>:<name>`, `color=<id>:#RRGGBB`, `captions=on|off`, `preview=on|off`, `srt=<path>`,
//! `enroll=<a>-<b>:<name>`, `forget=<name>`, `reanalyse`, `reanalyse_fresh`, `mute|unmute=<n>`,
//! `solo|unsolo=<n>` (tracks from 1), `menu=<id>`, `speed=<x>`, `loop=<a>-<b>`, `unloop`, `reload`.
//! Point `JUNCTUR_VOICES`, `JUNCTUR_RECENT`, `JUNCTUR_CACHE_DIR` and `JUNCTUR_CONFIG` at scratch
//! places, or the steps change the user's own files.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::commands::App;
use crate::editor::{Op, Step};
use crate::lock::lock;
use crate::player::Player;
use crate::state::PlayerState;
use crate::trace::Trace;

pub const ENV: &str = "JUNCTUR_TEST_OFFSCREEN";
pub const STEPS_ENV: &str = "JUNCTUR_TEST_STEPS";
/// The event a `menu=<id>` step is sent to the page as, as the title bar's own items go to
/// App.svelte's `onMenu`.
const MENU_EVENT: &str = "menu";

/// Where the window goes: well outside any monitor layout.
const POSITION: (i32, i32) = (-12000, -12000);

/// mpv options added in test mode: no audio device at all.
pub const SILENT: &[(&str, &str)] = &[("ao", "null"), ("mute", "yes")];

/// Moves the (hidden) window off-screen, then shows it without taking focus. Shown, because a
/// hidden window's WebView and surface are not composed.
pub fn place(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    window.set_position(tauri::PhysicalPosition::new(POSITION.0, POSITION.1))?;
    window.show()?;
    // Some window managers move a window back on screen when it is shown; say where it ended up.
    eprintln!("junctur-app test: window at {:?}", window.outer_position()?);
    Ok(())
}

/// The test's run time, when test mode is on.
pub fn seconds() -> Option<f64> {
    parse(std::env::var(ENV).ok().as_deref())
}

fn parse(v: Option<&str>) -> Option<f64> {
    let s: f64 = v?.trim().parse().ok()?;
    (s > 0.0 && s.is_finite()).then_some(s)
}

#[derive(Debug, Clone, PartialEq)]
enum Action {
    Seek(f64),
    /// A keyframe seek, as the timeline sends while it is dragged.
    KeySeek(f64),
    Skip(bool),
    Pause,
    Edit(Step),
    Rename(u32, String),
    Recolor(u32, String),
    Captions(bool),
    /// The captions over the picture.
    Preview(bool),
    Srt(String),
    Enroll(usize, usize, String),
    Forget(String),
    /// Whether it is a fresh cut (the words and speakers heard again), else nothing redone.
    Reanalyse(bool),
    /// Audio track (0-based) and whether to mute it.
    Mute(usize, bool),
    /// Audio track (0-based) and whether to solo it.
    Solo(usize, bool),
    /// A menu item chosen, by id.
    Menu(String),
    /// Playback speed, and review's loop of one clip (source seconds) or review's end.
    Speed(f64),
    Loop(f64, f64),
    Unloop,
    /// The page loads again, the app going on.
    Reload,
}

/// The frontend's log lines (`ui_log`), kept for the report.
static UI_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A line from the frontend: printed and kept for the report, in test mode only. Outside it the
/// lines are dropped: the caption preview sends one per caption shown, which an ordinary
/// session has no use for on stderr.
pub fn ui_log(message: String) {
    if seconds().is_some() {
        eprintln!("junctur-app ui: {message}");
        lock(&UI_LOG).push(message);
    }
}

/// `"a-b"` as two whole numbers.
fn pair(v: &str) -> Option<(u64, u64)> {
    let (a, b) = v.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// `"<seconds>:<action>;..."`, sorted by time. Unknown steps are an error, so a typo in a check
/// script fails loudly instead of testing nothing.
fn parse_steps(s: &str) -> Result<Vec<(f64, Action)>, String> {
    let mut steps = Vec::new();
    for step in s.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let bad = || format!("bad test step {step:?}");
        let (at, action) = step.split_once(':').ok_or_else(bad)?;
        let at: f64 = at.trim().parse().map_err(|_| bad())?;
        let action = action.trim();
        let action = match action.split_once('=') {
            Some(("seek", v)) => Action::Seek(v.parse().map_err(|_| bad())?),
            Some(("kseek", v)) => Action::KeySeek(v.parse().map_err(|_| bad())?),
            Some(("skip", "on")) => Action::Skip(true),
            Some(("skip", "off")) => Action::Skip(false),
            Some((kind @ ("cutw" | "keepw" | "cutf" | "keepf"), v)) => {
                let (a, b) = pair(v).ok_or_else(bad)?;
                let keep = kind.starts_with("keep");
                Action::Edit(Step::Do(if kind.ends_with('w') {
                    Op::Words {
                        first: a as usize,
                        last: b as usize,
                        keep,
                    }
                } else {
                    Op::Frames {
                        start: a,
                        end: b,
                        keep,
                    }
                }))
            }
            Some(("rename", v)) => {
                let (id, name) = v.split_once(':').ok_or_else(bad)?;
                Action::Rename(id.trim().parse().map_err(|_| bad())?, name.to_string())
            }
            Some(("color", v)) => {
                let (id, color) = v.split_once(':').ok_or_else(bad)?;
                Action::Recolor(id.trim().parse().map_err(|_| bad())?, color.to_string())
            }
            Some(("captions", "on")) => Action::Captions(true),
            Some(("captions", "off")) => Action::Captions(false),
            Some(("preview", "on")) => Action::Preview(true),
            Some(("preview", "off")) => Action::Preview(false),
            Some(("srt", path)) if !path.is_empty() => Action::Srt(path.to_string()),
            Some(("enroll", v)) => {
                let (words, name) = v.split_once(':').ok_or_else(bad)?;
                let (a, b) = pair(words).ok_or_else(bad)?;
                Action::Enroll(a as usize, b as usize, name.to_string())
            }
            Some(("forget", name)) if !name.is_empty() => Action::Forget(name.to_string()),
            Some((kind @ ("mute" | "unmute"), v)) => {
                let n: usize = v.trim().parse().map_err(|_| bad())?;
                Action::Mute(n.checked_sub(1).ok_or_else(bad)?, kind == "mute")
            }
            Some((kind @ ("solo" | "unsolo"), v)) => {
                let n: usize = v.trim().parse().map_err(|_| bad())?;
                Action::Solo(n.checked_sub(1).ok_or_else(bad)?, kind == "solo")
            }
            Some(("menu", id)) if !id.is_empty() => Action::Menu(id.to_string()),
            Some(("speed", v)) => Action::Speed(v.trim().parse().map_err(|_| bad())?),
            Some(("loop", v)) => {
                let (a, b) = v.split_once('-').ok_or_else(bad)?;
                Action::Loop(
                    a.trim().parse().map_err(|_| bad())?,
                    b.trim().parse().map_err(|_| bad())?,
                )
            }
            None if action == "pause" => Action::Pause,
            None if action == "undo" => Action::Edit(Step::Undo),
            None if action == "redo" => Action::Edit(Step::Redo),
            None if action == "reanalyse" => Action::Reanalyse(false),
            None if action == "reanalyse_fresh" => Action::Reanalyse(true),
            None if action == "unloop" => Action::Unloop,
            None if action == "reload" => Action::Reload,
            _ => return Err(bad()),
        };
        steps.push((at, action));
    }
    steps.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(steps)
}

/// What a run showed: `playing` needs a loaded file, new frames drawn and the position moving
/// between the two samples (frames alone could be redraws of one still frame); `idle` is no file;
/// anything else is `stalled`.
fn verdict(loaded: bool, frames: u64, first: Option<f64>, last: Option<f64>) -> &'static str {
    match (loaded, first, last) {
        (false, _, _) => "idle",
        (true, Some(a), Some(b)) if frames > 0 && b > a => "playing",
        _ => "stalled",
    }
}

fn with_player<T>(app: &AppHandle, f: impl FnOnce(&Player) -> T) -> Result<T, String> {
    let state = app.try_state::<App>().ok_or("app not set up")?;
    let guard = lock(&state.player);
    let p = guard.as_ref().map_err(|e| e.clone())?;
    Ok(f(p))
}

/// The open recording's analysis progress.
fn analysis(app: &AppHandle) -> Option<crate::analysis::Progress> {
    let file = with_player(app, |p| p.file()).ok()??;
    app.try_state::<App>()?
        .analysis
        .progress(std::path::Path::new(&file))
}

fn run(app: &AppHandle, a: &Action) -> Result<(), String> {
    if let Action::Menu(id) = a {
        use tauri::Emitter;
        return app.emit(MENU_EVENT, id).map_err(|e| e.to_string());
    }
    if let Action::Reload = a {
        let window = app.get_webview_window("main").ok_or("no window")?;
        return window.eval("location.reload()").map_err(|e| e.to_string());
    }
    if let Action::Reanalyse(fresh) = *a {
        let file = with_player(app, |p| p.file())?.ok_or("no file is open")?;
        let state = app.try_state::<App>().ok_or("app not set up")?;
        let notify = crate::commands::notifier(app.clone());
        use crate::analysis::{Again, Reanalysis, Redo};
        // The pop-up with nothing ticked to redo and everything kept, or a fresh cut over the
        // user's edits with the words and the speakers heard again.
        let ask = if fresh {
            Reanalysis::whole(Again::RecutOverEdits, Redo::ALL)
        } else {
            Reanalysis::whole(Again::KeepList, Redo::NOTHING)
        };
        let r = state
            .analysis
            .reanalyse(std::path::Path::new(&file), ask, notify)?;
        eprintln!(
            "junctur-app test: reanalyse started: {}",
            r.progress.is_some()
        );
        return Ok(());
    }
    // Edits as the frontend's commands make them, through `App::editable`: refused while the
    // recording's list is to come, and holding a run of it off while they work.
    let edit = |f: &dyn Fn(&crate::commands::Editable) -> Result<(), String>| {
        let state = app.try_state::<App>().ok_or("app not set up")?;
        let e = state.editable()?;
        f(&e)
    };
    match a {
        Action::Edit(step) => return edit(&|e| e.edit(e.file(), *step)),
        Action::Rename(id, name) => return edit(&|e| e.rename_speaker(e.file(), *id, name)),
        Action::Recolor(id, color) => return edit(&|e| e.recolor_speaker(e.file(), *id, color)),
        Action::Captions(on) => {
            return edit(&|e| {
                let c = e.edit_state().captions;
                e.set_captions(e.file(), junctur_core::edit::Captions { on: *on, ..c })
            })
        }
        Action::Preview(on) => {
            return edit(&|e| {
                let c = e.edit_state().captions;
                e.set_captions(e.file(), junctur_core::edit::Captions { preview: *on, ..c })
            })
        }
        Action::Enroll(first, last, name) => {
            return edit(&|e| {
                e.enroll_voice(e.file(), &[(*first, *last)], name)
                    .map(|line| eprintln!("junctur-app test: {line}"))
            })
        }
        Action::Forget(name) => return edit(&|e| e.forget_voice(e.file(), name)),
        _ => {}
    }
    with_player(app, |p| match a {
        Action::Seek(s) => p.seek(*s, true).map(|_| ()),
        Action::KeySeek(s) => p.seek(*s, false).map(|_| ()),
        Action::Skip(on) => p.set_skip(*on),
        Action::Pause => p.toggle_pause(),
        Action::Srt(path) => p.export_captions(std::path::Path::new(path)).map(|f| {
            eprintln!("junctur-app test: wrote {} captions to {path}", f.cues);
        }),
        Action::Mute(track, muted) => p.set_muted(*track, *muted),
        Action::Solo(track, on) => p.set_solo(*track, *on),
        Action::Speed(x) => p.set_speed(*x),
        Action::Loop(a, b) => p.play_between(*a, *b),
        Action::Unloop => p.end_review(),
        _ => unreachable!("run above"),
    })?
}

fn snapshot(app: &AppHandle) -> Result<(PlayerState, u64), String> {
    with_player(app, |p| (p.state(), p.frames()))
}

/// The mutes and the levels now.
fn mixer(app: &AppHandle) -> Value {
    with_player(app, |p| {
        let (meters, heard) = p.levels();
        json!({ "mixer": p.mixer(), "levels": { "meters": meters, "heard": heard } })
    })
    .unwrap_or(Value::Null)
}

/// A step that ran, waiting for the state it led to.
struct Ran<'a> {
    at: f64,
    action: &'a Action,
    error: Option<String>,
    /// How long the action itself took (an edit: apply, save and hand to playback).
    ms: f64,
}

impl Ran<'_> {
    fn report(self, app: &AppHandle) -> Value {
        let after = snapshot(app).ok().map(|s| s.0);
        let edit = with_player(app, |p| p.edit_state()).ok().map(|e| {
            json!({
                "ranges": e.summary.as_ref().map(|s| s.ranges),
                "kept": e.summary.as_ref().map(|s| s.kept),
                "outputFrames": e.output_frames,
                "canUndo": e.can_undo,
                "canRedo": e.can_redo,
                "error": e.error,
                "notice": e.notice,
                "speakers": e.speakers,
            })
        });
        json!({
            "at": self.at,
            "action": format!("{:?}", self.action),
            "error": self.error,
            "ms": self.ms,
            "after": after,
            "edit": edit,
            "mixer": mixer(app),
            "speedLoop": with_player(app, |p| p.looping()).ok(),
        })
    }
}

/// `edit.junctur` as it is on disk now, against the list in play: it must parse and be the same
/// list, and the backups beside it are listed.
fn disk_check(p: &Player) -> Value {
    let (path, playing) = p.edit_file();
    let Some(path) = path else {
        return Value::Null;
    };
    let read = std::fs::read_to_string(&path).map_err(|e| e.to_string());
    let parsed = read.and_then(|t| junctur_core::edit::parse(&t));
    let backups: Vec<String> = std::fs::read_dir(path.parent().unwrap_or(&path))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(".bak"))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "parses": parsed.as_ref().err().cloned().map_or(Value::Bool(true), Value::String),
        "ranges": parsed.as_ref().ok().map(|l| l.keep.len()),
        "sameAsPlaying": parsed.ok().zip(playing).map(|(d, p)| d == *p),
        "backups": backups,
    })
}

/// The trace test mode records into, when test mode is on.
pub fn trace() -> Option<Arc<Trace>> {
    seconds().map(|_| Arc::new(Trace::new()))
}

/// Runs the steps, samples the position halfway and at the end of `seconds`, then reports and
/// exits.
pub fn arm(app: AppHandle, seconds: f64, trace: Option<Arc<Trace>>) {
    std::thread::spawn(move || {
        let began = Instant::now();
        let sleep_until = |at: f64| {
            let now = began.elapsed().as_secs_f64();
            if at > now {
                std::thread::sleep(Duration::from_secs_f64(at - now));
            }
        };
        let steps = match std::env::var(STEPS_ENV).map(|s| parse_steps(&s)) {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                let report = json!({ "verdict": "error", "error": e });
                println!("junctur-app test: {report}");
                app.exit(1);
                return;
            }
            Err(_) => Vec::new(),
        };
        let half = seconds / 2.0;
        let position = |app: &AppHandle| snapshot(app).ok().and_then(|(s, _)| s.position);
        let mut first = None;
        let mut sampled = false;
        let mut done: Vec<Value> = Vec::new();
        let mut last: Option<Ran> = None;
        for (at, action) in &steps {
            if !sampled && *at > half {
                sleep_until(half);
                first = position(&app);
                sampled = true;
            }
            sleep_until(*at);
            if let Some(r) = last.take() {
                done.push(r.report(&app));
            }
            let t0 = Instant::now();
            let error = run(&app, action).err();
            last = Some(Ran {
                at: *at,
                action,
                error,
                ms: t0.elapsed().as_secs_f64() * 1000.0,
            });
        }
        if !sampled {
            sleep_until(half);
            first = position(&app);
        }
        sleep_until(seconds);
        if let Some(r) = last.take() {
            done.push(r.report(&app));
        }
        let report = match snapshot(&app) {
            Ok((state, frames)) => json!({
                "verdict": verdict(state.path.is_some(), frames, first, state.position),
                "frames": frames,
                "state": state,
                "steps": done,
                "skips": trace.map(|t| t.report()),
                "loads": with_player(&app, |p| p.loads()).ok(),
                "disk": with_player(&app, disk_check).ok(),
                "analysis": analysis(&app),
                "outdated": with_player(&app, |p| p.file()).ok().flatten()
                    .map(|f| crate::analysis::outdated(std::path::Path::new(&f))),
                "mixer": mixer(&app),
                "waves": app.try_state::<App>().and_then(|a| a.waves.state()),
                "settings": crate::settings::read(),
                "margins": with_player(&app, |p| p.video_margins()).ok(),
                "ui": lock(&UI_LOG).clone(),
            }),
            Err(e) => json!({ "verdict": "error", "error": e }),
        };
        println!("junctur-app test: {report}");
        app.exit(0);
    });
}

#[cfg(test)]
mod tests;
