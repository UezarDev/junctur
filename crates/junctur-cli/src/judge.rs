//! `junctur judge`: smart keep's stage alone on an analysed recording, with its prompt or a new one:
//! the rules write the busy silent stretches to keep (phase 3d). 3b's `--agreement`, `--cpu` and `--force`
//! went with Laya: the rules have no margin to measure, no model and nothing reused.

use junctur_core::config;
use junctur_core::keep::{self, rules};
use junctur_core::pipeline::{self, Project};

use crate::args::Args;
use crate::flags;

pub fn judge(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("judge"))?;
    let p = Project::open(&a.file()?, a.path("--cache-dir"), a.path("--project"))
        .map_err(|e| e.to_string())?;
    let mut opts = config::analysis_options(None, true, true).map_err(|e| e.to_string())?;
    opts.keep.prompt = a.value("--prompt").map(str::to_string);
    if let Some(prompt) = &opts.keep.prompt {
        remember(prompt);
    }
    let judged = pipeline::judge(&p, &opts, &mut |line| eprintln!("junctur: {line}"))
        .map_err(|e| e.to_string())?
        .ok_or("smart keep is off for this recording: give it a prompt with --prompt")?;
    println!("  {}", p.dir.join(keep::FILE).display());
    println!(
        "  read as {}; {} stretches to keep",
        rules::Strictness::read(&judged.prompt).describe(),
        judged.parts.len()
    );
    Ok(())
}

/// Keeps a prompt given on the command line as the last one, as the app does; a config file that
/// cannot be written is said, not a failure.
pub fn remember(prompt: &str) {
    if let Err(e) = config::remember_prompt(prompt) {
        eprintln!("junctur: the prompt was not remembered as the last one ({e})");
    }
}
