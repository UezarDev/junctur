//! Test mode left out: a release build without the `test-hooks` feature. Its steps edit, rename,
//! forget and write files from the environment, which no user's build should do; this stands in
//! with test mode never on.

use std::sync::Arc;

use tauri::AppHandle;

use crate::trace::Trace;

pub const SILENT: &[(&str, &str)] = &[];

pub fn seconds() -> Option<f64> {
    None
}

pub fn place(_window: &tauri::WebviewWindow) -> tauri::Result<()> {
    Ok(())
}

pub fn trace() -> Option<Arc<Trace>> {
    None
}

pub fn arm(_app: AppHandle, _seconds: f64, _trace: Option<Arc<Trace>>) {}

/// The page's log lines are kept only for a test's report.
pub fn ui_log(_message: String) {}
