//! `junctur tracks`: what is on each audio track.

use std::path::PathBuf;

use junctur_core::{analyze, AnalyzeOptions};

use crate::{input, value, write_json, USAGE};

pub fn run(mut it: impl Iterator<Item = String>) -> Result<(), String> {
    let mut file = None;
    let mut json = None;
    let mut opts = AnalyzeOptions::default();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = Some(PathBuf::from(value(&mut it, "--json")?)),
            "--cache-dir" => opts.cache_dir = Some(PathBuf::from(value(&mut it, "--cache-dir")?)),
            "--no-cache" => opts.use_cache = false,
            "--windows" => {
                opts.windows = value(&mut it, "--windows")?
                    .parse()
                    .map_err(|_| "--windows must be a whole number")?
            }
            "--window-seconds" => {
                opts.window_seconds = value(&mut it, "--window-seconds")?
                    .parse()
                    .map_err(|_| "--window-seconds must be a number")?
            }
            _ if file.is_none() && !a.starts_with("--") => file = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}\n{USAGE}")),
        }
    }
    let report = analyze(&input(file)?, &opts).map_err(|e| e.to_string())?;
    print!("{}", report.render());
    if let Some(out) = json {
        write_json(&out, &report)?;
    }
    Ok(())
}
