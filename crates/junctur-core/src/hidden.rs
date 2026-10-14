//! Copies of the user's AppData folders that Windows hid in `Packages\<package>\LocalCache` for a
//! process started by a packaged (MSIX) app; such a process cannot escape the redirect, so the real
//! folder copies in what a hidden copy has and it lacks: never replacing, once settled, once each.

use std::collections::{BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// A file modified more recently than this may still be written by a run inside the hidden copy.
pub const SETTLED: Duration = Duration::from_secs(5);

/// Inside a folder that took copies: the names it took, so none is taken twice.
pub const LEDGER: &str = ".adopted.json";

/// Where hidden copies live: `%LOCALAPPDATA%`, `%APPDATA%` and the packages under the former.
#[derive(Debug)]
pub struct Lookup {
    local: PathBuf,
    roaming: Option<PathBuf>,
    packages: Vec<PathBuf>,
}

impl Lookup {
    pub fn new(local: &Path, roaming: Option<&Path>) -> Self {
        let mut packages: Vec<PathBuf> = fs::read_dir(local.join("Packages"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        packages.sort();
        Self {
            local: local.to_path_buf(),
            roaming: roaming.map(Path::to_path_buf),
            packages,
        }
    }

    /// This system's, listed once per process; `None` off Windows, where nothing is hidden.
    pub fn system() -> Option<&'static Lookup> {
        static LOOKUP: OnceLock<Option<Lookup>> = OnceLock::new();
        LOOKUP
            .get_or_init(|| {
                if !cfg!(windows) {
                    return None;
                }
                let local = std::env::var_os("LOCALAPPDATA")?;
                let roaming = std::env::var_os("APPDATA");
                Some(Lookup::new(
                    Path::new(&local),
                    roaming.as_deref().map(Path::new),
                ))
            })
            .as_ref()
    }

    /// The hidden copies of `real` that exist, one per packaged app whose children wrote there. Empty
    /// outside AppData, for AppData itself and inside `Packages`.
    pub fn copies(&self, real: &Path) -> Vec<PathBuf> {
        let (kind, rest) = match relative(real, &self.local) {
            Some(rest) => ("Local", rest),
            None => match self.roaming.as_deref().and_then(|r| relative(real, r)) {
                Some(rest) => ("Roaming", rest),
                None => return Vec::new(),
            },
        };
        let first = rest.components().next().map(|c| lower(c.as_os_str()));
        if first.is_none() || first.as_deref() == Some("packages") {
            return Vec::new();
        }
        self.packages
            .iter()
            .map(|p| p.join("LocalCache").join(kind).join(&rest))
            .filter(|p| p.is_dir())
            .collect()
    }
}

fn lower(s: &OsStr) -> String {
    s.to_string_lossy().to_lowercase()
}

/// `path` without its `\\?\` prefix, the spelling `canonicalize` gives on Windows.
fn plain(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match (s.strip_prefix(r"\\?\UNC\"), s.strip_prefix(r"\\?\")) {
        (Some(rest), _) => PathBuf::from(format!(r"\\{rest}")),
        (None, Some(rest)) => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// `path` below `base`, comparing names regardless of case (Windows paths are case-insensitive, and
/// a cache folder may be spelled differently from the environment's `%LOCALAPPDATA%`).
fn relative(path: &Path, base: &Path) -> Option<PathBuf> {
    let (path, base) = (plain(path), plain(base));
    let key = |c: Component| lower(c.as_os_str());
    let mut parts = path.components();
    for b in base.components() {
        if key(parts.next()?) != key(b) {
            return None;
        }
    }
    Some(parts.collect())
}

/// What one adoption did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Adopted {
    /// Files copied in.
    pub copied: usize,
    /// Files both had that differ; the real one kept.
    pub differ: usize,
    /// Files skipped for now: still being written, changed while copied, or not readable. Tried again
    /// on a later pass.
    pub pending: usize,
}

/// The names a folder took from hidden copies, kept in [`LEDGER`] inside it.
pub struct Ledger {
    path: PathBuf,
    names: BTreeSet<String>,
    changed: bool,
}

impl Ledger {
    /// A missing or unreadable ledger is an empty one.
    pub fn load(folder: &Path) -> Self {
        let path = folder.join(LEDGER);
        let names = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            names,
            changed: false,
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub fn add(&mut self, name: String) {
        self.changed |= self.names.insert(name);
    }

    pub fn save(self) -> io::Result<()> {
        if !self.changed {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::output::write_replacing(&self.path, serde_json::to_vec_pretty(&self.names)?)
    }
}

/// A name for [`Ledger`]: `/`-separated, the same on every system.
fn ledger_name(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every file under `dir`, relative to it, without temporary files and ledgers.
pub fn files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let rel = rel.join(e.file_name());
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path(), &rel, out),
                Ok(t) if t.is_file() && !is_temporary(&rel) && !rel.ends_with(LEDGER) => {
                    out.push(rel)
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out);
    out.sort();
    out
}

/// Another writer's half-done file (`output::write_replacing`, the edit list's save, a copy here).
pub fn is_temporary(p: &Path) -> bool {
    p.extension().is_some_and(|x| x == "tmp")
}

/// Whether `dir` holds no temporary file and nothing modified within [`SETTLED`] of `now`.
pub fn settled(dir: &Path, now: SystemTime) -> bool {
    fn walk(dir: &Path, since: SystemTime) -> bool {
        fs::read_dir(dir).into_iter().flatten().flatten().all(|e| {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&p, since),
                _ => !is_temporary(&p) && file_settled(&p, since),
            }
        })
    }
    walk(dir, now - SETTLED)
}

fn file_settled(p: &Path, since: SystemTime) -> bool {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t <= since)
}

/// Length and modified time: what says a file is the one seen before.
pub fn stamp(p: &Path) -> Option<(u64, SystemTime)> {
    let m = fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Copies `src` to the missing `dst` with its modified time. `Ok(false)` when `dst` appeared meanwhile
/// (kept, not replaced). Fails, leaving nothing, when `src` changed while being copied. The copy is
/// hard-linked into place from a temporary file, so it appears whole; where hard links are refused
/// (FAT, some network drives) it is written to `dst` opened as new, removed again on a failure.
pub fn copy_in(src: &Path, dst: &Path) -> io::Result<bool> {
    let before = stamp(src).ok_or_else(|| io::Error::new(ErrorKind::NotFound, "no source"))?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut name: OsString = dst.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".adopt.{}.tmp", std::process::id()));
    let tmp = dst.with_file_name(name);
    let result = (|| {
        fs::copy(src, &tmp)?;
        set_modified(&tmp, before.1)?;
        if stamp(src) != Some(before) {
            return Err(io::Error::new(
                ErrorKind::Interrupted,
                "changed while copied",
            ));
        }
        match fs::hard_link(&tmp, dst) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(_) => write_new(&tmp, dst, before.1),
        }
    })();
    let _ = fs::remove_file(&tmp);
    result
}

