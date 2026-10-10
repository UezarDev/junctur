//! `junctur` command line.
//!
//! ```text
//! junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--no-speech]
//!                       [--windows <n>] [--window-seconds <s>]
//! junctur separate <file> [--track <n>] [--start <s>] [--duration <s>] [--cpu] [--force]
//!                         [--no-speakers] [--json <out.json>] [--cache-dir <dir>]
//! junctur analyze <file> [--project <dir>] [--cache-dir <dir>] [--cpu] [--model <m>] [--force]
//!                        [--no-transcript] [cut flags]
//! junctur status <file> [--project <dir>] [--cache-dir <dir>]
//! junctur cut <file> [--project <dir>] [--cache-dir <dir>] [cut flags]
//! junctur export <file> --fcpxml <out.fcpxml> [--project <dir>] [--cache-dir <dir>]
//! ```
//!
//! Arguments are parsed by hand: a few commands with a handful of flags do not justify a parser dependency.

mod args;
mod commands;
mod project;

use std::process::ExitCode;

const USAGE: &str = "usage:
  junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--no-speech]
                        [--windows <n>] [--window-seconds <s>]
  junctur separate <file> [--track <n>] [--start <s>] [--duration <s>] [--cpu] [--force]
                          [--no-speakers] [--json <out.json>] [--cache-dir <dir>]
  junctur analyze <file> [--project <dir>] [--cache-dir <dir>] [--cpu] [--force] [--no-transcript]
                         [--model auto|large-v3|large-v3-q5_0] [cut flags]
  junctur status <file> [--project <dir>] [--cache-dir <dir>]
  junctur cut <file> [--project <dir>] [--cache-dir <dir>] [cut flags]
  junctur export <file> --fcpxml <out.fcpxml> [--project <dir>] [--cache-dir <dir>]
cut flags: [--pad-before <s>] [--pad-after <s>] [--merge-gap <s>] [--crossfade <ms>]";

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
        Some("analyze") => project::analyze(a),
        Some("status") => project::status(a),
        Some("cut") => project::cut(a),
        Some("export") => project::export(a),
        _ => Err(USAGE.into()),
    }
}
