//! Copies of the user's AppData folders that Windows hid from them (2f).
//!
//! A process started by a packaged (MSIX) app, such as `junctur analyze` run from Claude desktop, has
//! its new AppData folders redirected into `%LOCALAPPDATA%\Packages\<package>\LocalCache\Local\...`
//! (`...\Roaming\...` for `%APPDATA%`), and keeps writing there once a folder exists in that copy. It
//! sees both merged; every other process sees only the real folder, so a recording analysed from Claude
//! opened in the app as never analysed. Nothing inside such a process escapes it (measured: not
//! `KF_FLAG_NO_PACKAGE_REDIRECTION`, not a `\\?\` path; the processes have no package identity to
//! undo), so the real folder instead takes what a hidden copy has and it lacks ([`adopt`]).

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The hidden copies of `real` that exist: one per packaged app whose children wrote there. Empty
/// outside AppData and on other systems.
pub fn copies(real: &Path) -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }
    match std::env::var_os("LOCALAPPDATA") {
        Some(local) => copies_in(
            real,
            Path::new(&local),
            std::env::var_os("APPDATA").as_deref().map(Path::new),
        ),
        None => Vec::new(),
    }
}

/// [`copies`] for the given `%LOCALAPPDATA%` and `%APPDATA%`.
fn copies_in(real: &Path, local: &Path, roaming: Option<&Path>) -> Vec<PathBuf> {
    let (kind, rest) = match real.strip_prefix(local) {
        Ok(rest) => ("Local", rest),
        Err(_) => match roaming.and_then(|r| real.strip_prefix(r).ok()) {
            Some(rest) => ("Roaming", rest),
            None => return Vec::new(),
        },
    };
    if rest.as_os_str().is_empty() || rest.starts_with("Packages") {
        return Vec::new();
    }
    let Ok(packages) = fs::read_dir(local.join("Packages")) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = packages
        .flatten()
        .map(|p| p.path().join("LocalCache").join(kind).join(rest))
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    found
}

/// What [`adopt`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Adopted {
    /// Files the real folder lacked, copied in.
    pub copied: usize,
    /// Files both have, which differ; the real one kept.
    pub differ: usize,
    /// Files that could not be read or copied (e.g. being written right now); tried again next time.
    pub failed: usize,
}

/// Copies every file of `hidden` (recursively) that `real` lacks into `real`. Never replaces a file
/// `real` has, never moves or deletes one: the real folder is the user's, and a file it has is the same
/// or their own newer work. Each copy keeps its modified time and appears whole (hard-linked into place
/// from a temporary file, which also refuses a file that appeared meanwhile).
pub fn adopt(real: &Path, hidden: &Path) -> Adopted {
    let mut out = Adopted::default();
    walk(hidden, real, &mut out);
    out
}

fn walk(from: &Path, to: &Path, out: &mut Adopted) {
    let Ok(entries) = fs::read_dir(from) else {
        out.failed += 1;
        return;
    };
    for e in entries.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&src, &dst, out),
            Ok(t) if t.is_file() && !is_temporary(&src) => match fs::metadata(&dst) {
                Ok(_) if differs(&src, &dst) => out.differ += 1,
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => match copy_in(&src, &dst) {
                    Ok(true) => out.copied += 1,
                    Ok(false) => out.differ += 1,
                    Err(_) => out.failed += 1,
                },
                Err(_) => out.failed += 1,
            },
            _ => {}
        }
    }
}

/// Another writer's half-done file (`output::write_replacing`, the edit list's save).
fn is_temporary(p: &Path) -> bool {
    p.extension().is_some_and(|x| x == "tmp")
}

fn differs(a: &Path, b: &Path) -> bool {
    let stamp = |p: &Path| fs::metadata(p).map(|m| (m.len(), m.modified().ok())).ok();
    stamp(a) != stamp(b)
}

/// `Ok(false)` when `dst` appeared while copying (kept, not replaced).
fn copy_in(src: &Path, dst: &Path) -> std::io::Result<bool> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut name: OsString = dst.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".adopt.{}.tmp", std::process::id()));
    let tmp = dst.with_file_name(name);
    let result = copy_stamped(src, &tmp).and_then(|()| match fs::hard_link(&tmp, dst) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    });
    let _ = fs::remove_file(&tmp);
    result
}

