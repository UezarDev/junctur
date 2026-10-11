//! A launch check that disturbs nobody: `JUNCTUR_TEST_OFFSCREEN=<seconds>` opens the window far
//! off-screen, unfocused and out of the taskbar, plays silently, prints what the player reached
//! (state, new frames drawn, and a verdict) as one `junctur-app test:` JSON line on stdout, and
//! exits.
//!
//! Embedding can only be checked in a real window, so this is the way a script (or a later phase)
//! runs the app without putting a window or a sound in front of the user.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::commands::App;
use crate::lock::lock;
use crate::state::PlayerState;

pub const ENV: &str = "JUNCTUR_TEST_OFFSCREEN";

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

/// Samples the position halfway and at the end of `seconds`, then reports and exits.
pub fn arm(app: AppHandle, seconds: f64) {
    std::thread::spawn(move || {
        let sample = |app: &AppHandle| -> Result<(PlayerState, u64), String> {
            let state = app.try_state::<App>().ok_or("app not set up")?;
            let guard = lock(&state.player);
            let p = guard.as_ref().map_err(|e| e.clone())?;
            Ok((p.state(), p.frames()))
        };
        let half = Duration::from_secs_f64(seconds / 2.0);
        std::thread::sleep(half);
        let first = sample(&app).ok().and_then(|(s, _)| s.position);
        std::thread::sleep(half);
        let report = match sample(&app) {
            Ok((state, frames)) => serde_json::json!({
                "verdict": verdict(state.path.is_some(), frames, first, state.position),
                "frames": frames,
                "state": state,
            }),
            Err(e) => serde_json::json!({ "verdict": "error", "error": e }),
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
}
