//! The OS's per-user folders Junctur keeps files in, found one way: the Windows variable, else the
//! XDG one, else the usual folder under `HOME`.

use std::path::PathBuf;

fn user_dir(windows: &str, xdg: &str, under_home: &str) -> Option<PathBuf> {
    std::env::var_os(windows)
        .or_else(|| std::env::var_os(xdg))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(under_home)))
}

/// `%APPDATA%`, `$XDG_CONFIG_HOME` or `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    user_dir("APPDATA", "XDG_CONFIG_HOME", ".config")
}

/// `%LOCALAPPDATA%`, `$XDG_CACHE_HOME` or `~/.cache`.
pub fn cache_home() -> Option<PathBuf> {
    user_dir("LOCALAPPDATA", "XDG_CACHE_HOME", ".cache")
}