fn set_modified(p: &Path, t: SystemTime) -> io::Result<()> {
    fs::File::options().write(true).open(p)?.set_modified(t)
}

fn write_new(tmp: &Path, dst: &Path, modified: SystemTime) -> io::Result<bool> {
    let mut out = match fs::File::options().write(true).create_new(true).open(dst) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    let written =
        io::copy(&mut fs::File::open(tmp)?, &mut out).and_then(|_| out.set_modified(modified));
    drop(out);
    match written {
        Ok(()) => Ok(true),
        Err(e) => {
            let _ = fs::remove_file(dst);
            Err(e)
        }
    }
}

/// Takes, file by file, what the hidden copies of `real` hold and `real` lacks, for the files `keep`
/// accepts (relative paths). Each file is taken once ([`Ledger`]), from the first copy that has it
/// settled; a file `real` has is never touched.
pub fn adopt_files_with(
    lookup: &Lookup,
    real: &Path,
    keep: impl Fn(&Path) -> bool,
    now: SystemTime,
) -> Adopted {
    let mut out = Adopted::default();
    let hidden = lookup.copies(real);
    if hidden.is_empty() {
        return out;
    }
    let mut ledger = Ledger::load(real);
    let since = now - SETTLED;
    for h in &hidden {
        for rel in files(h).into_iter().filter(|r| keep(r)) {
            let name = ledger_name(&rel);
            let (src, dst) = (h.join(&rel), real.join(&rel));
            if ledger.has(&name) {
                continue;
            }
            if dst.exists() {
                if stamp(&src) != stamp(&dst) {
                    out.differ += 1;
                }
                continue;
            }
            if !file_settled(&src, since) {
                out.pending += 1;
                continue;
            }
            match copy_in(&src, &dst) {
                Ok(true) => {
                    out.copied += 1;
                    ledger.add(name);
                }
                Ok(false) => out.differ += 1,
                Err(_) => out.pending += 1,
            }
        }
    }
    if ledger.save().is_err() {
        eprintln!("junctur: could not record what {} took", real.display());
    }
    report(real, &hidden, out);
    out
}

