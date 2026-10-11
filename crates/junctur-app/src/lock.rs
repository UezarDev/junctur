//! Locking that survives a poisoned mutex. Every lock in the app guards plain state (player
//! state, wake flags) that stays valid if a thread panicked mid-update, and the app should keep
//! playing rather than cascade the panic into the UI thread.

use std::sync::{Mutex, MutexGuard};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
