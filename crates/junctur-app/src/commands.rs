//! The commands the frontend invokes. Thin: each forwards to the player or the project.

use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;

use crate::player::Player;
use crate::project::{self, ProjectStatus};
use crate::state::PlayerState;

/// The player, or why it could not start (libmpv missing, no GL): the window still opens and
/// says so instead of the app failing to launch.
pub struct App {
    pub player: Mutex<Result<Player, String>>,
}

impl App {
    fn with<T>(&self, f: impl FnOnce(&Player) -> Result<T, String>) -> Result<T, String> {
        let guard = self.player.lock().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            Ok(p) => f(p),
            Err(e) => Err(e.clone()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub project: Option<ProjectStatus>,
    /// The file plays even when its project cannot be opened (e.g. an unwritable cache).
    pub project_error: Option<String>,
}

/// A file named on the command line (`junctur-app <file>`, or "Open with"), for the page to open
/// once it is ready.
#[tauri::command]
pub fn startup_file() -> Option<String> {
    std::env::args().nth(1)
}

#[tauri::command]
pub fn player_state(app: State<App>) -> Result<PlayerState, String> {
    app.with(|p| Ok(p.state()))
}

#[tauri::command]
pub fn open_file(app: State<App>, path: String) -> Result<Opened, String> {
    app.with(|p| p.open(&path))?;
    Ok(match project::status(Path::new(&path), None) {
        Ok(s) => Opened {
            project: Some(s),
            project_error: None,
        },
        Err(e) => Opened {
            project: None,
            project_error: Some(e),
        },
    })
}

#[tauri::command]
pub fn toggle_pause(app: State<App>) -> Result<(), String> {
    app.with(Player::toggle_pause)
}

#[tauri::command]
pub fn seek(app: State<App>, seconds: f64, exact: bool) -> Result<(), String> {
    app.with(|p| p.seek(seconds, exact))
}

#[tauri::command]
pub fn set_audio_track(app: State<App>, id: i64) -> Result<(), String> {
    app.with(|p| p.set_audio_track(id))
}
