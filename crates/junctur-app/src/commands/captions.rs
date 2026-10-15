//! Captions: settings, the SRT export, the preview's cues, Send to Resolve and translation.

use std::path::Path;

use junctur_core::edit::{Captions, ExistingClips};
use junctur_core::export::layout::Placed;
use junctur_core::resolve::{Answer, Outcome};
use serde::Serialize;
use tauri::{AppHandle, State};

use super::{blocking, App};
use crate::translation::Run;

/// Sets the caption settings: switches, styling, how cues are cut, the Text+ look, text options
/// that change only what captions show, and `translate` ([`translate_captions`] makes it).
#[tauri::command]
pub async fn set_captions(app: State<'_, App>, captions: Captions) -> Result<(), String> {
    let e = app.editable()?;
    e.set_captions(e.file(), captions)
}

/// What an export wrote: the file's captions.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub cues: usize,
    /// Per-speaker files beside it from an older export.
    pub stale: Vec<String>,
}

/// Writes the captions as one SRT to `path`, people talking at once stacked in one caption.
#[tauri::command]
pub async fn export_captions(app: State<'_, App>, path: String) -> Result<Exported, String> {
    let core = app.core()?;
    let f = blocking(move || core.export_captions(Path::new(&path))).await?;
    Ok(Exported {
        cues: f.cues,
        stale: f.stale.iter().map(|p| p.display().to_string()).collect(),
    })
}

/// The captions as the export cuts them, for the preview: output seconds, placed where the Resolve
/// send puts them, `aspect` the picture's height over its width.
#[tauri::command]
pub async fn caption_cues(app: State<'_, App>, aspect: f64) -> Result<Vec<Placed>, String> {
    let core = app.core()?;
    blocking(move || core.caption_cues(aspect)).await
}

/// Sends the edit into DaVinci Resolve as a timeline with Text+ captions, through the Junctur
/// script there. `answer` decides on a timeline of that name already there (none: answers
/// `exists`), `existing` on clips Junctur did not place in the captions' way. Takes minutes.
#[tauri::command]
pub async fn send_to_resolve(
    app: State<'_, App>,
    answer: Option<Answer>,
    existing: Option<ExistingClips>,
) -> Result<Outcome, String> {
    let core = app.core()?;
    blocking(move || core.send_to_resolve(answer, existing)).await
}

/// Translates the open recording's captions in the background, into the language its caption
/// settings name: the sentences no translation is kept for yet. Progress comes as `translation`
/// events. Gathering the sentences reads the whole transcript, so off the async runtime.
#[tauri::command]
pub async fn translate_captions(app: State<'_, App>, handle: AppHandle) -> Result<Run, String> {
    let core = app.core()?;
    let job = blocking(move || core.translation_job()).await?;
    Ok(app.translation.start(handle, job))
}

/// Stops the caption translation after the sentences in hand; those done are kept.
#[tauri::command]
pub async fn cancel_translation(app: State<'_, App>) -> Result<(), String> {
    app.translation.cancel();
    Ok(())
}

/// The latest translation run, and how much of the open recording's captions is left to
/// translate.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationStatus {
    pub run: Run,
    pub missing: usize,
    pub sentences: usize,
}

#[tauri::command]
pub async fn translation_status(app: State<'_, App>) -> Result<TranslationStatus, String> {
    let core = app.core()?;
    let (missing, sentences) = blocking(move || core.untranslated()).await?;
    Ok(TranslationStatus {
        run: app.translation.run(),
        missing,
        sentences,
    })
}

#[derive(Serialize)]
pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
}

/// The languages captions can be translated into, by English name.
#[tauri::command]
pub async fn translation_languages() -> Vec<Language> {
    let mut l: Vec<Language> = junctur_core::translate::LANGUAGES
        .iter()
        .map(|&(code, name)| Language { code, name })
        .collect();
    l.sort_by_key(|l| l.name);
    l
}

/// The Fusion Titles in the open Resolve project's media pool, for the template picker,
/// through the Junctur script running there. Off the async runtime: a script not listening takes
/// seconds to give up on, a busy Resolve longer.
#[tauri::command]
pub async fn resolve_templates() -> Result<Vec<String>, String> {
    blocking(junctur_core::resolve::templates).await
}
