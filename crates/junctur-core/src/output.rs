//! Guards for files Junctur writes where the user said. The recording is only ever read, so no
//! output may land on it, under any of its names.

use std::path::Path;

use crate::{Error, Result};

/// Whether two paths name one file. Asks the OS for the file's identity rather than comparing
/// canonical paths, which a hard link (a second name for the same file) passes. A path that does
/// not exist is never the same file.
pub fn same_file(a: &Path, b: &Path) -> bool {
    same_file::is_same_file(a, b).unwrap_or(false)
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
}
