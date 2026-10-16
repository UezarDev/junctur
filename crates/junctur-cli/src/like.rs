//! `junctur like`: "find all like this" (7b) from the command line, on the moments given with `--at`:
//! the matches above `--threshold` and how long the search took, to measure the matcher on a real
//! recording (every peak, with `--json`). Writes only the track's spectrogram into the project folder (built once), never the
//! app's `like.json`.

use std::time::Instant;

use junctur_core::labels::busy;
use junctur_core::like::{self, mel, Example};
use junctur_core::pipeline::{self, Project};

use crate::args::Args;
use crate::flags;

pub fn like(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("like"))?;
    let p = Project::open(&a.file()?, a.path("--cache-dir"), a.path("--project"))
        .map_err(|e| e.to_string())?;
    let times = |flag: &str| -> Result<Vec<f64>, String> {
        a.value(flag)
            .map(|v| {
                v.split(',')
                    .map(|t| {
                        t.trim()
                            .parse()
                            .map_err(|_| format!("{flag}: not seconds: {t}"))
                    })
                    .collect()
            })
            .unwrap_or(Ok(Vec::new()))
    };
    let len = a.num::<f64>("--len")?.unwrap_or(like::CLICK_S);
    let example = |at: f64, positive| Example::span(at, at + len, positive);
    let mut examples: Vec<Example> = times("--at")?
        .into_iter()
        .map(|t| example(t, true))
        .collect();
    examples.extend(times("--not")?.into_iter().map(|t| example(t, false)));
    if !examples.iter().any(|e| e.positive) {
        return Err("give the moment to look for with --at <seconds>".into());
    }
    let report = pipeline::cached_tracks(&p);
    let track = match a.num::<usize>("--track")? {
        Some(n) if n >= 1 => n - 1,
        Some(_) => return Err("--track counts from 1".into()),
        None => report.as_ref().and_then(busy::track).map_or(0, |t| t.track),
    };
    let duration = report.as_ref().map_or(0.0, |r| r.duration);
    let t0 = Instant::now();
    let (spec, built) =
        mel::of_track(&p.dir, &p.file, track, duration).map_err(|e| e.to_string())?;
    let read = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let found = like::find(&spec, &examples, None).map_err(|e| e.to_string())?;
    let searched = t1.elapsed().as_secs_f64();
    let threshold = a.num::<f64>("--threshold")?.unwrap_or(like::THRESHOLD);
    let shown: Vec<_> = found.iter().filter(|m| m.score >= threshold).collect();
    println!(
        "  track {}: {:.0} s of sound, spectrogram {} in {read:.1} s",
        track + 1,
        spec.seconds(),
        if built { "built" } else { "read" }
    );
    println!(
        "  searched in {searched:.2} s: {} peaks, {} at or above {threshold}",
        found.len(),
        shown.len()
    );
    for m in shown.iter().take(40) {
        println!("  {:>9.2} s  {:.3}", m.point, m.score);
    }
    if let Some(out) = a.path("--json") {
        let json = serde_json::to_vec_pretty(&found).map_err(|e| e.to_string())?;
        std::fs::write(&out, json).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(())
}
