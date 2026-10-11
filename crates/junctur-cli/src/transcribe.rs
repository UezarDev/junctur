//! `junctur transcribe`: Whisper over the speech tracks.
//!
//! The model comes from `--model`, else the config file's `whisper_model`, else large-v3-turbo.

use std::path::PathBuf;

use junctur_core::transcribe::Progress;
use junctur_core::whisper::Model;
use junctur_core::{config, transcribe, TranscribeOptions};

use crate::{input, value, write_json, USAGE};

pub fn run(mut it: impl Iterator<Item = String>) -> Result<(), String> {
    let cfg = config::load(&config::default_path()).map_err(|e| e.to_string())?;
    let mut file = None;
    let mut json = None;
    let mut model: Option<Model> = None;
    let mut opts = TranscribeOptions {
        models_dir: cfg.models_dir,
        ..TranscribeOptions::default()
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--model" => model = Some(value(&mut it, "--model")?.parse()?),
            "--language" => opts.language = value(&mut it, "--language")?,
            "--json" => json = Some(PathBuf::from(value(&mut it, "--json")?)),
            "--cache-dir" => opts.cache_dir = Some(PathBuf::from(value(&mut it, "--cache-dir")?)),
            "--no-cache" => opts.use_cache = false,
            _ if file.is_none() && !a.starts_with("--") => file = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}\n{USAGE}")),
        }
    }
    opts.model = model.or(cfg.whisper_model).unwrap_or_default();

    let t = transcribe(&input(file)?, &opts, report).map_err(|e| e.to_string())?;
    print!("{}", t.render());
    if let Some(out) = json {
        write_json(&out, &t)?;
    }
    Ok(())
}

fn report(p: Progress) {
    match p {
        Progress::Scanning { track } => eprintln!("track {}: finding speech", track + 1),
        Progress::Chunks { track, done, total } => {
            eprintln!("track {}: {done}/{total} chunks transcribed", track + 1)
        }
    }
}
