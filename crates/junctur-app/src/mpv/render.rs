//! mpv's render API over OpenGL: mpv draws each frame into whatever GL framebuffer the caller has
//! current. The GL context and the window it draws on belong to `crate::video`; this file only
//! knows mpv's side, so another platform's surface can reuse it.

use std::ffi::{c_void, CString};
use std::sync::Arc;

use super::ffi;
use super::handle::Mpv;

type UpdateFn = Box<dyn Fn() + Send + Sync>;

pub struct RenderContext {
    mpv: Arc<Mpv>,
    ctx: *mut ffi::RenderContext,
    // Boxed twice so the pointer handed to mpv stays put; dropped only after the callback is unset.
    update: Option<Box<UpdateFn>>,
}

impl RenderContext {
    /// Must be called with the GL context current on this thread, and before any file is loaded
    /// (with `vo=libmpv`, video output needs a render context to exist).
    pub fn opengl(
        mpv: Arc<Mpv>,
        get_proc_address: ffi::GetProcAddress,
        proc_ctx: *mut c_void,
    ) -> Result<Self, String> {
        let api_type = CString::new("opengl").expect("no NUL");
        let mut init = ffi::OpenGlInitParams {
            get_proc_address,
            get_proc_address_ctx: proc_ctx,
        };
        let mut params = [
            param(ffi::RENDER_PARAM_API_TYPE, api_type.as_ptr() as *mut c_void),
            param(
                ffi::RENDER_PARAM_OPENGL_INIT_PARAMS,
                &mut init as *mut _ as _,
            ),
            param(ffi::RENDER_PARAM_INVALID, std::ptr::null_mut()),
        ];
        let mut ctx = std::ptr::null_mut();
        // SAFETY: params are terminated by INVALID and live through the call.
        let r =
            unsafe { (mpv.api.render_context_create)(&mut ctx, mpv.handle, params.as_mut_ptr()) };
        mpv.check(r, "mpv render context")?;
        Ok(Self {
            mpv,
            ctx,
            update: None,
        })
    }

    /// `f` runs on an mpv thread whenever a new frame (or a redraw) is wanted. It must not call
    /// into mpv; it should only wake the render thread.
    pub fn on_update(&mut self, f: impl Fn() + Send + Sync + 'static) {
        let boxed: Box<UpdateFn> = Box::new(Box::new(f));
        let ptr = &*boxed as *const UpdateFn as *mut c_void;
        // SAFETY: `ptr` stays valid until Drop unsets the callback before freeing it.
        unsafe {
            (self.mpv.api.render_context_set_update_callback)(self.ctx, Some(trampoline), ptr)
        };
        self.update = Some(boxed);
    }

    /// Whether mpv has a new frame to draw. Call after every update callback.
    pub fn needs_frame(&self) -> bool {
        // SAFETY: live context, on the render thread.
        unsafe { (self.mpv.api.render_context_update)(self.ctx) & ffi::RENDER_UPDATE_FRAME != 0 }
    }

    /// Draws the current frame into framebuffer `fbo` (0 = the window) of `w`x`h` pixels.
    pub fn render(&self, fbo: i32, w: i32, h: i32) -> Result<(), String> {
        let mut target = ffi::OpenGlFbo {
            fbo,
            w,
            h,
            internal_format: 0,
        };
        // GL's window framebuffer has its origin bottom-left; mpv draws top-down unless told.
        let mut flip: i32 = 1;
        let mut params = [
            param(ffi::RENDER_PARAM_OPENGL_FBO, &mut target as *mut _ as _),
            param(ffi::RENDER_PARAM_FLIP_Y, &mut flip as *mut _ as _),
            param(ffi::RENDER_PARAM_INVALID, std::ptr::null_mut()),
        ];
        // SAFETY: live context with its GL context current; params terminated and alive.
        let r = unsafe { (self.mpv.api.render_context_render)(self.ctx, params.as_mut_ptr()) };
        self.mpv.check(r, "mpv render")
    }

    /// Tells mpv a frame reached the screen, for its frame timing.
    pub fn report_swap(&self) {
        // SAFETY: live context.
        unsafe { (self.mpv.api.render_context_report_swap)(self.ctx) }
    }
}

fn param(kind: i32, data: *mut c_void) -> ffi::RenderParam {
    ffi::RenderParam { kind, data }
}

unsafe extern "C" fn trampoline(ctx: *mut c_void) {
    // SAFETY: ctx is the `UpdateFn` registered in `on_update`, alive while registered.
    let f = unsafe { &*(ctx as *const UpdateFn) };
    f();
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        // SAFETY: the owner drops this on the render thread with the GL context current; unsetting
        // the callback first means mpv never calls into freed memory.
        unsafe {
            (self.mpv.api.render_context_set_update_callback)(self.ctx, None, std::ptr::null_mut());
            (self.mpv.api.render_context_free)(self.ctx);
        }
        self.update = None;
    }
}
