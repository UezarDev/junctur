//! A safe handle over one mpv instance. mpv's client API is thread-safe, so one `Mpv` is shared
//! (in an `Arc`) by the command handlers, the event thread and the render thread.

use std::ffi::{c_char, c_void, CString};
use std::sync::Arc;

use super::event::{self, Event};
use super::ffi;

/// The value format a property is observed in.
#[derive(Debug, Clone, Copy)]
pub enum Format {
    Str,
    Flag,
    Double,
}

impl Format {
    fn raw(self) -> i32 {
        match self {
            Format::Str => ffi::FORMAT_STRING,
            Format::Flag => ffi::FORMAT_FLAG,
            Format::Double => ffi::FORMAT_DOUBLE,
        }
    }
}

pub struct Mpv {
    pub(super) api: Arc<ffi::Api>,
    pub(super) handle: *mut ffi::Handle,
}

// SAFETY: every mpv_* client function may be called from any thread (client.h, "Thread safety").
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

fn cstr(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("NUL byte in {s:?}"))
}

impl Mpv {
    /// Creates and initialises mpv with `options` (set before init, as `--name=value` would be).
    pub fn new(api: Arc<ffi::Api>, options: &[(&str, &str)]) -> Result<Self, String> {
        // SAFETY: mpv_create has no preconditions; a null return is handled.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }
        let mpv = Self { api, handle };
        for (k, v) in options {
            let (kc, vc) = (cstr(k)?, cstr(v)?);
            // SAFETY: live handle, NUL-terminated strings.
            let r = unsafe { (mpv.api.set_option_string)(mpv.handle, kc.as_ptr(), vc.as_ptr()) };
            mpv.check(r, &format!("option {k}={v}"))?;
        }
        // SAFETY: live handle, not yet initialised.
        let r = unsafe { (mpv.api.initialize)(mpv.handle) };
        mpv.check(r, "mpv_initialize")?;
        Ok(mpv)
    }

    pub(super) fn check(&self, r: i32, what: &str) -> Result<(), String> {
        if r >= 0 {
            return Ok(());
        }
        // SAFETY: mpv_error_string returns a static string for any code.
        let msg = unsafe { event::text((self.api.error_string)(r)) };
        Err(format!("{what}: {msg}"))
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned = args
            .iter()
            .map(|a| cstr(a))
            .collect::<Result<Vec<_>, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: a NULL-terminated array of strings that outlive the call.
        let r = unsafe { (self.api.command)(self.handle, ptrs.as_mut_ptr()) };
        self.check(r, &args.join(" "))
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name)?, cstr(value)?);
        let mut p = v.as_ptr();
        // SAFETY: MPV_FORMAT_STRING takes a pointer to a `char *`.
        let r = unsafe {
            (self.api.set_property)(
                self.handle,
                n.as_ptr(),
                ffi::FORMAT_STRING,
                &mut p as *mut _ as *mut c_void,
            )
        };
        self.check(r, name)
    }

    pub fn observe(&self, id: u64, name: &str, format: Format) -> Result<(), String> {
        let n = cstr(name)?;
        // SAFETY: live handle, NUL-terminated name.
        let r = unsafe { (self.api.observe_property)(self.handle, id, n.as_ptr(), format.raw()) };
        self.check(r, name)
    }

    /// Log lines at `level` and above arrive as [`Event::Log`].
    pub fn request_log(&self, level: &str) -> Result<(), String> {
        let l = cstr(level)?;
        // SAFETY: live handle, NUL-terminated level.
        let r = unsafe { (self.api.request_log_messages)(self.handle, l.as_ptr()) };
        self.check(r, "log level")
    }

    /// Blocks up to `timeout` seconds (negative: forever) for the next event. Only one thread may
    /// wait at a time, so the player's event thread is the only caller.
    pub fn wait_event(&self, timeout: f64) -> Option<Event> {
        // SAFETY: mpv returns a pointer valid until the next call, which decode copies out of.
        unsafe {
            let e = (self.api.wait_event)(self.handle, timeout);
            e.as_ref().and_then(|e| event::decode(&self.api, e))
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: the last reference is gone; the render context holds an `Arc<Mpv>`, so it was
        // freed before this, as render.h requires.
        unsafe { (self.api.terminate_destroy)(self.handle) }
    }
}
