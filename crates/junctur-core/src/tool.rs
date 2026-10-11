//! Running an external program and collecting its output, shared by the ffmpeg and whisper.cpp
//! wrappers.

use std::process::{Command, Stdio};

use crate::{Error, Result};

pub(crate) fn run(program: &'static str, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Spawn { program, source })?;
    if !out.status.success() {
        return Err(Error::Tool {
            program,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(out.stdout)
}
