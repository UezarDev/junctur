//! A safe handle over one mpv instance. mpv's client API is thread-safe, so one `Mpv` is shared
//! (in an `Arc`) by the command handlers, the event thread and the render thread.

use std::cell::Cell;
use std::ffi::{c_char, c_void, CString};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
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
    api: Arc<ffi::Api>,
    handle: *mut ffi::Handle,
    /// Whether the one [`Events`] reader has been handed out.
    events_taken: AtomicBool,
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
        let mpv = Self {
            api,
            handle,
            events_taken: AtomicBool::new(false),
        };
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

    /// A property as text (a map or list comes as JSON); `None` when mpv has no value for it.
    pub fn get_string(&self, name: &str) -> Option<String> {
        let n = cstr(name).ok()?;
        // SAFETY: live handle, NUL-terminated name; a non-null result is mpv's string, copied out
        // and then freed with mpv_free as client.h requires.
        unsafe {
            let p = (self.api.get_property_string)(self.handle, n.as_ptr());
            if p.is_null() {
                return None;
            }
            let s = event::text(p);
            (self.api.free)(p.cast());
            Some(s)
        }
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

    /// The library and raw handle, for the render context (the other half of the mpv API).
    pub(super) fn raw(&self) -> (&ffi::Api, *mut ffi::Handle) {
        (&self.api, self.handle)
    }

    /// The event reader. mpv allows one thread at a time in `mpv_wait_event`, so there is only
    /// ever one, and it cannot be shared between threads (it is `Send`, not `Sync`).
    pub fn events(self: &Arc<Self>) -> Option<Events> {
        (!self.events_taken.swap(true, Ordering::SeqCst)).then(|| Events {
            mpv: self.clone(),
            _not_sync: PhantomData,
        })
    }
}

pub struct Events {
    mpv: Arc<Mpv>,
    _not_sync: PhantomData<Cell<()>>,
}

impl Events {
    /// Blocks up to `timeout` seconds (negative: forever) for the next event.
    pub fn wait(&mut self, timeout: f64) -> Option<Event> {
        let (api, handle) = self.mpv.raw();
        // SAFETY: this is the only reader (`Mpv::events`), and `&mut self` keeps it on one thread
        // at a time; mpv's pointer is valid until the next call, and decode copies out of it.
        unsafe {
            let e = (api.wait_event)(handle, timeout);
            e.as_ref().and_then(|e| event::decode(api, e))
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