/// [`adopt_files_with`] on this system, once per process for each folder.
pub fn adopt_files_once(real: &Path, keep: impl Fn(&Path) -> bool) {
    static DONE: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
    let Some(lookup) = Lookup::system() else {
        return;
    };
    {
        let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
        if !done
            .get_or_insert_with(HashSet::new)
            .insert(real.to_path_buf())
        {
            return;
        }
    }
    adopt_files_with(lookup, real, keep, SystemTime::now());
}

/// One line on stderr when an adoption did anything worth knowing, the way a model download is told.
pub fn report(real: &Path, hidden: &[PathBuf], a: Adopted) {
    let from = hidden
        .iter()
        .map(|h| h.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if a.copied > 0 {
        eprintln!(
            "junctur: copied {} file(s) a packaged app's process left in {from} into {}",
            a.copied,
            real.display()
        );
    }
    if a.differ > 0 || a.pending > 0 {
        eprintln!(
            "junctur: {from} holds {} file(s) that differ from {} (kept the latter's) and {} not taken yet",
            a.differ,
            real.display(),
            a.pending
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn scratch(name: &str) -> PathBuf {
        crate::test_support::scratch("hidden", name)
    }

    /// Writes a file modified a minute ago (settled), or now when `fresh`.
    pub fn write(p: &Path, s: &str, fresh: bool) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
        if !fresh {
            set_modified(p, SystemTime::now() - Duration::from_secs(60)).unwrap();
        }
    }

    /// A fake AppData under `d`: the lookup, the real Local, and packages A's and B's hidden Local.
    pub fn appdata(d: &Path) -> (Lookup, PathBuf, PathBuf, PathBuf) {
        let local = d.join("Local");
        let a = local.join("Packages/A_1/LocalCache/Local");
        let b = local.join("Packages/B_2/LocalCache/Local");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        (Lookup::new(&local, Some(&d.join("Roaming"))), local, a, b)
    }

    #[test]
    fn finds_each_packages_copy_of_a_local_or_roaming_folder() {
        let d = scratch("copies");
        let (local, roaming) = (d.join("Local"), d.join("Roaming"));
        let pkgs = local.join("Packages");
        fs::create_dir_all(pkgs.join("A_1/LocalCache/Local/junctur/cache")).unwrap();
        fs::create_dir_all(pkgs.join("B_2/LocalCache/Local/junctur")).unwrap();
        fs::create_dir_all(pkgs.join("B_2/LocalCache/Roaming/junctur")).unwrap();
        let l = Lookup::new(&local, Some(&roaming));

        assert_eq!(
            l.copies(&local.join("junctur/cache")),
            vec![pkgs.join("A_1/LocalCache/Local/junctur/cache")]
        );
        assert_eq!(
            l.copies(&roaming.join("junctur")),
            vec![pkgs.join("B_2/LocalCache/Roaming/junctur")]
        );
        // Outside AppData, AppData itself, and the Packages folder have none.
        assert!(l.copies(&d.join("elsewhere")).is_empty());
        assert!(l.copies(&local).is_empty());
        assert!(l.copies(&pkgs.join("A_1")).is_empty());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_differently_cased_or_verbatim_folder_finds_its_copies() {
        let d = scratch("case");
        let (l, local, a, _) = appdata(&d);
        fs::create_dir_all(a.join("junctur/cache")).unwrap();
        let shouted = PathBuf::from(local.to_string_lossy().to_uppercase()).join("junctur/cache");
        assert_eq!(l.copies(&shouted), vec![a.join("junctur/cache")]);
        if cfg!(windows) {
            assert_eq!(
                relative(
                    Path::new(r"\\?\C:\Users\U\AppData\Local\junctur"),
                    Path::new(r"c:\users\u\appdata\local")
                ),
                Some(PathBuf::from("junctur"))
            );
        }
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn takes_settled_missing_files_once_and_never_replaces_the_users() {
        let d = scratch("files");
        let (l, local, a, b) = appdata(&d);
        let real = local.join("junctur");
        write(&a.join("junctur/voices.json"), "hidden voices", false);
        write(&a.join("junctur/config.json"), "hidden config", false);
        write(&a.join("junctur/recent.json"), "being written", true);
        write(&a.join("junctur/voices.json.tmp"), "half", false);
        write(&b.join("junctur/config.json"), "the other copy's", false);
        write(&real.join("voices.json"), "the user's voices", false);
        let all = |_: &Path| true;

        let first = adopt_files_with(&l, &real, all, SystemTime::now());
        assert_eq!(
            first,
            Adopted {
                copied: 1,
                differ: 1,
                pending: 1
            }
        );
        let read = |p: &str| fs::read_to_string(real.join(p)).ok();
        assert_eq!(read("config.json").as_deref(), Some("hidden config"));
        assert_eq!(read("voices.json").as_deref(), Some("the user's voices"));
        assert_eq!(read("recent.json"), None, "not settled yet");
        assert_eq!(read("voices.json.tmp"), None);
        assert!(files(&real).iter().all(|f| !is_temporary(f)));
        assert_eq!(
            fs::read_to_string(a.join("junctur/voices.json")).unwrap(),
            "hidden voices",
            "the hidden copy is untouched"
        );

        // A taken config the user deletes does not come back, from either copy; the settled
        // recent.json comes.
        fs::remove_file(real.join("config.json")).unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(adopt_files_with(&l, &real, all, later).copied, 1);
        assert_eq!(read("config.json"), None);
        assert_eq!(read("recent.json").as_deref(), Some("being written"));

        // Only what `keep` accepts.
        let other = local.join("other");
        write(&a.join("other/x.json"), "x", false);
        write(&a.join("other/y.json"), "y", false);
        adopt_files_with(&l, &other, |r| r == Path::new("y.json"), SystemTime::now());
        assert!(!other.join("x.json").exists() && other.join("y.json").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_copy_keeps_the_time_and_never_replaces() {
        let d = scratch("copy");
        let (src, dst) = (d.join("src.json"), d.join("dst.json"));
        write(&src, "first", false);
        assert!(copy_in(&src, &dst).unwrap());
        assert_eq!(stamp(&src), stamp(&dst), "the modified time kept");
        fs::write(&dst, "the user's").unwrap();
        assert!(!copy_in(&src, &dst).unwrap());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "the user's");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_without_hidden_copies_is_left_alone() {
        let d = scratch("none");
        let (l, local, _, _) = appdata(&d);
        let real = local.join("junctur");
        assert_eq!(
            adopt_files_with(
                &l,
                &real,
                |_| panic!("nothing to ask about"),
                SystemTime::now()
            ),
            Adopted::default()
        );
        assert!(!real.exists(), "not even a ledger");
        fs::remove_dir_all(&d).unwrap();
    }
}
