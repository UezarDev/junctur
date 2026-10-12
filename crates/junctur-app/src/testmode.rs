//! A launch check that disturbs nobody: `JUNCTUR_TEST_OFFSCREEN=<seconds>` opens the window far
//! off-screen, unfocused and out of the taskbar, plays silently, prints what the player reached
//! (state, new frames drawn, and a verdict) as one `junctur-app test:` JSON line on stdout, and
//! exits.
//!
//! `JUNCTUR_TEST_STEPS="2:seek=130.5;4:skip=off;6:skip=on;7:pause"` also runs player actions at
//! those seconds, and the report gives the state each one led to (sampled just before the next)
//! and the skip trace (see `trace`): every range boundary crossed with its frames and stall.
//!
//! Embedding can only be checked in a real window, so this is the way a script (or a later phase)
//! runs the app without putting a window or a sound in front of the user.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::commands::App;
use crate::lock::lock;
use crate::player::Player;
use crate::state::PlayerState;
use crate::trace::Trace;

pub const ENV: &str = "JUNCTUR_TEST_OFFSCREEN";
pub const STEPS_ENV: &str = "JUNCTUR_TEST_STEPS";

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
    Skip(bool),
    Pause,
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
            Some(("skip", "on")) => Action::Skip(true),
            Some(("skip", "off")) => Action::Skip(false),
            None if action == "pause" => Action::Pause,
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

fn run(app: &AppHandle, a: &Action) -> Result<(), String> {
    with_player(app, |p| match a {
        Action::Seek(s) => p.seek(*s, true),
        Action::Skip(on) => p.set_skip(*on),
        Action::Pause => p.toggle_pause(),
    })?
}

fn snapshot(app: &AppHandle) -> Result<(PlayerState, u64), String> {
    with_player(app, |p| (p.state(), p.frames()))
}

/// A step that ran, waiting for the state it led to.
struct Ran<'a> {
    at: f64,
    action: &'a Action,
    error: Option<String>,
}

impl Ran<'_> {
    fn report(self, app: &AppHandle) -> Value {
        let after = snapshot(app).ok().map(|s| s.0);
        json!({
            "at": self.at,
            "action": format!("{:?}", self.action),
            "error": self.error,
            "after": after,
        })
    }
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
            last = Some(Ran {
                at: *at,
                action,
                error: run(&app, action).err(),
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
        let s = parse_steps("4:skip=off; 2:seek=130.5 ;6:skip=on;7:pause;").unwrap();
        assert_eq!(
            s,
            vec![
                (2.0, Action::Seek(130.5)),
                (4.0, Action::Skip(false)),
                (6.0, Action::Skip(true)),
                (7.0, Action::Pause),
            ]
        );
        assert!(parse_steps("2:jump=3").is_err());
        assert!(parse_steps("x:pause").is_err());
        assert_eq!(parse_steps("").unwrap(), vec![]);
    }
}
