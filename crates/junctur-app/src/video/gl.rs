//! A WGL (OpenGL) context on the video surface, for mpv's render API, which on Windows offers
//! only OpenGL or software rendering (no Direct3D).
//!
//! The legacy `wglCreateContext` is enough: on NVIDIA it returns a 4.6 compatibility context,
//! which mpv's GL renderer accepts, and it needs no extension loading first.

use std::ffi::{c_char, c_void};

use windows_sys::Win32::Foundation::{HMODULE, HWND};
use windows_sys::Win32::Graphics::Gdi::{GetDC, HDC};
use windows_sys::Win32::Graphics::OpenGL::{
    wglCreateContext, wglDeleteContext, wglGetProcAddress, wglMakeCurrent, SwapBuffers, HGLRC,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

pub struct Gl {
    dc: HDC,
    ctx: HGLRC,
    opengl32: HMODULE,
}

impl Gl {
    /// Creates a context on `hwnd` and makes it current on the calling thread, which keeps it.
    pub fn new(hwnd: isize) -> Result<Self, String> {
        // SAFETY: the window has its pixel format set (Surface::create) and a private DC.
        unsafe {
            let dc = GetDC(hwnd as HWND);
            let ctx = wglCreateContext(dc);
            if ctx.is_null() {
                return Err(format!(
                    "wglCreateContext failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if wglMakeCurrent(dc, ctx) == 0 {
                wglDeleteContext(ctx);
                return Err(format!(
                    "wglMakeCurrent failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let opengl32 = GetModuleHandleA(c"opengl32.dll".as_ptr() as *const u8);
            let gl = Self { dc, ctx, opengl32 };
            gl.set_vsync();
            Ok(gl)
        }
    }

    /// Present at the display's rate rather than as fast as possible; mpv times frames from the
    /// swaps it is told about.
    fn set_vsync(&self) {
        type SwapInterval = unsafe extern "system" fn(i32) -> i32;
        // SAFETY: a current context; the pointer, when present, has this signature.
        unsafe {
            let p = get_proc_address(self.opengl32, c"wglSwapIntervalEXT".as_ptr());
            if !p.is_null() {
                let f: SwapInterval = std::mem::transmute(p);
                f(1);
            }
        }
    }

    /// The value mpv passes back to [`get_proc_address`].
    pub fn proc_ctx(&self) -> *mut c_void {
        self.opengl32
    }

    pub fn swap(&self) {
        // SAFETY: this thread's current DC.
        unsafe {
            SwapBuffers(self.dc);
        }
    }
}

impl Drop for Gl {
    fn drop(&mut self) {
        // SAFETY: the context is current on this thread and nothing uses it after this.
        unsafe {
            wglMakeCurrent(std::ptr::null_mut(), std::ptr::null_mut());
            wglDeleteContext(self.ctx);
        }
    }
}

/// mpv's GL function loader. `wglGetProcAddress` returns only extension and post-1.1 functions
/// (and 0, 1, 2, 3 or -1 for the rest), so GL 1.1 core functions come from opengl32.dll.
///
/// # Safety
/// `ctx` is opengl32's module handle and `name` a NUL-terminated string.
pub unsafe extern "C" fn get_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: per this function's contract.
    unsafe {
        if let Some(f) = wglGetProcAddress(name as *const u8) {
            let p = f as usize;
            if !matches!(p, 1 | 2 | 3 | usize::MAX) {
                return p as *mut c_void;
            }
        }
        GetProcAddress(ctx as HMODULE, name as *const u8)
            .map_or(std::ptr::null_mut(), |f| f as *mut c_void)
    }
}
