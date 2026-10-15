//! Word fixes, flagged words and Correct (its pop-up's re-analysis: the words, the speakers and the
//! cut redone apart, the user's fixes, names and cut kept or dropped, over a selection or all).

use std::path::Path;

use junctur_core::edit::Fix;
use junctur_core::pipeline::{self, Project, SetAside};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use super::{blocking, notifier, App};
use crate::analysis::{Again, Forget, Progress, Reanalysis, Redo};
use crate::player::{FixDone, WordRef};
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
/// heard it (or only changed its case). `word` names the word as the page had it when its field
/// opened: refused when `index` holds another word now (5ae).
#[tauri::command]
pub async fn fix_word(
    app: State<'_, App>,
    index: usize,
    word: WordRef,
    text: String,
) -> Result<Fixed, String> {
    let e = app.editable()?;
    let done = e.fix_word(e.file(), index, word, &text)?;
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
/// list. One word is learned as [`fix_word`] learns it. `ends`: the first and last words as the
/// page had them when the field opened, as for [`fix_word`] (5ae).
#[tauri::command]
pub async fn fix_words(
    app: State<'_, App>,
    first: usize,
    last: usize,
    ends: [WordRef; 2],
    text: String,
) -> Result<Fixed, String> {
    let e = app.editable()?;
    let done = e.fix_words(e.file(), first, last, ends, &text)?;
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

/// A fix of the open recording's list where its transcript has it (5ae: the list of fixes).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedFix {
    pub fix: Fix,
    /// Its first and last words' indices in the transcript; none when it is on no word now.
    pub first: Option<usize>,
    pub last: Option<usize>,
    /// Its words as heard.
    pub heard: String,
    /// A fix of the heard text (Undo those, a removal, Looks right): the words read as heard.
    pub as_heard: bool,
}

/// Every fix of the open recording's edit list with its words (5ae), in the list's order.
#[tauri::command]
pub async fn fixes_list(app: State<'_, App>) -> Result<Vec<ListedFix>, String> {
    Ok(app
        .core()?
        .fixes_list()?
        .into_iter()
        .map(|p| ListedFix {
            as_heard: p.as_heard(),
            first: p.words.map(|w| w.0),
            last: p.words.map(|w| w.1),
            heard: p.heard,
            fix: p.fix,
        })
        .collect())
}

/// Takes the fixes of `fixes` (as [`fixes_list`] gave them) back to what their words were heard
/// as, those still as listed (5ae: the list's Remove); how many.
#[tauri::command]
pub async fn remove_fixes(app: State<'_, App>, fixes: Vec<Fix>) -> Result<usize, String> {
    let e = app.editable()?;
    e.remove_fixes(e.file(), &fixes)
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

/// What the Correct pop-up asks for, as the page sends it: what to hear again, what of the user's
/// to keep, and the selection.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// Transcribe the words again.
    pub words: bool,
    /// Hear the speakers again.
    pub speakers: bool,
    /// Keep the cut; unticked, a fresh one (asks first over the user's edits).
    pub keep_cuts: bool,
    /// Keep the speakers' names and the seconds pinned to people; unticked, named afresh.
    pub keep_names: bool,
    /// Keep the fixed words and word times, carried onto new words; unticked, dropped.
    pub keep_fixes: bool,
    /// The selection in source seconds; `None` for the whole timeline.
    pub within: Option<[f64; 2]>,
    /// Re-cut even over the user's edits: the page asked.
    pub replace: bool,
}

impl Ask {
    pub fn reanalysis(&self) -> Reanalysis {
        Reanalysis {
            again: match (self.keep_cuts, self.replace) {
                (true, _) => Again::KeepList,
                (false, false) => Again::Recut,
                (false, true) => Again::RecutOverEdits,
            },
            redo: Redo {
                words: self.words,
                speakers: self.speakers,
            },
            forget: Forget {
                names: !self.keep_names,
                fixes: !self.keep_fixes,
            },
            within: self.within.map(|[a, b]| (a, b)),
        }
    }
}

/// What the Correct pop-up's run did.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Corrected {
    /// The run, when one started.
    pub progress: Option<Progress>,
    /// The cut was to be made afresh and nothing started, because the edit list holds the user's
    /// edits: the page asks first.
    pub hand_edited: bool,
    /// The fixes its first step made on words heard like a fixed one, and what those were heard as
    /// (the note's Undo, as for a fix).
    pub others: Vec<Fix>,
    pub heard: Vec<String>,
    /// Heard texts fixed two ways, left as heard.
    pub unclear: Vec<String>,
}

/// The Correct pop-up: analyses `path` again as `ask` says ([`Analysis::reanalyse`]). When it
/// transcribes the whole timeline again keeping the fixes, every fix is first spread to the words
/// heard like it (Correct's first step); never for a selection, whose words outside it stay as they
/// are, nor with the fixes dropped. Refused while analysed. Off the async runtime: asking whether
/// the list holds the user's edits reads the track analysis and the transcript.
///
/// [`Analysis::reanalyse`]: crate::analysis::Analysis::reanalyse
#[tauri::command]
pub async fn reanalyse(
    handle: AppHandle,
    app: State<'_, App>,
    path: String,
    ask: Ask,
) -> Result<Corrected, String> {
    let r = ask.reanalysis();
    let spread = r.redo.words && !r.forget.fixes && r.within.is_none();
    // Held from before the run starts: started, it waits for the mark before it reads anything,
    // so the spread below is in the list it keeps.
    let e = app.editable()?;
    let analysis = app.analysis.clone();
    blocking(move || {
        let started = analysis.reanalyse(Path::new(&path), r, notifier(handle))?;
        let mut done = Corrected {
            progress: started.progress,
            hand_edited: started.hand_edited,
            others: Vec::new(),
            heard: Vec::new(),
            unclear: Vec::new(),
        };
        let open = e.file().is_some_and(|f| Path::new(f) == Path::new(&path));
        if spread && open && done.progress.is_some() {
            let s = e.spread_fixes(e.file()).unwrap_or_else(|err| {
                eprintln!("junctur-app: Correct's spread: {err}");
                Default::default()
            });
            (done.others, done.heard, done.unclear) = (s.added, s.heard, s.unclear);
        }
        Ok(done)
    })
    .await
}
