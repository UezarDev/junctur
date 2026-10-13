//! The frameless window's one native piece (2i): Windows 11's snap layouts.
//!
//! The title bar is drawn by the page, and Windows shows its layout flyout only while the pointer
//! rests on a native maximise button (`HTMAXBUTTON` from `WM_NCHITTEST`). The bar's button lives
//! inside WebView2's own child window, which takes the pointer before the frame could answer, so
//! the page asks for the flyout instead when the pointer rests on its maximise button, and this
//! sends Win+Z, the shell's own shortcut for it. Rejected: subclassing the window to answer
//! `HTMAXBUTTON` (the WebView's child gets the mouse first), and a native caption strip above the
//! WebView (painted on its own, splitting the page in two).

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
