//! Junctur's desktop app: a Tauri window whose transparent WebView draws the UI over video that
//! libmpv renders underneath (see `video`). The engine is `junctur-core`, linked as a library.

// No console window in release builds; dev builds keep it for mpv's and the app's log lines.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod mpv;
mod player;
mod project;
mod state;
mod testmode;
#[cfg(windows)]
mod video;

use std::sync::Mutex;

use tauri::window::Color;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use commands::App;
use player::Player;

/// The app's dark background (`--panel-solid` in app.css), shown wherever no video is drawn.
const BACKGROUND: Color = Color(16, 17, 20, 0);

fn main() {
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
            let player = Player::start(app.handle().clone(), &window, extra);
            if let Err(e) = &player {
                eprintln!("junctur-app: no player: {e}");
            }
            app.manage(App {
                player: Mutex::new(player),
            });
            if let Some(secs) = test {
                testmode::arm(app.handle().clone(), secs);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Resized(size) = event {
                // Resizes during window creation come before the player is managed.
                let Some(app) = window.try_state::<App>() else {
                    return;
                };
                let guard = app.player.lock().unwrap_or_else(|e| e.into_inner());
                if let Ok(p) = &*guard {
                    p.resize(size.width, size.height);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::startup_file,
            commands::player_state,
            commands::open_file,
            commands::toggle_pause,
            commands::seek,
            commands::set_audio_track,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the app")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                let Some(state) = app.try_state::<App>() else {
                    return;
                };
                let mut guard = state.player.lock().unwrap_or_else(|e| e.into_inner());
                if let Ok(p) = &mut *guard {
                    p.shutdown();
                }
            }
        });
}
