//! `junctur judge`: Laya's keep stage alone on an analysed recording, with its prompt or a new one, and
//! with `--agreement` how often it agrees with the edit list as it is (the phase's first test).

use junctur_core::config;
use junctur_core::keep::{self, Score};
use junctur_core::pipeline::{self, Project};
use junctur_core::runtime::DevicePref;

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
    if a.has("--cpu") {
        opts.device = DevicePref::Cpu;
    }
    opts.force = a.has("--force");
    let judged = pipeline::judge(&p, &opts, &mut |line| eprintln!("junctur: {line}"))
        .map_err(|e| e.to_string())?
        .ok_or("smart keep is off for this recording: give it a prompt with --prompt")?;
    println!("  {}", p.dir.join(keep::FILE).display());
    if a.has("--agreement") {
        let list = p.edit().map_err(|e| e.to_string())?;
        let auto = pipeline::automatic_keep(&p, &list)
            .ok_or("run `junctur analyze` on this file first")?;
        print_agreement(&keep::agreement(&judged, &list.keep, &auto));
    }
    Ok(())
}

/// Keeps a prompt given on the command line as the last one, as the app does; a config file that
/// cannot be written is said, not a failure.
pub fn remember(prompt: &str) {
    if let Err(e) = config::remember_prompt(prompt) {
        eprintln!("junctur: the prompt was not remembered as the last one ({e})");
    }
}

fn print_agreement(a: &keep::Agreement) {
    println!("agreement with the edit list (a part's answer: kept when half of it is)");
    println!(
        "  {:<7} {:>5} {:>5} | {:>6} {:>6} | held out {:>4}: {:>6} {:>6} {:>7}   fitted on the other half",
        "", "parts", "kept", "always", "Laya", "", "always", "Laya", "fitted"
    );
    for (name, s) in [("all", &a.all), ("talk", &a.talk), ("silent", &a.silent)] {
        println!("  {}", row(name, s));
    }
    println!(
        "  parts the list does otherwise than the automatic cut (hand edits): {}, Laya at 0.5 agrees with {}",
        a.hand, a.hand_agree
    );
}

fn row(name: &str, s: &Score) -> String {
    let pc = |x: f64| format!("{:.1}%", x * 100.0);
    format!(
        "{name:<7} {:>5} {:>5} | {:>6} {:>6} | held out {:>4}: {:>6} {:>6} {:>7}   cutoff {:.3}, temperature {:.2}, {} pairs",
        s.parts,
        s.kept,
        pc(s.always_keep),
        pc(s.laya),
        s.held_out,
        pc(s.always_keep_held),
        pc(s.laya_held),
        pc(s.fitted_held),
        s.fit.cutoff,
        s.fit.temperature,
        s.fit.pairs
    )
}
