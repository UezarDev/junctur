//! `junctur` command line.
//!
//! ```text
//! junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--no-speech]
//!                       [--windows <n>] [--window-seconds <s>]
//! junctur separate <file> [--track <n>] [--start <s>] [--duration <s>] [--cpu] [--force]
//!                         [--no-speakers] [--json <out.json>] [--cache-dir <dir>]
//! ```
//!
//! Arguments are parsed by hand: two commands with a handful of flags do not justify a parser dependency.

mod args;
mod commands;

use std::process::ExitCode;

const USAGE: &str = "usage:
  junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--no-speech]
                        [--windows <n>] [--window-seconds <s>]
  junctur separate <file> [--track <n>] [--start <s>] [--duration <s>] [--cpu] [--force]
                          [--no-speakers] [--json <out.json>] [--cache-dir <dir>]";

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
    let cmd = it.next();
    let a = args::Args::parse(it).map_err(|e| format!("{e}\n{USAGE}"))?;
    match cmd.as_deref() {
        Some("tracks") => commands::tracks(a),
        Some("separate") => commands::separate(a),
        _ => Err(USAGE.into()),
    }
}
