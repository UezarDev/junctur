//! A launch check that disturbs nobody: `JUNCTUR_TEST_OFFSCREEN=<seconds>` opens the window far
//! off-screen, unfocused and out of the taskbar, plays silently, prints what the player reached
//! (state, new frames drawn, and a verdict) as one `junctur-app test:` JSON line on stdout, and
//! exits.
//!
//! `JUNCTUR_TEST_STEPS="2:seek=130.5;3:kseek=200;4:skip=off;6:skip=on;7:pause"` also runs player
//! actions at those seconds (`kseek` is a keyframe seek, as a dragged timeline sends), and the
//! report gives the state each one led to (sampled just before the next) and the skip trace (see
//! `trace`): every range boundary crossed with its frames and stall.
//!
//! Edit steps (2c) run the editor as the frontend does: `cutw=12-15` / `keepw=12-15` (transcript
//! words, by index), `cutf=300-450` / `keepf=300-450` (source frames), `undo`, `redo`, and
//! `rename=<id>:<name>` (5a: a speaker's name; no `;` in it), `color=<id>:#RRGGBB`,
//! `captions=on|off` and `srt=<path>` (write the captions there), `preview=on|off` (5d: the
//! captions over the picture; each caption it shows is a `caption preview` line in `ui`),
//! `enroll=<first>-<last>:<name>` (5b: remember a voice from transcript words) and `forget=<name>`. Set `JUNCTUR_VOICES` to a
//! scratch file for these, or they change the user's own voices. `reanalyse` (2d) analyses the open
//! recording again, replacing user edits (a recording without an edit list is analysed as it
//! opens); the report's `analysis` is the open recording's analysis progress and `outdated` what
//! the project panel would offer a re-analysis for. Set `JUNCTUR_RECENT` and `JUNCTUR_CACHE_DIR` to
//! scratch places too. `mute=<n>` / `unmute=<n>` (2e) mute or unmute audio track n (from 1, as
//! the play bar numbers them); each step's report and the final one add the mutes (`mixer`) and
//! the levels read then (`levels`: each track's meter and what is heard, in dBFS); `solo=<n>` /
//! `unsolo=<n>` (2g) solo or unsolo one, and `menu=<id>` chooses a menu item as a click on it does
//! (`menu=talking` shows the overlay of who is talking; 2i: the title bar's menus, so also
//! `menu=close`, back to the start window, and `menu=settings`, the Settings dialog). The final
//! report adds how far the waveforms are (`waves`) and the settings the dialog shows (`settings`;
//! set `JUNCTUR_CONFIG` to a scratch file before anything saves them);
//! the picture's shape is in the `state` (`width`, `height`, `aspect`). Each step's
//! report adds the edit state it left, and the final report re-reads `edit.junctur` from disk
//! (`disk`: does it parse, and is it the list in play), counts the loads asked of mpv (a save must
//! not come back as a second reload) and lists the frontend's log lines (`ui`).
//!
//! Embedding can only be checked in a real window, so this is the way a script (or a later phase)
//! runs the app without putting a window or a sound in front of the user.

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
/// App.svelte's `onMenu` (2i; 2g's native menu sent the same).
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
    /// 5d: the captions over the picture.
    Preview(bool),
    Srt(String),
    Enroll(usize, usize, String),
    Forget(String),
    Reanalyse,
    /// Audio track (0-based) and whether to mute it.
    Mute(usize, bool),
    /// Audio track (0-based) and whether to solo it (2g).
    Solo(usize, bool),
    /// A menu item chosen (2g), by id.
    Menu(String),
}

/// The frontend's log lines (`ui_log`), kept for the report.
static UI_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A line from the frontend: printed and kept for the report, in test mode only. Outside it the
/// lines are dropped: the caption preview (5d) sends one per caption shown, which an ordinary
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
            None if action == "pause" => Action::Pause,
            None if action == "undo" => Action::Edit(Step::Undo),
            None if action == "redo" => Action::Edit(Step::Redo),
            None if action == "reanalyse" => Action::Reanalyse,
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
    if let Action::Reanalyse = a {
        let file = with_player(app, |p| p.file())?.ok_or("no file is open")?;
        let state = app.try_state::<App>().ok_or("app not set up")?;
        let notify = crate::commands::notifier(app.clone());
        let r = state
            .analysis
            .reanalyse(std::path::Path::new(&file), true, notify)?;
        eprintln!(
            "junctur-app test: reanalyse started: {}",
            r.progress.is_some()
        );
        return Ok(());
    }
    with_player(app, |p| match a {
        Action::Seek(s) => p.seek(*s, true).map(|_| ()),
        Action::KeySeek(s) => p.seek(*s, false).map(|_| ()),
        Action::Skip(on) => p.set_skip(*on),
        Action::Pause => p.toggle_pause(),
        Action::Edit(step) => p.edit(*step),
        Action::Rename(id, name) => p.rename_speaker(*id, name),
        Action::Recolor(id, color) => p.recolor_speaker(*id, color),
        Action::Captions(on) => {
            let c = p.edit_state().captions;
            p.set_captions(junctur_core::edit::Captions { on: *on, ..c })
        }
        Action::Preview(on) => {
            let c = p.edit_state().captions;
            p.set_captions(junctur_core::edit::Captions { preview: *on, ..c })
        }
        Action::Srt(path) => p.export_captions(std::path::Path::new(path)).map(|f| {
            eprintln!("junctur-app test: wrote {} captions to {path}", f.cues);
        }),
        Action::Enroll(first, last, name) => p
            .enroll_voice(*first, *last, name)
            .map(|line| eprintln!("junctur-app test: {line}")),
        Action::Forget(name) => p.forget_voice(name),
        Action::Mute(track, muted) => p.set_muted(*track, *muted),
        Action::Solo(track, on) => p.set_solo(*track, *on),
        Action::Reanalyse | Action::Menu(_) => unreachable!("run above"),
    })?
}

