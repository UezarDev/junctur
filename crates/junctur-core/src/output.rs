//! Guards for files Junctur writes where the user said. The recording is only ever read, so no
//! output may land on it, under any of its names. And replacing a file whole ([`write_replacing`]),
//! so a reader, or a close in the middle, never finds it half-written.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::{Error, Result};

/// Whether two paths name one file. Asks the OS for the file's identity rather than comparing
/// canonical paths, which a hard link (a second name for the same file) passes. A path that does
/// not exist is never the same file.
pub fn same_file(a: &Path, b: &Path) -> bool {
    same_file::is_same_file(a, b).unwrap_or(false)
}

/// A rename can meet a reader (an antivirus scan, the edit-list watch) holding the file on
/// Windows; it is tried this many times, this far apart, before it fails.
const RENAME_TRIES: u32 = 6;
const RENAME_WAIT: Duration = Duration::from_millis(40);

/// Renames `tmp` over `path`, retrying while a reader holds `path` ([`RENAME_TRIES`]).
pub fn rename_over(tmp: &Path, path: &Path) -> std::io::Result<()> {
    let mut tries = 1;
    while let Err(e) = std::fs::rename(tmp, path) {
        if tries >= RENAME_TRIES {
            return Err(e);
        }
        tries += 1;
        std::thread::sleep(RENAME_WAIT);
    }
    Ok(())
}

/// Removes `path`, retrying while a reader holds it, as [`rename_over`] does; gone already is done.
pub fn remove_retrying(path: &Path) -> std::io::Result<()> {
    let mut tries = 1;
    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if tries >= RENAME_TRIES => return Err(e),
            Err(_) => {
                tries += 1;
                std::thread::sleep(RENAME_WAIT);
            }
        }
    }
}

/// A temporary name beside `path` no other writer uses: this process and a counter, so two
/// writers (two threads, or the app and the CLI) never write one temporary file.
fn temp_beside(path: &Path) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Writes `bytes` to `path` through a temporary file renamed over it: `path` holds the old content
/// or the new, never part of either. The temporary file is removed when anything fails.
pub fn write_replacing(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let tmp = temp_beside(path);
    let result = std::fs::write(&tmp, bytes).and_then(|()| rename_over(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `<name>.<suffix>` beside `path` for `n` 1, else `<name>.<suffix><n>`: backups and files set
/// aside, numbered so none is ever overwritten.
pub fn numbered(path: &Path, suffix: &str, n: u32) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match n {
        1 => path.with_file_name(format!("{name}.{suffix}")),
        n => path.with_file_name(format!("{name}.{suffix}{n}")),
    }
}

/// The first [`numbered`] name that does not exist.
pub fn first_free(path: &Path, suffix: &str) -> PathBuf {
    (1..)
        .map(|n| numbered(path, suffix, n))
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// Refuse `out` when it is `source`.
pub fn refuse_source(source: &Path, out: &Path) -> Result<()> {
    if same_file(source, out) {
        return Err(Error::Output(format!(
            "{} is the source recording; pick another output path",
            out.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_links_and_other_spellings_are_the_source() {
        let dir = std::env::temp_dir().join(format!("junctur-output-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        std::fs::write(&src, b"x").unwrap();
        let link = dir.join("link.mp4");
        std::fs::hard_link(&src, &link).unwrap();
        assert!(refuse_source(&src, &src).is_err());
        assert!(refuse_source(&src, &dir.join("./in.mp4")).is_err());
        assert!(
            refuse_source(&src, &link).is_err(),
            "a hard link is the source"
        );
        assert!(refuse_source(&src, &dir.join("missing.json")).is_ok());
        std::fs::write(dir.join("other.json"), b"x").unwrap();
        assert!(refuse_source(&src, &dir.join("other.json")).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replacing_leaves_the_new_content_and_no_temporary_file() {
        let d = std::env::temp_dir().join(format!("junctur-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("jobs.json");
        write_replacing(&p, "old").unwrap();
        write_replacing(&p, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let names: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("jobs.json")]);
        assert_ne!(temp_beside(&p), temp_beside(&p), "a fresh name per write");
        // A folder that is not there: the error comes back, nothing is left behind.
        assert!(write_replacing(&d.join("no").join("x.json"), "x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
