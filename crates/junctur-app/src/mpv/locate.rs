//! Where `libmpv-2.dll` is looked for. `scripts/fetch-libmpv.ps1` puts it in `vendor/mpv/`.

use std::path::{Path, PathBuf};

pub const DLL: &str = "libmpv-2.dll";

/// Candidates in order: `JUNCTUR_LIBMPV` (a file), beside the executable (a shipped app), then the
/// source tree's `vendor/mpv/` (a dev build run from anywhere).
pub fn candidates(env: Option<PathBuf>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    out.extend(env);
    if let Some(d) = exe_dir {
        out.push(d.join(DLL));
    }
    out.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/mpv")
            .join(DLL),
    );
    out
}

pub fn find() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(Path::parent);
    let all = candidates(
        std::env::var_os("JUNCTUR_LIBMPV").map(PathBuf::from),
        exe_dir,
    );
    all.iter().find(|p| p.is_file()).cloned().ok_or_else(|| {
        let tried: Vec<String> = all.iter().map(|p| p.display().to_string()).collect();
        format!(
            "{DLL} not found (run scripts/fetch-libmpv.ps1, or set JUNCTUR_LIBMPV). Tried: {}",
            tried.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_first_then_exe_dir_then_vendor() {
        let c = candidates(Some("C:/x/mpv.dll".into()), Some(Path::new("C:/app")));
        assert_eq!(c[0], PathBuf::from("C:/x/mpv.dll"));
        assert_eq!(c[1], Path::new("C:/app").join(DLL));
        assert!(c[2].ends_with(Path::new("vendor/mpv").join(DLL)));
        assert_eq!(candidates(None, None).len(), 1);
    }
}
