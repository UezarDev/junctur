//! A launch check that disturbs nobody: `JUNCTUR_TEST_OFFSCREEN=<seconds>` opens the window far
//! off-screen, unfocused and out of the taskbar, plays silently, prints what the player reached
//! (state and frames drawn) as one `junctur-app test:` JSON line on stdout, and exits.
//!
//! Embedding can only be checked in a real window, so this is the way a script (or a later phase)
//! runs the app without putting a window or a sound in front of the user.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::commands::App;

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

/// Reports and exits after `seconds`.
pub fn arm(app: AppHandle, seconds: f64) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(seconds));
        let report = match app.try_state::<App>() {
            Some(state) => {
                let guard = state.player.lock().unwrap_or_else(|e| e.into_inner());
                match &*guard {
                    Ok(p) => serde_json::json!({ "state": p.state(), "frames": p.frames() }),
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            None => serde_json::json!({ "error": "app not set up" }),
        };
        println!("junctur-app test: {report}");
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
