//! Writing an edit list that something else may have written before: an editor saving on every
//! change, where the file on disk may be a hand edit or another tool's output.
//!
//! A save ends with a stamp comment, a hash of the text above it. A file whose stamp still matches
//! is an editor save nobody has touched since (this session's or an earlier one's), so replacing it
//! loses nothing and needs no backup; anything else is backed up first. A comment rather than a
//! file beside the list, so a list kept in git or moved stays self-describing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::EditList;

const STAMP: &str = "# saved by the Junctur app, unchanged since if this matches: ";

/// A rename can meet a reader (an antivirus scan, the edit-list watch) holding the file on
/// Windows; it is retried this many times, this far apart, before the save fails.
const RENAME_TRIES: u32 = 5;
const RENAME_WAIT: Duration = Duration::from_millis(40);

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
/// never overwritten: a single `.bak` rewritten each time lost hand edits (1d).
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

/// Writes `list` to `path`. The file there is first copied to [`backup_path`] unless it is an
/// untouched editor save, the same text, or the same as the newest backup (so a retried save does
/// not pile up copies). The text goes to a temporary file and is renamed over the list, so a
/// reader (or a crash) never sees a half-written or missing list; a failed save removes the
/// temporary file and leaves the list as it was.
pub fn save(path: &Path, list: &EditList) -> std::io::Result<Saved> {
    let text = stamped(list.to_text());
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
    let mut tries = 1;
    while let Err(e) = std::fs::rename(tmp, path) {
        if tries >= RENAME_TRIES {
            return Err(e);
        }
        tries += 1;
        std::thread::sleep(RENAME_WAIT);
    }
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
