//! The OS's per-user folders Junctur keeps files in, found one way: the Windows variable, else the
//! XDG one, else the usual folder under `HOME`.

use std::ffi::OsString;
use std::path::PathBuf;

/// The rule over `var` (the environment, or a test's own).
fn user_dir(
    var: impl Fn(&str) -> Option<OsString>,
    windows: &str,
    xdg: &str,
    under_home: &str,
) -> Option<PathBuf> {
    var(windows)
        .or_else(|| var(xdg))
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(under_home)))
}

fn env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// `%APPDATA%`, `$XDG_CONFIG_HOME` or `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    user_dir(env, "APPDATA", "XDG_CONFIG_HOME", ".config")
}

/// `%LOCALAPPDATA%`, `$XDG_CACHE_HOME` or `~/.cache`.
pub fn cache_home() -> Option<PathBuf> {
    user_dir(env, "LOCALAPPDATA", "XDG_CACHE_HOME", ".cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_variable_then_xdg_then_home() {
        let with = |set: &'static [(&'static str, &'static str)]| {
            move |n: &str| {
                set.iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        let dir = |set| user_dir(with(set), "APPDATA", "XDG_CONFIG_HOME", ".config");
        let all = &[("APPDATA", "A"), ("XDG_CONFIG_HOME", "X"), ("HOME", "H")];
        assert_eq!(dir(all), Some(PathBuf::from("A")));
        assert_eq!(dir(&all[1..]), Some(PathBuf::from("X")));
        assert_eq!(dir(&all[2..]), Some(PathBuf::from("H").join(".config")));
        assert_eq!(dir(&[]), None);
        // Another rule's variables are not this one's.
        assert_eq!(dir(&[("LOCALAPPDATA", "L"), ("XDG_CACHE_HOME", "C")]), None);
    }
}
