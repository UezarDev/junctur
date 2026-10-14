//! Playback: pause, seek, review's loop and speed, the skip toggle, mutes, margins and waveforms.

use tauri::{AppHandle, Manager, State};

use super::{blocking, App};
use crate::mixer::MixerState;
use crate::state::PlayerState;
use crate::waves::WavesState;

#[tauri::command]
pub async fn player_state(app: State<'_, App>) -> Result<PlayerState, String> {
    app.with(|p| Ok(p.state()))
}

/// Plays only the kept ranges (`on`) or the whole file.
#[tauri::command]
pub async fn set_skip(app: State<'_, App>, on: bool) -> Result<(), String> {
    app.with(|p| p.set_skip(on))
}

#[tauri::command]
pub async fn toggle_pause(app: State<'_, App>) -> Result<(), String> {
    app.with(|p| p.toggle_pause())
}

/// Plays source seconds `start` to `end` on a loop (review's one clip), from `start`; the first
/// call puts the player in review (the whole file plays until `end_review`).
#[tauri::command]
pub async fn play_between(app: State<'_, App>, start: f64, end: f64) -> Result<(), String> {
    app.with(|p| p.play_between(start, end))
}

/// Plays from source second `at` (from just before a dragged cut edge, so the move is heard).
#[tauri::command]
pub async fn play_from(app: State<'_, App>, at: f64) -> Result<(), String> {
    app.with(|p| p.play_from(at))
}

/// Stops the loop and pauses, still in review (past the last clip).
#[tauri::command]
pub async fn pause_review(app: State<'_, App>) -> Result<(), String> {
    app.with(|p| p.pause_review())
}

/// Leaves review: no loop, 1x, the skip toggle as it was.
#[tauri::command]
pub async fn end_review(app: State<'_, App>) -> Result<(), String> {
    app.with(|p| p.end_review())
}

/// Playback speed, a quarter to four times.
#[tauri::command]
pub async fn set_speed(app: State<'_, App>, speed: f64) -> Result<(), String> {
    app.with(|p| p.set_speed(speed))
}

/// Returns the source time the seek lands on (a cut lands on the next kept range while skipping).
#[tauri::command]
pub async fn seek(app: State<'_, App>, seconds: f64, exact: bool) -> Result<f64, String> {
    app.with(|p| p.seek(seconds, exact))
}

/// The open recording's tracks with their mutes and roles.
#[tauri::command]
pub async fn mixer(app: State<'_, App>) -> Result<MixerState, String> {
    app.with(|p| Ok(p.mixer()))
}

/// Mutes or unmutes audio track `track` (0-based) in the preview, remembered for the recording.
/// Playback only, so allowed while the recording is analysed.
#[tauri::command]
pub async fn set_muted(app: State<'_, App>, track: usize, muted: bool) -> Result<(), String> {
    app.core()?.set_muted(track, muted)
}

/// The shares of the window on the right (the side panel) and at the bottom (play bar and lanes)
/// kept free of video.
#[tauri::command]
pub async fn set_video_margins(
    app: State<'_, App>,
    right: f64,
    bottom: f64,
    top: f64,
) -> Result<(), String> {
    app.core()?.set_video_margins(right, bottom, top)
}

/// Solos or unsolos audio track `track` (0-based) in the preview; not remembered.
#[tauri::command]
pub async fn set_solo(app: State<'_, App>, track: usize, on: bool) -> Result<(), String> {
    app.core()?.set_solo(track, on)
}

/// How far the open recording's waveforms are.
#[tauri::command]
pub async fn waves_state(app: State<'_, App>) -> Result<Option<WavesState>, String> {
    Ok(app.waves.state())
}

/// Each track's peak per column of `[from, to)` seconds, for the lanes to draw, as raw bytes
/// ([`WavesView::bytes`](crate::waves::WavesView::bytes); none when no recording is open): asked on every pan, zoom and step of the
/// read, where a JSON array of numbers would be several times the size and parsed each time.
#[tauri::command]
pub async fn waves(
    handle: AppHandle,
    from: f64,
    to: f64,
    columns: usize,
) -> Result<tauri::ipc::Response, String> {
    let bytes = blocking(move || {
        Ok(handle
            .state::<App>()
            .waves
            .columns(from, to, columns)
            .map_or_else(Vec::new, |v| v.bytes()))
    })
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}
