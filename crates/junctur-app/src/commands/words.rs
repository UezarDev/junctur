//! Word fixes, flagged words and Correct (transcribing again with the fixes carried over).

use std::path::Path;

use junctur_core::edit::Fix;
use junctur_core::pipeline::{self, Project, SetAside};
use serde::Serialize;
use tauri::{AppHandle, State};

use super::{notifier, App};
use crate::analysis::Progress;
use crate::player::FixDone;
use crate::settings;

/// What a fix did besides itself, for the page: the entries the word list gained, and the fixes
/// also made on the words heard like it with what those were heard as (for [`keep_heard`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fixed {
    pub learned: Vec<String>,
    pub others: Vec<Fix>,
    pub heard: Vec<String>,
}

/// Saves what `done` teaches to the word list (no prompt, never twice). A word list that cannot be
/// saved does not undo the fix: it is logged.
fn learned(done: FixDone, fixed: &str) -> Fixed {
    let learned = settings::learn(&done.learn).unwrap_or_else(|err| {
        eprintln!("junctur-app: adding {fixed:?}'s words to the word list: {err}");
        Vec::new()
    });
    Fixed {
        learned,
        others: done.others,
        heard: done.heard,
    }
}

/// Fixes the spelling of transcript word `index` (empty, or the heard text again, removes the fix),
/// and every other word heard like it; its spelling joins the word list when the recording never
/// heard it (or only changed its case).
#[tauri::command]
pub async fn fix_word(app: State<'_, App>, index: usize, text: String) -> Result<Fixed, String> {
    let e = app.editable()?;
    let done = e.fix_word(e.file(), index, &text)?;
    Ok(learned(done, &text))
}

/// Marks transcript word `index` as checked: it keeps what Whisper heard and is no longer
/// flagged. Nothing joins the word list.
#[tauri::command]
pub async fn confirm_word(app: State<'_, App>, index: usize) -> Result<(), String> {
    let e = app.editable()?;
    e.confirm_word(e.file(), index)
}

/// Fixes transcript words `first..=last` as one text (empty or the heard words again clears their
/// fixes), and the same words heard elsewhere; its words the recording never heard join the word
/// list. One word is learned as [`fix_word`] learns it.
#[tauri::command]
pub async fn fix_words(
    app: State<'_, App>,
    first: usize,
    last: usize,
    text: String,
) -> Result<Fixed, String> {
    let e = app.editable()?;
    let done = e.fix_words(e.file(), first, last, &text)?;
    Ok(learned(done, &text))
}

/// Takes back fixes a fix made on other words ("Undo those"), where they are still as made:
/// those words keep what they were heard as (`heard`, beside each), and no later spread reaches
/// them. How many were still as made (none once a re-transcription moved them).
#[tauri::command]
pub async fn keep_heard(
    app: State<'_, App>,
    others: Vec<Fix>,
    heard: Vec<String>,
) -> Result<usize, String> {
    let e = app.editable()?;
    e.keep_heard(e.file(), &others, &heard)
}

/// The fixes a re-transcription of the open recording could not put on any word, until
/// [`forget_fixes_set_aside`].
#[tauri::command]
pub async fn fixes_set_aside(app: State<'_, App>) -> Result<Vec<SetAside>, String> {
    let Some(file) = app.core()?.file() else {
        return Ok(Vec::new());
    };
    let p = Project::open(Path::new(&file), None, None).map_err(|e| e.to_string())?;
    Ok(pipeline::fixes_set_aside(&p))
}

/// The user has seen `seen`, fixes set aside for the open recording as [`fixes_set_aside`] listed
/// them: those are forgotten, and any a run set aside since stay.
#[tauri::command]
pub async fn forget_fixes_set_aside(
    app: State<'_, App>,
    seen: Vec<SetAside>,
) -> Result<(), String> {
    let Some(file) = app.core()?.file() else {
        return Ok(());
    };
    let p = Project::open(Path::new(&file), None, None).map_err(|e| e.to_string())?;
    pipeline::forget_fixes_set_aside(&p, &seen).map_err(|e| e.to_string())
}

/// What the Correct button started.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Corrected {
    /// The run hearing the speakers and the words again.
    pub progress: Progress,
    /// The fixes its first step made on words heard like a fixed one, and what those were heard as
    /// (the note's Undo, as for a fix).
    pub others: Vec<Fix>,
    pub heard: Vec<String>,
    /// Heard texts fixed two ways, left as heard.
    pub unclear: Vec<String>,
}

/// The Correct button: every fix spread to the words heard like it, then a run hearing the speakers
/// and the words again, keeping the user's edit list (`Analysis::correct`). Refused while analysed.
#[tauri::command]
pub async fn correct(handle: AppHandle, app: State<'_, App>) -> Result<Corrected, String> {
    let e = app.editable()?;
    let path = e.file().ok_or("no recording is open")?.to_string();
    // The run first, while this edit's mark is held: refused, nothing has changed; started, it
    // waits for the mark before it reads anything, so the spread below is in the list it keeps.
    let progress = app.analysis.correct(Path::new(&path), notifier(handle))?;
    let s = e.spread_fixes(e.file()).unwrap_or_else(|err| {
        eprintln!("junctur-app: Correct's spread: {err}");
        Default::default()
    });
    drop(e);
    Ok(Corrected {
        progress,
        others: s.added,
        heard: s.heard,
        unclear: s.unclear,
    })
}