/// A copy with the source's modified time, so [`differs`] sees the two as the same file.
fn copy_stamped(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::copy(src, dst)?;
    let modified: SystemTime = fs::metadata(src)?.modified()?;
    fs::File::options()
        .write(true)
        .open(dst)?
        .set_modified(modified)
}

/// [`adopt`]s every hidden copy of `real`, while `wanted(real)` says the real folder still takes one,
/// and says on stderr what happened when anything did.
pub fn adopt_copies(real: &Path, wanted: impl Fn(&Path) -> bool) {
    for hidden in copies(real) {
        if !wanted(real) {
            break;
        }
        let a = adopt(real, &hidden);
        if a.copied > 0 {
            eprintln!(
                "junctur: copied {} file(s) a packaged app's process left in {} into {}",
                a.copied,
                hidden.display(),
                real.display()
            );
        }
        if a.differ > 0 || a.failed > 0 {
            eprintln!(
                "junctur: {} holds {} file(s) that differ from {} (kept the latter's) and {} it could not copy",
                hidden.display(),
                a.differ,
                real.display(),
                a.failed
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("junctur-hidden-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn finds_each_packages_copy_of_a_local_or_roaming_folder() {
        let d = scratch("copies");
        let (local, roaming) = (d.join("Local"), d.join("Roaming"));
        let pkgs = local.join("Packages");
        fs::create_dir_all(pkgs.join("A_1/LocalCache/Local/junctur/cache")).unwrap();
        fs::create_dir_all(pkgs.join("B_2/LocalCache/Local/junctur")).unwrap();
        fs::create_dir_all(pkgs.join("B_2/LocalCache/Roaming/junctur")).unwrap();

        let cache = local.join("junctur/cache");
        assert_eq!(
            copies_in(&cache, &local, Some(&roaming)),
            vec![pkgs.join("A_1/LocalCache/Local/junctur/cache")]
        );
        assert_eq!(
            copies_in(&roaming.join("junctur"), &local, Some(&roaming)),
            vec![pkgs.join("B_2/LocalCache/Roaming/junctur")]
        );
        // Outside AppData, AppData itself, and the Packages folder have none.
        assert!(copies_in(&d.join("elsewhere"), &local, Some(&roaming)).is_empty());
        assert!(copies_in(&local, &local, Some(&roaming)).is_empty());
        assert!(copies_in(&pkgs.join("A_1"), &local, Some(&roaming)).is_empty());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn copies_what_the_real_folder_lacks_and_never_replaces_what_it_has() {
        let d = scratch("adopt");
        let (real, hidden) = (d.join("real"), d.join("hidden"));
        write(&hidden.join("p/jobs.json"), "hidden jobs");
        write(&hidden.join("p/transcript.json"), "words");
        write(&hidden.join("p/edit.junctur"), "hidden edit");
        write(&hidden.join("p/edit.junctur.tmp"), "half");
        write(&real.join("p/edit.junctur"), "the user's edit");

        let a = adopt(&real, &hidden);
        assert_eq!(
            a,
            Adopted {
                copied: 2,
                differ: 1,
                failed: 0
            }
        );
        let read = |p: &str| fs::read_to_string(real.join(p)).ok();
        assert_eq!(read("p/jobs.json").as_deref(), Some("hidden jobs"));
        assert_eq!(read("p/transcript.json").as_deref(), Some("words"));
        assert_eq!(read("p/edit.junctur").as_deref(), Some("the user's edit"));
        assert_eq!(
            read("p/edit.junctur.tmp"),
            None,
            "a half-written file stays behind"
        );
        assert!(fs::read_dir(real.join("p"))
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().contains(".adopt.")));
        // The hidden copy is untouched, and a second pass finds the copies the same.
        assert_eq!(
            fs::read_to_string(hidden.join("p/edit.junctur")).unwrap(),
            "hidden edit"
        );
        assert_eq!(
            adopt(&real, &hidden),
            Adopted {
                copied: 0,
                differ: 1,
                failed: 0
            }
        );
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_without_hidden_copies_is_left_alone() {
        // No package wrote a copy of this scratch folder, so nothing is asked or copied.
        let d = scratch("wanted");
        adopt_copies(&d, |_| panic!("no hidden copy to ask about"));
        fs::remove_dir_all(&d).unwrap();
    }
}
