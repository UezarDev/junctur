//! `junctur` command line. Phase 1 has one command:
//!
//! ```text
//! junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache]
//!                       [--windows <n>] [--window-seconds <s>]
//! ```
//!
//! Arguments are parsed by hand: one command with five flags does not justify a parser dependency.

use std::path::PathBuf;
use std::process::ExitCode;

use junctur_core::{analyze, AnalyzeOptions};

const USAGE: &str = "usage: junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] \
[--no-cache] [--windows <n>] [--window-seconds <s>]";

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
        Some("tracks") => {}
        _ => return Err(USAGE.into()),
    }
    let mut file = None;
    let mut json = None;
    let mut opts = AnalyzeOptions::default();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value\n{USAGE}"));
        match a.as_str() {
            "--json" => json = Some(PathBuf::from(val("--json")?)),
            "--cache-dir" => opts.cache_dir = Some(PathBuf::from(val("--cache-dir")?)),
            "--no-cache" => opts.use_cache = false,
            "--windows" => {
                opts.windows = val("--windows")?
                    .parse()
                    .map_err(|_| "--windows must be a whole number")?
            }
            "--window-seconds" => {
                opts.window_seconds = val("--window-seconds")?
                    .parse()
                    .map_err(|_| "--window-seconds must be a number")?
            }
            _ if file.is_none() && !a.starts_with("--") => file = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}\n{USAGE}")),
        }
    }
    let file = file.ok_or(USAGE)?;
    let report = analyze(&file, &opts).map_err(|e| e.to_string())?;
    print!("{}", report.render());
    if let Some(out) = json {
        let body = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(&out, body).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(())
}
