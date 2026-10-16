//! `junctur ask`: a typed request (7c) from the command line: how it was read (the model, or the
//! keywords without it), the recipe in plain words, and the ghost cuts and keeps it makes of the edit
//! list, to measure the reader and the detectors on a real recording. Writes only the detectors'
//! caches into the project folder, never the app's `ask.json` or the edit list.

use std::sync::atomic::AtomicBool;

use junctur_core::ask;
use junctur_core::llm::{self, Size, Translator};
use junctur_core::pipeline::Project;
use junctur_core::recipe;
use junctur_core::runtime::DevicePref;

use crate::args::Args;
use crate::flags;

pub fn ask(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("ask"))?;
    let p = Project::open(&a.file()?, a.path("--cache-dir"), a.path("--project"))
        .map_err(|e| e.to_string())?;
    let list = p
        .edit()
        .map_err(|e| format!("{e} (run `junctur analyze` on this file first)"))?;
    let request = a
        .value("--prompt")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("say what to do with --prompt \"...\"")?;
    let range = match (a.num::<f64>("--from")?, a.num::<f64>("--to")?) {
        (None, None) => None,
        (from, to) => Some((from.unwrap_or(0.0), to.unwrap_or(list.duration))),
    };
    let speakers = ask::speakers(&list);
    let read = if let Some(json) = a.value("--recipe") {
        let r = recipe::parse(json).map_err(|e| format!("--recipe: {e}"))?;
        llm::Read {
            recipe: Ok(r),
            reading: llm::Reading::by("given"),
        }
    } else if a.has("--no-model") {
        llm::read(None, request, &speakers, Some("--no-model".into()))
    } else {
        let pref = if a.has("--cpu") {
            DevicePref::Cpu
        } else {
            DevicePref::Auto
        };
        let size = match a.value("--model") {
            None | Some("1.7b") => Size::Small,
            Some("4b") => Size::Large,
            Some(other) => return Err(format!("--model is 1.7b or 4b, not {other}")),
        };
        match Translator::open(&p.cache_dir, size, pref, &AtomicBool::new(false)) {
            Ok(mut t) => llm::read(Some(&mut t), request, &speakers, None),
            Err(e) => llm::read(None, request, &speakers, Some(format!("no model: {e}"))),
        }
    };
    let step = ask::propose(&p, &list, request, range, read);
    let how = &step.reading;
    println!("  request: {request}");
    println!(
        "  read by {}{}{}",
        how.reader,
        how.device.map(|d| format!(" on {d:?}")).unwrap_or_default(),
        match (how.load_s, how.answer_s) {
            (Some(l), Some(s)) => format!(" (opened in {l:.1} s, answered in {s:.2} s)"),
            _ => String::new(),
        }
    );
    if let Some(why) = &how.fallback {
        println!("  the model did not read it: {why}");
    }
    for answer in &how.answers {
        println!("  model said: {answer}");
    }
    if let Some(r) = &step.recipe {
        println!("  recipe: {r}");
        println!("  {}", step.described);
    }
    if let Some(e) = &step.error {
        println!("  nothing to do: {e}");
    }
    for n in &step.notes {
        println!("  {n}");
    }
    // `0.0 +`: an empty float sum is -0.0, which prints as "-0".
    let cut = 0.0
        + step
            .ghosts
            .iter()
            .filter(|g| !g.keep)
            .map(|g| g.end - g.start)
            .sum::<f64>();
    println!(
        "  found {} in {:.2} s: {} ghosts ({cut:.1} s cut), {} marks, {} clips",
        step.found,
        step.seconds,
        step.ghosts.len(),
        step.marks.len(),
        step.clips.len()
    );
    for g in step.ghosts.iter().take(40) {
        println!(
            "  {:>9.2} - {:>9.2} s  {}",
            g.start,
            g.end,
            if g.keep { "keep" } else { "cut" }
        );
    }
    if let Some(out) = a.path("--json") {
        let json = serde_json::to_vec_pretty(&step).map_err(|e| e.to_string())?;
        std::fs::write(&out, json).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(())
}
