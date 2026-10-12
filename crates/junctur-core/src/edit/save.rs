//! Writing an edit list that something else may have written before: an editor saving on every
//! change, where the file on disk may be a hand edit or another tool's output.

use std::path::{Path, PathBuf};

use super::EditList;

/// The first free backup name beside `edit_path`: `<name>.bak`, then `<name>.bak2`, ... Backups are
/// never overwritten: a single `.bak` rewritten each time lost hand edits (1d).
pub fn backup_path(edit_path: &Path) -> PathBuf {
    let name = edit_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (1..)
        .map(|n| match n {
            1 => edit_path.with_file_name(format!("{name}.bak")),
            n => edit_path.with_file_name(format!("{name}.bak{n}")),
        })
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// Writes `list` to `path` and returns the text written. The file there is first copied to
/// [`backup_path`] unless it is `own` (the text this writer saved last, so nothing of anyone
/// else's would be lost) or already holds the same text. The write goes through a temporary file
/// and a rename, so a reader (or a crash) never sees a half-written or missing list.
pub fn save(path: &Path, list: &EditList, own: Option<&str>) -> std::io::Result<String> {
    let text = list.to_text();
    match std::fs::read_to_string(path) {
        Ok(old) if old != text && Some(old.as_str()) != own => {
            std::fs::write(backup_path(path), old)?;
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let tmp = path.with_extension("junctur.tmp");
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, path)?;
    Ok(text)
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
            windows: vec![],
        }
    }

    #[test]
    fn backs_up_only_what_someone_else_wrote() {
        let dir = std::env::temp_dir().join(format!("junctur-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("edit.junctur");
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();

        // Nothing there yet: nothing to keep.
        let first = save(&path, &list(1.0), None).unwrap();
        assert_eq!(read("edit.junctur.bak"), None);
        // Our own last write is replaced without a backup.
        let second = save(&path, &list(2.0), Some(&first)).unwrap();
        assert_eq!(read("edit.junctur.bak"), None);
        assert_eq!(read("edit.junctur").as_deref(), Some(second.as_str()));
        // A hand edit since then is kept, and the next one goes to .bak2.
        std::fs::write(&path, "hand edit 1").unwrap();
        let third = save(&path, &list(3.0), Some(&second)).unwrap();
        assert_eq!(read("edit.junctur.bak").as_deref(), Some("hand edit 1"));
        std::fs::write(&path, "hand edit 2").unwrap();
        save(&path, &list(4.0), Some(&third)).unwrap();
        assert_eq!(read("edit.junctur.bak2").as_deref(), Some("hand edit 2"));
        assert_eq!(
            read("edit.junctur.tmp"),
            None,
            "the temporary file is renamed away"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
