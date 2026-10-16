//! Moments and the speaker roll call (phase 7a, a prototype): the moments of the open recording and
//! the user's answers ([`crate::moments`]), and the roll call's cards and answers
//! ([`crate::rollcall`]).

use std::path::Path;

use tauri::{AppHandle, Manager, State};

use super::{blocking, App};
use crate::moments::{self, Action, MomentsView};
use crate::rollcall::{self, RollCallView};

/// What the Moments panel and lane show of `path`.
#[tauri::command]
pub async fn moments_view(path: String) -> Result<MomentsView, String> {
    blocking(move || moments::view(Path::new(&path), None)).await
}

/// Accepts, rejects or undoes the moment marked at `at` (or accepts every waiting one) on the open
/// recording; accepting edits the list as one undo step.
#[tauri::command]
pub async fn moment_answer(
    handle: AppHandle,
    action: Action,
    at: Option<f64>,
) -> Result<(), String> {
    blocking(move || {
        let app = handle.state::<App>();
        let e = app.editable()?;
        moments::answer(&e, e.file(), None, action, at)
    })
    .await
}

/// The roll call of `path`: a card per speaker and the stretches to ask about.
#[tauri::command]
pub async fn roll_call(path: String) -> Result<RollCallView, String> {
    blocking(move || rollcall::view(Path::new(&path), None)).await
}

/// Marks the roll call of `path` shown, so it does not open by itself again.
#[tauri::command]
pub async fn roll_call_seen(path: String) -> Result<(), String> {
    blocking(move || rollcall::seen(Path::new(&path), None)).await
}

/// Gives source seconds `start..end` of the open recording to speaker `speaker`, or to a new
/// speaker called `name`.
#[tauri::command]
pub async fn roll_call_assign(
    app: State<'_, App>,
    start: f64,
    end: f64,
    speaker: Option<u32>,
    name: Option<String>,
) -> Result<(), String> {
    let e = app.editable()?;
    blocking(move || e.assign_stretch(e.file(), (start, end), speaker, name.as_deref())).await
}

/// Remembers `name`'s voice from source seconds `stretches` (a card's `voice`, read before any
/// enrolment renumbered the speakers); returns a line for the user.
#[tauri::command]
pub async fn roll_call_remember(
    app: State<'_, App>,
    name: String,
    stretches: Vec<(f64, f64)>,
) -> Result<String, String> {
    let e = app.editable()?;
    blocking(move || e.enroll_stretches(e.file(), &stretches, &name)).await
}
