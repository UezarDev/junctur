//! The recordings opened before, for the start window (2d): `junctur/recent.json` beside the user's
//! config and voices (`JUNCTUR_RECENT` names another file, as test mode does), newest first.
//!
//! Only paths and when each was opened: anything about a recording's analysis would mean opening
//! every project to draw the list. A recent list that cannot be read is an empty one, and one that
//! cannot be written is said on stderr: neither is worth stopping a recording from opening.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const ENV: &str = "JUNCTUR_RECENT";

/// As many as the start window lists.
const MAX: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    path: String,
    /// Unix seconds.
    opened: u64,
}

/// A recent recording as the start window shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub path: String,
    pub name: String,
    pub folder: String,
    /// Unix seconds.
    pub opened: u64,
    /// The file is not there any more (moved, deleted, a drive not connected).
    pub missing: bool,
}

fn file() -> Option<PathBuf> {
    match std::env::var_os(ENV) {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => junctur_core::config::folder().map(|f| f.join("recent.json")),
    }
}

fn read(path: &Path) -> Vec<Entry> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Through a temporary file of its own and a rename, so a reader never sees half a list and two
/// writers never share a temporary file.
fn write(path: &Path, list: &[Entry]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    junctur_core::output::write_replacing(path, json).map_err(|e| e.to_string())
}

/// `list` with `path` first, opened at `now`; each path once, at most [`MAX`]. Paths are compared
/// ignoring case on Windows, where `C:\A.mp4` and `c:\a.mp4` are one file.
fn push(mut list: Vec<Entry>, path: &str, now: u64) -> Vec<Entry> {
    list.retain(|e| !same(&e.path, path));
    list.insert(
        0,
        Entry {
            path: path.to_string(),
            opened: now,
        },
    );
    list.truncate(MAX);
    list
}

fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn view(e: Entry) -> Recent {
    let p = Path::new(&e.path);
    Recent {
        name: p
            .file_name()
            .map_or_else(|| e.path.clone(), |n| n.to_string_lossy().into_owned()),
        folder: p
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        missing: !p.is_file(),
        path: e.path,
        opened: e.opened,
    }
}

fn list_at(path: &Path) -> Vec<Recent> {
    read(path).into_iter().map(view).collect()
}

fn add_at(path: &Path, recording: &Path, now: u64) -> Result<(), String> {
    let abs = std::path::absolute(recording).map_err(|e| e.to_string())?;
    write(path, &push(read(path), &abs.display().to_string(), now))
}

fn remove_at(path: &Path, recording: &str) -> Result<(), String> {
    let mut list = read(path);
    list.retain(|e| !same(&e.path, recording));
    write(path, &list)
}

pub fn list() -> Vec<Recent> {
    file().map(|f| list_at(&f)).unwrap_or_default()
}

/// Puts `recording` first in the list.
pub fn add(recording: &Path) {
    let Some(f) = file() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Err(e) = add_at(&f, recording, now) {
        eprintln!("junctur-app: recent recordings not saved: {e}");
    }
}

pub fn remove(recording: &str) -> Result<(), String> {
    let f = file().ok_or("there is no config folder to keep recent recordings in")?;
    remove_at(&f, recording)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, opened: u64) -> Entry {
        Entry {
            path: path.into(),
            opened,
        }
    }

    #[test]
    fn newest_first_once_each_and_capped() {
        let list = push(vec![entry("a", 1), entry("b", 2)], "b", 9);
        assert_eq!(list, vec![entry("b", 9), entry("a", 1)]);
        let mut list = Vec::new();
        for i in 0..(MAX as u64 + 5) {
            list = push(list, &format!("f{i}"), i);
        }
        assert_eq!(list.len(), MAX);
        assert_eq!(list[0].path, format!("f{}", MAX + 4));
    }

    #[test]
    fn kept_on_disk_and_missing_files_marked() {
        let d = std::env::temp_dir().join(format!("junctur-app-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let store = d.join("sub").join("recent.json");
        assert!(list_at(&store).is_empty(), "no file yet: empty");
        let rec = d.join("rec.mp4");
        std::fs::write(&rec, b"x").unwrap();
        add_at(&store, &rec, 5).unwrap();
        add_at(&store, &d.join("gone.mp4"), 6).unwrap();
        let l = list_at(&store);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].name.as_str(), l[0].missing), ("gone.mp4", true));
        assert_eq!((l[1].name.as_str(), l[1].missing), ("rec.mp4", false));
        assert_eq!(l[1].folder, d.display().to_string());
        remove_at(&store, &l[0].path).unwrap();
        assert_eq!(list_at(&store).len(), 1);
        // A broken file is an empty list, and the next add replaces it.
        std::fs::write(&store, "{ not json").unwrap();
        assert!(list_at(&store).is_empty());
        add_at(&store, &rec, 7).unwrap();
        assert_eq!(list_at(&store).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