fn snapshot(app: &AppHandle) -> Result<(PlayerState, u64), String> {
    with_player(app, |p| (p.state(), p.frames()))
}

/// The mutes and the levels now (2e).
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
mod tests {
    use super::*;

    #[test]
    fn playing_needs_frames_and_a_moving_position() {
        assert_eq!(verdict(false, 0, None, None), "idle");
        assert_eq!(verdict(true, 120, Some(1.0), Some(3.0)), "playing");
        // Redraws of a still frame, or a position that never moved, are not playback.
        assert_eq!(verdict(true, 0, Some(1.0), Some(3.0)), "stalled");
        assert_eq!(verdict(true, 120, Some(2.0), Some(2.0)), "stalled");
        assert_eq!(verdict(true, 120, None, Some(2.0)), "stalled");
    }

    #[test]
    fn parses_positive_seconds_only() {
        assert_eq!(parse(Some("8")), Some(8.0));
        assert_eq!(parse(Some(" 2.5 ")), Some(2.5));
        assert_eq!(parse(Some("0")), None);
        assert_eq!(parse(Some("-1")), None);
        assert_eq!(parse(Some("yes")), None);
        assert_eq!(parse(None), None);
    }

    #[test]
    fn parses_steps_in_time_order() {
        let s = parse_steps("4:skip=off; 2:seek=130.5 ;6:skip=on;7:pause;8:kseek=1").unwrap();
        assert_eq!(
            s,
            vec![
                (2.0, Action::Seek(130.5)),
                (4.0, Action::Skip(false)),
                (6.0, Action::Skip(true)),
                (7.0, Action::Pause),
                (8.0, Action::KeySeek(1.0)),
            ]
        );
        let s = parse_steps("9:reanalyse").unwrap();
        assert_eq!(s, vec![(9.0, Action::Reanalyse)]);
        let s = parse_steps("1:cutw=3-5;2:keepf=30-60;3:undo;4:redo").unwrap();
        assert_eq!(
            s.iter().map(|x| x.1.clone()).collect::<Vec<_>>(),
            vec![
                Action::Edit(Step::Do(Op::Words {
                    first: 3,
                    last: 5,
                    keep: false
                })),
                Action::Edit(Step::Do(Op::Frames {
                    start: 30,
                    end: 60,
                    keep: true
                })),
                Action::Edit(Step::Undo),
                Action::Edit(Step::Redo),
            ]
        );
        assert_eq!(
            parse_steps("5:rename=2:Ana María").unwrap(),
            vec![(5.0, Action::Rename(2, "Ana María".into()))]
        );
        assert_eq!(
            parse_steps("1:color=2:#FF0000;2:captions=on;3:srt=C:\\x\\out.srt;4:preview=on")
                .unwrap(),
            vec![
                (1.0, Action::Recolor(2, "#FF0000".into())),
                (2.0, Action::Captions(true)),
                (3.0, Action::Srt("C:\\x\\out.srt".into())),
                (4.0, Action::Preview(true)),
            ]
        );
        assert_eq!(
            parse_steps("1:enroll=10-40:Ana María;2:forget=Ana María").unwrap(),
            vec![
                (1.0, Action::Enroll(10, 40, "Ana María".into())),
                (2.0, Action::Forget("Ana María".into())),
            ]
        );
        assert_eq!(
            parse_steps("1:mute=2;2:unmute=1").unwrap(),
            vec![(1.0, Action::Mute(1, true)), (2.0, Action::Mute(0, false))]
        );
        assert!(parse_steps("1:mute=0").is_err(), "tracks count from 1");
        assert_eq!(
            parse_steps("1:solo=2;2:unsolo=2").unwrap(),
            vec![(1.0, Action::Solo(1, true)), (2.0, Action::Solo(1, false))]
        );
        assert!(parse_steps("1:solo=0").is_err());
        assert_eq!(
            parse_steps("3:menu=talking").unwrap(),
            vec![(3.0, Action::Menu("talking".into()))]
        );
        assert!(parse_steps("1:mute=x").is_err());
        assert!(parse_steps("1:enroll=10:Ana").is_err());
        assert!(parse_steps("1:forget=").is_err());
        assert!(parse_steps("5:captions=maybe").is_err());
        assert!(parse_steps("5:rename=x:Ana").is_err());
        assert!(parse_steps("5:rename=2").is_err());
        assert!(parse_steps("1:cutw=3").is_err());
        assert!(parse_steps("2:jump=3").is_err());
        assert!(parse_steps("x:pause").is_err());
        assert_eq!(parse_steps("").unwrap(), vec![]);
    }
}
