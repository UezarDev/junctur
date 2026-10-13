//! Junctur's desktop app: a Tauri window whose transparent WebView draws the UI over video that
//! libmpv renders underneath (see `video`). The engine is `junctur-core`, linked as a library.

// No console window in release builds; dev builds keep it for mpv's and the app's log lines.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod commands;
mod cuts;
mod editor;
mod lock;
mod menu;
mod mixer;
mod mpv;
mod player;
mod project;
mod recent;
mod state;
mod testmode;
mod trace;
#[cfg(windows)]
mod video;
mod voices;
mod waves;

use std::sync::Mutex;

use tauri::window::Color;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use commands::App;
use lock::lock;
use player::Player;

/// The app's dark background (`--panel-solid` in app.css), shown wherever no video is drawn.
const BACKGROUND: Color = Color(16, 17, 20, 0);

fn main() {
    // 2f: settings, voices and recent recordings a Claude-launched run saved in a hidden copy of the
    // config folder (a file pointed elsewhere by its variable, as in launch checks, is not taken).
    let [config, voices] = junctur_core::config::FILES;
    junctur_core::config::adopt_hidden(&[config, voices, ("recent.json", recent::ENV)]);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Built here rather than in tauri.conf.json so the player can attach to it before the
            // page asks for anything. Alpha 0 makes only the WebView transparent, so the video
            // under it shows through; the window itself stays opaque (tao fills it with the RGB).
            // Rejected: `.transparent(true)`, which makes the whole window see-through (DWM
            // blur-behind): the desktop showed through, and the GL surface never did.
            let test = testmode::seconds();
            let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Junctur")
                .inner_size(1280.0, 800.0)
                .min_inner_size(640.0, 400.0)
                .background_color(BACKGROUND);

            if test.is_some() {
                // Built hidden and moved before it is shown, so it never flashes on screen.
                builder = builder.visible(false).focused(false).skip_taskbar(true);
            }
            let window = builder.build()?;
            if test.is_some() {
                testmode::place(&window)?;
            }
            let extra = if test.is_some() {
                testmode::SILENT
            } else {
                &[]
            };
            let trace = testmode::trace();
            let player = Player::start(app.handle().clone(), &window, extra, trace.clone());
            // The menu goes on once the video surface exists: built with the window, it left the
            // picture invisible in every launch check (the surface was there and drawing). It
            // takes its height from the client area without a resize event, so the surface is fitted
            // to what is left by hand.
            window.set_menu(menu::build(app.handle())?)?;
            if let (Ok(p), Ok(size)) = (&player, window.inner_size()) {
                p.resize(size.width, size.height);
            }
            if let Err(e) = &player {
                eprintln!("junctur-app: no player: {e}");
            }
            app.manage(App {
                player: Mutex::new(player),
                analysis: Default::default(),
                waves: Default::default(),
            });
            if let Some(secs) = test {
                testmode::arm(app.handle().clone(), secs, trace);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
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
        })
        .invoke_handler(tauri::generate_handler![
            commands::startup_file,
            commands::player_state,
            commands::open_file,
            commands::recent,
            commands::forget_recent,
            commands::analysis,
            commands::reanalyse,
            commands::toggle_pause,
            commands::seek,
            commands::mixer,
            commands::set_muted,
            commands::set_skip,
            commands::edit_state,
            commands::transcript,
            commands::edit_words,
            commands::edit_frames,
            commands::rename_speaker,
            commands::recolor_speaker,
            commands::set_captions,
            commands::export_captions,
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
            commands::recent_info,
            commands::thumbnail,
            commands::reveal,
            commands::set_menu_check,
            commands::ui_log,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                let Some(state) = app.try_state::<App>() else {
                    return;
                };
                let mut guard = lock(&state.player);
                if let Ok(p) = &mut *guard {
                    p.shutdown();
                }
            }
        });
}
