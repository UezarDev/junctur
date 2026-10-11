//! `junctur` command line:
//!
//! ```text
//! junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache]
//!                       [--windows <n>] [--window-seconds <s>]
//! junctur transcribe <file> [--model large-v3-turbo|large-v3] [--language <code>]
//!                           [--json <out.json>] [--cache-dir <dir>] [--no-cache]
//! ```
//!
//! Arguments are parsed by hand: two commands with a handful of flags do not justify a parser
//! dependency.

mod tracks;
mod transcribe;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:
  junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--windows <n>] \
[--window-seconds <s>]
  junctur transcribe <file> [--model large-v3-turbo|large-v3] [--language <code>] \
[--json <out.json>] [--cache-dir <dir>] [--no-cache]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("junctur: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("tracks") => tracks::run(it),
        Some("transcribe") => transcribe::run(it),
        _ => Err(USAGE.into()),
    }
}

/// The value after flag `name`.
fn value(it: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    it.next().ok_or(format!("{name} needs a value\n{USAGE}"))
}

fn write_json(out: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(out, body).map_err(|e| format!("{}: {e}", out.display()))
}

fn input(file: Option<PathBuf>) -> Result<PathBuf, String> {
    file.ok_or_else(|| USAGE.into())
}
