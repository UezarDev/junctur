//! Settings, the window's snap layouts and the page's log.

use junctur_core::edit::Captions;
use tauri::State;

use super::App;
use crate::settings::{self, Settings, SettingsView};

/// The Settings dialog's contents.
#[tauri::command]
pub async fn settings() -> SettingsView {
    settings::read()
}

/// Saves the Settings dialog. Nothing is analysed again: the page asks for `outdated` after.
#[tauri::command]
pub async fn save_settings(
    app: State<'_, App>,
    settings: Settings,
) -> Result<SettingsView, String> {
    let view = settings::save(&settings)?;
    if let Ok(core) = app.core() {
        core.set_caption_defaults(view.settings.captions.clone());
    }
    Ok(view)
}

/// The Captions panel's "Save as default": these become the caption defaults.
#[tauri::command]
pub async fn save_caption_defaults(
    app: State<'_, App>,
    captions: Captions,
) -> Result<SettingsView, String> {
    let view = settings::save_caption_defaults(captions)?;
    if let Ok(core) = app.core() {
        core.set_caption_defaults(view.settings.captions.clone());
    }
    Ok(view)
}

/// Opens Windows' snap layouts for the window, when the pointer rests on the title bar's maximise
/// button (see `window`).
#[tauri::command]
pub fn snap_layouts(window: tauri::WebviewWindow) -> Result<(), String> {
    crate::window::snap_layouts(&window)
}

/// A line from the frontend for the app's log (timings test mode reports).
#[tauri::command]
pub fn ui_log(message: String) {
    crate::testmode::ui_log(message);
}
