//! Junctur's desktop app: a Tauri window whose transparent WebView draws the UI over video that
//! libmpv renders underneath (see `video`). The engine is `junctur-core`, linked as a library.

// No console window in release builds; dev builds keep it for mpv's and the app's log lines.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod commands;
mod cuts;
mod editor;
mod lane;
mod lock;
mod mixer;
mod mpv;
mod player;
mod project;
mod recent;
mod settings;
mod smartkeep;
mod state;
#[cfg(test)]
mod test_support;
// Test mode (the off-screen launch checks) is in dev builds, and in a release build only with
// `--features test-hooks`.
#[cfg(any(debug_assertions, feature = "test-hooks"))]
mod testmode;
#[cfg(not(any(debug_assertions, feature = "test-hooks")))]
#[path = "testmode/off.rs"]
mod testmode;
// Only test mode makes a trace.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
mod trace;
mod translation;
#[cfg(windows)]
mod video;
mod voices;
mod waves;
mod window;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, RunEvent, Window, WindowEvent};

use commands::App;
use lock::lock;
use player::Player;

fn main() {
    // Settings, voices and recent recordings an automated run saved in a hidden copy of the config
    // folder (a file pointed elsewhere by its variable, as in launch checks, is not taken).
    let [config, voices] = junctur_core::config::FILES;
    junctur_core::config::adopt_hidden(&[config, voices, ("recent.json", recent::ENV)]);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| Ok(setup(app)?))
        .on_window_event(resized)
        .invoke_handler(tauri::generate_handler![
            commands::startup_file,
            commands::player_state,
            commands::open_file,
            commands::current_recording,
            commands::recent,
            commands::forget_recent,
            commands::analysis,
            commands::reanalyse,
            commands::transcription,
            commands::pause_transcription,
            commands::toggle_pause,
            commands::seek,
            commands::play_between,
            commands::play_from,
            commands::pause_review,
            commands::end_review,
            commands::set_speed,
            commands::mixer,
            commands::set_muted,
            commands::set_skip,
            commands::edit_state,
            commands::transcript,
            commands::edit_words,
            commands::edit_frames,
            commands::move_edge,
            commands::rename_speaker,
            commands::fix_word,
            commands::confirm_word,
            commands::fix_words,
            commands::keep_heard,
            commands::fixes_set_aside,
            commands::forget_fixes_set_aside,
            commands::correct,
            commands::recolor_speaker,
            commands::set_captions,
            commands::export_captions,
            commands::caption_cues,
            commands::send_to_resolve,
            commands::translate_captions,
            commands::cancel_translation,
            commands::translation_status,
            commands::translation_languages,
            commands::resolve_templates,
            commands::voices,
            commands::enroll_voice,
            commands::forget_voice,
            commands::reanalyze_speakers,
            commands::undo,
            commands::redo,
            commands::set_video_margins,
            commands::set_solo,
            commands::turns,
            commands::waves_state,
            commands::waves,
            commands::labels,
            commands::keep_view,
            commands::keep_judge,
            commands::keep_prompt,
            commands::keep_answer,
            commands::recent_info,
            commands::thumbnail,
            commands::reveal,
            commands::close_file,
            commands::settings,
            commands::save_settings,
            commands::save_caption_defaults,
            commands::outdated,
            commands::snap_layouts,
            commands::ui_log,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the app")
        .run(exited);
}

/// The window, then the player attached to it, managed before the page can ask for anything; in
/// test mode hidden, muted and armed.
fn setup(app: &mut tauri::App) -> tauri::Result<()> {
    let test = testmode::seconds();
    let window = window::create(app, test.is_some())?;
    let extra = if test.is_some() {
        testmode::SILENT
    } else {
        &[]
    };
    let trace = testmode::trace();
    let player = Player::start(app.handle().clone(), &window, extra, trace.clone());
    match &player {
        Ok(p) => p.set_caption_defaults(settings::caption_defaults()),
        Err(e) => eprintln!("junctur-app: no player: {e}"),
    }
    app.manage(App {
        player: Mutex::new(player),
        analysis: Default::default(),
        waves: Default::default(),
        translation: Default::default(),
        judging: Default::default(),
    });
    if let Some(secs) = test {
        testmode::arm(app.handle().clone(), secs, trace);
    }
    Ok(())
}

/// The video follows the window's size.
fn resized(window: &Window, event: &WindowEvent) {
    if let WindowEvent::Resized(size) = event {
        // Resizes during window creation come before the player is managed.
        let Some(app) = window.try_state::<App>() else {
            return;
        };
        let guard = lock(&app.player);
        if let Ok(p) = &*guard {
            p.resize(size.width, size.height);
        }
    }
}

/// mpv is stopped before the process ends ([`Player::shutdown`]).
fn exited(app: &AppHandle, event: RunEvent) {
    if let RunEvent::Exit = event {
        let Some(state) = app.try_state::<App>() else {
            return;
        };
        let mut guard = lock(&state.player);
        if let Ok(p) = &mut *guard {
            p.shutdown();
        }
    }
}
