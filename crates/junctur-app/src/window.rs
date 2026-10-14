//! The main window: frameless over the video, and its one native piece, Windows 11's snap layouts.
//! The page's maximise button sits in WebView2's child window, which takes the pointer before the
//! frame could answer `HTMAXBUTTON`, so the page asks for the flyout and this sends Win+Z.

use tauri::window::Color;
use tauri::{WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::testmode;

/// The app's dark background (`--panel-solid` in app.css), shown wherever no video is drawn.
const BACKGROUND: Color = Color(16, 17, 20, 0);

/// Builds the main window, before the player attaches to it. Alpha 0 makes only the WebView
/// transparent (not `.transparent(true)`, which shows the desktop through). Frameless: the page
/// draws the title bar. No shadow: its hidden frame made captures of the window come back blank.
pub fn create(app: &tauri::App, test: bool) -> tauri::Result<WebviewWindow> {
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
        .title("Junctur")
        .decorations(false)
        .shadow(false)
        .inner_size(1280.0, 800.0)
        .min_inner_size(640.0, 400.0)
        .background_color(BACKGROUND);
    if test {
        // Built hidden and moved before it is shown, so it never flashes on screen.
        builder = builder.visible(false).focused(false).skip_taskbar(true);
    }
    let window = builder.build()?;
    if test {
        testmode::place(&window)?;
    }
    Ok(window)
}

/// Opens the snap layouts flyout for `window`, only while it is the foreground window: Win+Z goes
/// to whichever window has the foreground, never to one the user is not in.
#[cfg(windows)]
pub fn snap_layouts(window: &tauri::WebviewWindow) -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
        VK_LWIN, VK_Z,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    // SAFETY: a plain query.
    if unsafe { GetForegroundWindow() } as isize != hwnd {
        return Ok(());
    }
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // Z between the Win key's press and release, so the Start menu does not open on its own.
    let inputs = [
        key(VK_LWIN, false),
        key(VK_Z, false),
        key(VK_Z, true),
        key(VK_LWIN, true),
    ];
    // SAFETY: a valid array of keyboard inputs and its element size.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != inputs.len() {
        return Err(format!(
            "the snap layouts shortcut was not sent: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn snap_layouts(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}
