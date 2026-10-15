//! Saving an edit list the disk may hold a hand edit of. A save ends with a stamp comment, a hash of
//! the text above it: a file whose stamp still matches is replaced without a backup, anything else
//! is backed up first. A comment rather than a file beside it, so a moved list stays self-describing.

use std::path::{Path, PathBuf};

use super::EditList;

const STAMP: &str = "# saved by the Junctur app, unchanged since if this matches: ";

/// What a save did.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    /// The text written, stamp included.
    pub text: String,
    /// Where the replaced file was kept, when it was.
    pub backup: Option<PathBuf>,
    /// The replaced file did not parse as an edit list (it was backed up, or already had been).
    pub replaced_unreadable: bool,
}

/// FNV-1a of the text: it only tells an untouched save from an edited one; it protects nothing.
fn hash(s: &str) -> u64 {
    crate::hash::fnv(s.as_bytes())
}

fn stamped(text: String) -> String {
    let h = hash(&text);
    format!("{text}{STAMP}{h:016x}\n")
}

/// `text` is an editor save whose stamp still matches what is above it.
pub fn is_untouched_save(text: &str) -> bool {
    let Some(at) = text.rfind(STAMP) else {
        return false;
    };
    let (body, line) = text.split_at(at);
    u64::from_str_radix(line[STAMP.len()..].trim(), 16).is_ok_and(|h| h == hash(body))
}

/// `text` without its stamp line when it is an untouched editor save; else `text` as it is.
pub fn unstamped(text: &str) -> &str {
    if is_untouched_save(text) {
        text.rfind(STAMP).map_or(text, |at| &text[..at])
    } else {
        text
    }
}

fn backup_name(edit_path: &Path, n: u32) -> PathBuf {
    crate::output::numbered(edit_path, "bak", n)
}

/// The first free backup name beside `edit_path`: `<name>.bak`, then `<name>.bak2`, ... Backups are
/// never overwritten: a single `.bak` rewritten each time would lose hand edits.
pub fn backup_path(edit_path: &Path) -> PathBuf {
    crate::output::first_free(edit_path, "bak")
}

/// The newest backup's text, if any.
fn latest_backup(edit_path: &Path) -> Option<String> {
    let last = (1..)
        .take_while(|&n| backup_name(edit_path, n).exists())
        .last()?;
    std::fs::read_to_string(backup_name(edit_path, last)).ok()
}

/// Writes `list` to `path`, first copying the file there to [`backup_path`] unless it is an untouched
/// editor save, the same text, or the newest backup. Written through a temporary file renamed over
/// the list, so a reader or a crash never sees a half-written list. A list the parser would not read
/// back is refused, nothing written.
pub fn save(path: &Path, list: &EditList) -> std::io::Result<Saved> {
    let _one = hold();
    save_now(path, list)
}

/// One writer of an edit list at a time in this process: the editor's saves and [`amend`]'s word
/// counts would otherwise write over each other. Other processes are not guarded.
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    /// This thread holds [`WRITING`] (through a [`Held`]).
    static HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`WRITING`], held until dropped; on a thread that holds it already, a no-op, so [`save`] and
/// [`amend`] work inside a [`hold`].
pub struct Held(Option<std::sync::MutexGuard<'static, ()>>);

impl Drop for Held {
    fn drop(&mut self) {
        if self.0.is_some() {
            HELD.with(|h| h.set(false));
        }
    }
}

/// Holds the edit-list writer lock from reading a list to saving over it: the app's
/// editor takes an outside change on disk and then saves the list it made from it, and word counts
/// [`amend`] wrote between the two would be saved over and lost.
pub fn hold() -> Held {
    if HELD.with(|h| h.get()) {
        return Held(None);
    }
    let g = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    HELD.with(|h| h.set(true));
    Held(Some(g))
}

/// What [`amend`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Amended {
    /// Rewritten with the change; the list now on disk.
    Written(EditList),
    /// The change changed nothing; the list on disk.
    Same(EditList),
    /// Not Junctur's form (a hand edit with comments or another layout, or a list that does not
    /// parse): rewriting it could lose what the user wrote, so it is left as it is.
    Left,
    /// No list there.
    Missing,
}

/// Applies `change` to the edit list at `path` and writes it back when that loses nothing: an
/// untouched editor save is saved again stamped, a list in Junctur's own form is written plain, and
/// anything else is left. Under the same lock as [`save`], and refused as [`save`] refuses.
pub fn amend(path: &Path, change: impl FnOnce(&mut EditList)) -> std::io::Result<Amended> {
    let _one = hold();
    let old = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Amended::Missing),
        Err(e) => return Err(e),
    };
    let editor = is_untouched_save(&old);
    let body = unstamped(&old);
    let Ok(list) = super::parse(body) else {
        return Ok(Amended::Left);
    };
    if !editor && list.to_text() != body {
        return Ok(Amended::Left);
    }
    let mut new = list.clone();
    change(&mut new);
    if new == list {
        return Ok(Amended::Same(list));
    }
    if editor {
        save_now(path, &new)?;
    } else {
        crate::output::write_replacing(path, checked(&new)?)?;
    }
    Ok(Amended::Written(new))
}

/// `list`'s text when the parser reads it back ([`EditList::checked_text`]), else an
/// `InvalidData` error: the file is left as it was.
fn checked(list: &EditList) -> std::io::Result<String> {
    list.checked_text()
        .map_err(|why| std::io::Error::new(std::io::ErrorKind::InvalidData, why))
}

fn save_now(path: &Path, list: &EditList) -> std::io::Result<Saved> {
    let text = stamped(checked(list)?);
    let old = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    // Not `write_replacing`: the backup goes between writing the new text and the rename, so a
    // failure to keep it leaves the list as it was.
    let tmp = crate::output::temp_beside(path);
    std::fs::write(&tmp, &text)?;
    let result = replace(path, &tmp, old, &text);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn replace(path: &Path, tmp: &Path, old: Option<String>, text: &str) -> std::io::Result<Saved> {
    let mut backup = None;
    let mut replaced_unreadable = false;
    if let Some(old) = old.filter(|o| o != text && !is_untouched_save(o)) {
        replaced_unreadable = super::parse(&old).is_err();
        if latest_backup(path).as_deref() != Some(old.as_str()) {
            let to = backup_path(path);
            std::fs::write(&to, &old)?;
            backup = Some(to);
        }
    }
    crate::output::rename_over(tmp, path)?;
    Ok(Saved {
        text: text.to_string(),
        backup,
        replaced_unreadable,
    })
}

#[cfg(test)]
mod tests;
