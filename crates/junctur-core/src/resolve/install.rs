//! Putting the Junctur script into Resolve's Scripts folder, where Workspace > Scripts lists it
//! (5g), and the caption template bin into the mailbox for the script to import. The script is
//! written with the mailbox paths in it: Resolve's sandboxed script state has no environment of
//! Junctur's to find them from. Written only when the contents differ, so sending again is cheap.

use std::path::{Path, PathBuf};

use super::bridge;

const SCRIPT: &str = include_str!("junctur.lua");
/// auto-subs' `caption-bin.drb` (tmoroney/auto-subs @ c98f6df, MIT, `LICENSE-auto-subs` beside it):
/// a Resolve bin holding a plain Text+ title, "Basic Template", which the script keeps as Junctur's
/// template. Byte for byte theirs: without a Resolve to make one in, a bin that is known to import
/// beats an edited copy.
const TEMPLATE_BIN: &[u8] = include_bytes!("caption-bin.drb");
const BIN_NAME: &str = "junctur-caption-bin.drb";

/// Where the script was put.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub script: PathBuf,
}

/// Installs or refreshes the script and the template bin.
pub fn install() -> Result<Installed, String> {
    let support = bridge::support().ok_or("cannot tell where DaVinci Resolve keeps its scripts")?;
    if !support.is_dir() {
        return Err(format!(
            "DaVinci Resolve does not seem to be installed here (no {}); run it once, then try again",
            support.display()
        ));
    }
    let mailbox = bridge::mailbox().ok_or("no folder for Junctur's settings")?;
    let script = support
        .join("Fusion")
        .join("Scripts")
        .join("Utility")
        .join("Junctur.lua");
    let bin = mailbox.join(BIN_NAME);
    put(&bin, TEMPLATE_BIN)?;
    put(
        &script,
        script_for(&mailbox.join("request.lua"), &bin).as_bytes(),
    )?;
    Ok(Installed { script })
}

/// The script with its paths in, each as a Lua long string (no escapes to get wrong in a path).
fn script_for(request: &Path, bin: &Path) -> String {
    SCRIPT
        .replace(
            "__JUNCTUR_REQUEST__",
            &long_string(&request.to_string_lossy()),
        )
        .replace(
            "__JUNCTUR_TEMPLATE_BIN__",
            &long_string(&bin.to_string_lossy()),
        )
}

/// `[==[…]==]` with enough `=` that the text cannot close it.
fn long_string(s: &str) -> String {
    let mut level = 0;
    while s.contains(&format!("]{}]", "=".repeat(level))) {
        level += 1;
    }
    let eq = "=".repeat(level);
    format!("[{eq}[{s}]{eq}]")
}

/// Writes `bytes` to `path` unless it already holds them; whether it wrote.
fn put(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    let dir = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(path, bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_go_in_as_long_strings() {
        assert_eq!(long_string("C:\\a b\\c"), "[[C:\\a b\\c]]");
        assert_eq!(long_string("x]]y"), "[=[x]]y]=]");
        let s = script_for(Path::new("C:\\J\\request.lua"), Path::new("C:\\J\\bin.drb"));
        assert!(s.contains("local REQUEST = [[C:\\J\\request.lua]]"), "{s}");
        assert!(s.contains("local TEMPLATE_BIN = [[C:\\J\\bin.drb]]"));
        assert!(!s.contains("__JUNCTUR_"));
    }

    #[test]
    fn the_template_bin_is_the_zip_it_was() {
        // A zip (`PK`) of Resolve's bin XML, as auto-subs ships it.
        assert_eq!(&TEMPLATE_BIN[..2], b"PK");
        assert_eq!(TEMPLATE_BIN.len(), 26924);
    }

    #[test]
    fn unchanged_files_are_not_written_again() {
        let d =
            std::env::temp_dir().join(format!("junctur-resolve-install-{}", std::process::id()));
        let f = d.join("sub").join("x.lua");
        assert!(put(&f, b"one").unwrap());
        assert!(!put(&f, b"one").unwrap());
        assert!(put(&f, b"two").unwrap());
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&d);
    }
}
