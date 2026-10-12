//! The slice of libmpv's C API the app uses, resolved by name from `libmpv-2.dll` at runtime.
//!
//! Loaded rather than linked so `cargo build/test/clippy` never need the DLL or an import library:
//! a clean checkout builds before `scripts/fetch-libmpv.ps1` has run, and a missing DLL becomes an
//! error the window can show instead of an exe that will not start. Values mirror `client.h`
//! (API 2.x), `render.h` and `render_gl.h`.

use std::ffi::{c_char, c_double, c_int, c_void};
use std::path::Path;

use libloading::Library;

pub enum Handle {}
pub enum RenderContext {}

pub const FORMAT_STRING: c_int = 1;
pub const FORMAT_FLAG: c_int = 3;
pub const FORMAT_DOUBLE: c_int = 5;

pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_LOG_MESSAGE: c_int = 2;
pub const EVENT_START_FILE: c_int = 6;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_PLAYBACK_RESTART: c_int = 21;
pub const EVENT_PROPERTY_CHANGE: c_int = 22;

pub const RENDER_PARAM_INVALID: c_int = 0;
pub const RENDER_PARAM_API_TYPE: c_int = 1;
pub const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const RENDER_PARAM_FLIP_Y: c_int = 4;
pub const RENDER_UPDATE_FRAME: u64 = 1;

#[repr(C)]
pub struct Event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct EventProperty {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct EventStartFile {
    pub playlist_entry_id: i64,
}

#[repr(C)]
pub struct EventEndFile {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[repr(C)]
pub struct EventLogMessage {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

#[repr(C)]
pub struct RenderParam {
    pub kind: c_int,
    pub data: *mut c_void,
}

pub type GetProcAddress =
    unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> *mut c_void;

#[repr(C)]
pub struct OpenGlInitParams {
    pub get_proc_address: GetProcAddress,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct OpenGlFbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

pub type Callback = unsafe extern "C" fn(ctx: *mut c_void);

/// Function pointers copied out of the library, which this struct keeps loaded.
pub struct Api {
    pub client_api_version: unsafe extern "C" fn() -> u64,
    pub error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    pub create: unsafe extern "C" fn() -> *mut Handle,
    pub initialize: unsafe extern "C" fn(*mut Handle) -> c_int,
    pub terminate_destroy: unsafe extern "C" fn(*mut Handle),
    pub set_option_string: unsafe extern "C" fn(*mut Handle, *const c_char, *const c_char) -> c_int,
    pub command: unsafe extern "C" fn(*mut Handle, *mut *const c_char) -> c_int,
    pub set_property: unsafe extern "C" fn(*mut Handle, *const c_char, c_int, *mut c_void) -> c_int,
    pub get_property_string: unsafe extern "C" fn(*mut Handle, *const c_char) -> *mut c_char,
    pub free: unsafe extern "C" fn(*mut c_void),
    pub observe_property: unsafe extern "C" fn(*mut Handle, u64, *const c_char, c_int) -> c_int,
    pub request_log_messages: unsafe extern "C" fn(*mut Handle, *const c_char) -> c_int,
    pub wait_event: unsafe extern "C" fn(*mut Handle, c_double) -> *mut Event,
    pub render_context_create:
        unsafe extern "C" fn(*mut *mut RenderContext, *mut Handle, *mut RenderParam) -> c_int,
    pub render_context_set_update_callback:
        unsafe extern "C" fn(*mut RenderContext, Option<Callback>, *mut c_void),
    pub render_context_update: unsafe extern "C" fn(*mut RenderContext) -> u64,
    pub render_context_render: unsafe extern "C" fn(*mut RenderContext, *mut RenderParam) -> c_int,
    pub render_context_report_swap: unsafe extern "C" fn(*mut RenderContext),
    pub render_context_free: unsafe extern "C" fn(*mut RenderContext),
    _lib: Library,
}

macro_rules! sym {
    ($lib:expr, $name:literal) => {
        *$lib
            .get(concat!($name, "\0").as_bytes())
            .map_err(|e| format!("{}: {e}", $name))?
    };
}

impl Api {
    pub fn load(path: &Path) -> Result<Self, String> {
        // SAFETY: loading libmpv runs its DLL initialisers, which have no preconditions; every
        // symbol below is typed after its declaration in mpv's headers.
        unsafe {
            let lib = Library::new(path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(Self {
                client_api_version: sym!(lib, "mpv_client_api_version"),
                error_string: sym!(lib, "mpv_error_string"),
                create: sym!(lib, "mpv_create"),
                initialize: sym!(lib, "mpv_initialize"),
                terminate_destroy: sym!(lib, "mpv_terminate_destroy"),
                set_option_string: sym!(lib, "mpv_set_option_string"),
                command: sym!(lib, "mpv_command"),
                set_property: sym!(lib, "mpv_set_property"),
                get_property_string: sym!(lib, "mpv_get_property_string"),
                free: sym!(lib, "mpv_free"),
                observe_property: sym!(lib, "mpv_observe_property"),
                request_log_messages: sym!(lib, "mpv_request_log_messages"),
                wait_event: sym!(lib, "mpv_wait_event"),
                render_context_create: sym!(lib, "mpv_render_context_create"),
                render_context_set_update_callback: sym!(
                    lib,
                    "mpv_render_context_set_update_callback"
                ),
                render_context_update: sym!(lib, "mpv_render_context_update"),
                render_context_render: sym!(lib, "mpv_render_context_render"),
                render_context_report_swap: sym!(lib, "mpv_render_context_report_swap"),
                render_context_free: sym!(lib, "mpv_render_context_free"),
                _lib: lib,
            })
        }
    }
}
