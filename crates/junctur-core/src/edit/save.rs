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

/// FNV-1a: stable across builds and platforms, unlike std's hasher. It only tells an untouched
/// save from an edited one; it protects nothing.
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
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
    let name = edit_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match n {
        1 => edit_path.with_file_name(format!("{name}.bak")),
        n => edit_path.with_file_name(format!("{name}.bak{n}")),
    }
}

/// The first free backup name beside `edit_path`: `<name>.bak`, then `<name>.bak2`, ... Backups are
/// never overwritten: a single `.bak` rewritten each time would lose hand edits.
pub fn backup_path(edit_path: &Path) -> PathBuf {
    (1..)
        .map(|n| backup_name(edit_path, n))
        .find(|p| !p.exists())
        .expect("some name is free")
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
    let tmp = path.with_extension("junctur.tmp");
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
mod tests {
    use super::super::{CutSettings, Range};
    use super::*;

    fn list(end: f64) -> EditList {
        EditList {
            source: "in.mp4".into(),
            duration: 10.0,
            settings: CutSettings::default(),
            keep: vec![Range { start: 0.0, end }],
            speakers: Default::default(),
            captions: Default::default(),
            fixes: Vec::new(),
            times: Vec::new(),
            windows: vec![],
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("junctur-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backs_up_only_what_someone_else_wrote() {
        let dir = scratch("bak");
        let path = dir.join("edit.junctur");
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();

        // Nothing there yet: nothing to keep.
        save(&path, &list(1.0)).unwrap();
        assert_eq!(read("edit.junctur.bak"), None);
        // An untouched save (this session's or an earlier one's) is replaced without a backup.
        let second = save(&path, &list(2.0)).unwrap();
        assert_eq!(second.backup, None);
        assert_eq!(read("edit.junctur").as_deref(), Some(second.text.as_str()));
        assert_eq!(super::super::parse(&second.text).unwrap(), list(2.0));
        // A hand edit is kept (its stamp no longer matches), the next one goes to .bak2, and one
        // that does not parse is said so.
        let hand = second.text.replace("keep 0.000 2.000", "keep 0.000 2.500");
        std::fs::write(&path, &hand).unwrap();
        let third = save(&path, &list(3.0)).unwrap();
        assert_eq!(read("edit.junctur.bak").as_deref(), Some(hand.as_str()));
        assert!(!third.replaced_unreadable);
        std::fs::write(&path, "hand edit 2").unwrap();
        let fourth = save(&path, &list(4.0)).unwrap();
        assert_eq!(read("edit.junctur.bak2").as_deref(), Some("hand edit 2"));
        assert!(fourth.replaced_unreadable);
        assert_eq!(read("edit.junctur.tmp"), None, "renamed away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn amend_rewrites_only_what_loses_nothing() {
        use super::super::Window;
        let dir = scratch("amend");
        let path = dir.join("edit.junctur");
        let counted = |e: &mut EditList| {
            e.windows = vec![Window {
                start: 0.0,
                score: 1.0,
                speech: 1.0,
                words: 7,
            }]
        };
        assert_eq!(amend(&path, counted).unwrap(), Amended::Missing);
        // As the pipeline writes it: written plain, nothing backed up.
        std::fs::write(&path, list(5.0).to_text()).unwrap();
        let Amended::Written(e) = amend(&path, counted).unwrap() else {
            panic!("not written")
        };
        assert_eq!(e.windows[0].words, 7);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!is_untouched_save(&text) && text == e.to_text());
        assert_eq!(amend(&path, counted).unwrap(), Amended::Same(e));
        // An editor save (the user cut while it transcribed; its first save keeps the pipeline's
        // list as a backup, as ever): their ranges kept, still an untouched save, so neither the
        // amend nor the editor's next save backs anything up.
        save(&path, &list(2.0)).unwrap();
        std::fs::remove_file(dir.join("edit.junctur.bak")).unwrap();
        let Amended::Written(e) = amend(&path, counted).unwrap() else {
            panic!("not written")
        };
        assert_eq!((e.keep[0].end, e.windows.len()), (2.0, 1));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(is_untouched_save(&text));
        assert_eq!(save(&path, &list(1.5)).unwrap().backup, None);
        assert!(!dir.join("edit.junctur.bak").exists());
        // A hand edit with a comment, or one that does not parse: left alone.
        let hand = format!("# mine\n{}", list(3.0).to_text());
        std::fs::write(&path, &hand).unwrap();
        assert_eq!(amend(&path, counted).unwrap(), Amended::Left);
        std::fs::write(&path, "keep x").unwrap();
        assert_eq!(amend(&path, counted).unwrap(), Amended::Left);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep x");
        assert!(!dir.join("edit.junctur.bak").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_the_parser_would_refuse_is_never_written() {
        // A kept-as-heard span longer than a span's text may be would fail the next open.
        use super::super::Fix;
        let dir = scratch("refused");
        let path = dir.join("edit.junctur");
        let mut bad = list(5.0);
        bad.fixes = vec![Fix {
            track: 0,
            start: 1.0,
            nth: 0,
            through: Some(super::super::WordAt { start: 2.0, nth: 0 }),
            text: "x".repeat(super::super::MAX_SPAN + 1),
        }];
        assert!(bad.checked_text().is_err());
        let e = save(&path, &bad).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(!path.exists(), "nothing written");
        // As the pipeline writes it, and as an editor save: amend refuses too, the file as it was.
        for first in [list(5.0).to_text(), save(&path, &list(5.0)).unwrap().text] {
            std::fs::write(&path, &first).unwrap();
            let e = amend(&path, |e| e.fixes = bad.fixes.clone()).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_retried_save_does_not_pile_up_backups() {
        let dir = scratch("retry");
        let path = dir.join("edit.junctur");
        std::fs::write(&path, "hand").unwrap();
        // An earlier attempt backed it up, then failed to rename: the list is still the hand edit.
        std::fs::write(dir.join("edit.junctur.bak"), "hand").unwrap();
        let s = save(&path, &list(1.0)).unwrap();
        assert_eq!(s.backup, None);
        assert!(!dir.join("edit.junctur.bak2").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
