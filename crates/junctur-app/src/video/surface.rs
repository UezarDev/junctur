//! The native surface mpv draws on: a child window of the app window, under the WebView.
//!
//! A child window rather than the app window itself, because WebView2 already owns the app
//! window's client area; a sibling placed at the bottom of the z-order shows through wherever the
//! (transparent) page draws nothing. It ignores input (`HTTRANSPARENT`, disabled) so every click
//! and drop reaches the WebView above it, and it paints nothing itself: the render thread does.
//!
//! GL presents by blitting into the redirection surface the app window shares with its children,
//! so two clip rules had to go (each alone left the video hidden in the launch checks): the
//! surface has no `WS_CLIPSIBLINGS`, or wry's container window above it (which paints nothing)
//! would clip away every pixel; and the app window gets `WS_CLIPCHILDREN`, or tao's background
//! fill paints over the video.

use std::sync::Once;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetDC, ValidateRect};
use windows_sys::Win32::Graphics::OpenGL::{
    ChoosePixelFormat, SetPixelFormat, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_MAIN_PLANE,
    PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, RegisterClassW,
    SetWindowLongPtrW, SetWindowPos, CS_OWNDC, GWLP_USERDATA, GWL_STYLE, HTTRANSPARENT,
    HWND_BOTTOM, SWP_NOACTIVATE, WM_ERASEBKGND, WM_NCHITTEST, WM_PAINT, WNDCLASSW, WS_CHILD,
    WS_CLIPCHILDREN, WS_DISABLED, WS_VISIBLE,
};

const CLASS: &str = "JuncturVideoSurface";

type PaintFn = Box<dyn Fn() + Send + Sync>;

pub struct Surface {
    // Kept as integers so the owner can live in Tauri's (Send + Sync) state; the window is only
    // used through Win32 calls, which take any thread's HWND.
    hwnd: isize,
    /// The `PaintFn` the window procedure calls, owned here and freed after the window.
    paint: usize,
}

impl Surface {
    /// Creates the surface inside `parent`, sized `w`x`h` physical pixels. Call on the thread
    /// that owns `parent` (the UI thread), which then also dispatches the surface's messages.
    /// `on_paint` runs whenever Windows asks for the surface to be repainted (shown, restored,
    /// uncovered): a frame drawn while the window was hidden is lost, so the last one is redrawn.
    pub fn create(
        parent: isize,
        w: i32,
        h: i32,
        on_paint: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let class = wide(CLASS);
        static REGISTER: Once = Once::new();
        // SAFETY: plain Win32 calls with valid, NUL-terminated wide strings.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            REGISTER.call_once(|| {
                let wc = WNDCLASSW {
                    style: CS_OWNDC,
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
                    lpszClassName: class.as_ptr(),
                    ..std::mem::zeroed()
                };
                RegisterClassW(&wc);
            });
            // tao's window lacks WS_CLIPCHILDREN (see the module comment). No SWP_FRAMECHANGED:
            // that is for frame styles, and WS_CLIPCHILDREN only changes how the client area is
            // clipped, which Windows reads at the next paint.
            let style = GetWindowLongPtrW(parent as HWND, GWL_STYLE);
            SetWindowLongPtrW(parent as HWND, GWL_STYLE, style | WS_CLIPCHILDREN as isize);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_DISABLED,
                0,
                0,
                w,
                h,
                parent as HWND,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(format!(
                    "could not create the video surface: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let paint: *mut PaintFn = Box::into_raw(Box::new(Box::new(on_paint)));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, paint as isize);
            let s = Self {
                hwnd: hwnd as isize,
                paint: paint as usize,
            };
            s.set_pixel_format()?;
            s.fit(w, h);
            Ok(s)
        }
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Resizes to the parent's client area and keeps the surface under the WebView.
    pub fn fit(&self, w: i32, h: i32) {
        // SAFETY: a live child window.
        unsafe {
            SetWindowPos(
                self.hwnd as HWND,
                HWND_BOTTOM,
                0,
                0,
                w.max(0),
                h.max(0),
                SWP_NOACTIVATE,
            );
        }
    }

    /// A double-buffered RGBA pixel format; set once per window, before any GL context exists.
    fn set_pixel_format(&self) -> Result<(), String> {
        // SAFETY: CS_OWNDC gives the window one DC for its lifetime, so it is never released.
        unsafe {
            let dc = GetDC(self.hwnd as HWND);
            let pfd = PIXELFORMATDESCRIPTOR {
                nSize: size_of::<PIXELFORMATDESCRIPTOR>() as u16,
                nVersion: 1,
                dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
                iPixelType: PFD_TYPE_RGBA,
                cColorBits: 32,
                iLayerType: PFD_MAIN_PLANE as u8,
                ..std::mem::zeroed()
            };
            let format = ChoosePixelFormat(dc, &pfd);
            if format == 0 || SetPixelFormat(dc, format, &pfd) == 0 {
                return Err(format!(
                    "no OpenGL pixel format for the video surface: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the render thread has stopped (the owner joins it first); the window, and with
        // it every call into `paint`, is gone before `paint` is freed.
        unsafe {
            DestroyWindow(self.hwnd as HWND);
            drop(Box::from_raw(self.paint as *mut PaintFn));
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            // SAFETY: hwnd is this window; its user data is null until `create` stores the
            // `PaintFn`, which lives until after the window is destroyed. Validating stops
            // Windows from sending WM_PAINT in a loop; the render thread does the drawing.
            unsafe {
                ValidateRect(hwnd, std::ptr::null());
                let paint = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const PaintFn;
                if let Some(f) = paint.as_ref() {
                    f();
                }
            }
            0
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
