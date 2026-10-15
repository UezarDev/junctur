//! The commands: turn parsed arguments into library options, run, print.

use std::path::{Path, PathBuf};

use junctur_core::ffmpeg::Span;
use junctur_core::output;
use junctur_core::runtime::DevicePref;
use junctur_core::tracks::Role;
use junctur_core::{analyze, separate as run_separate, AnalyzeOptions, SeparateOptions};
use serde::Serialize;

use crate::args::Args;
use crate::flags;

pub fn tracks(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("tracks"))?;
    let mut opts = AnalyzeOptions {
        cache_dir: a.path("--cache-dir"),
        use_cache: !a.has("--no-cache"),
        speech: !a.has("--no-speech"),
        ..Default::default()
    };
    if let Some(n) = a.num("--windows")? {
        opts.windows = n;
    }
    if let Some(s) = a.num("--window-seconds")? {
        opts.window_seconds = s;
    }
    let file = a.file()?;
    let json = json_out(&a, &file)?;
    let report = analyze(&file, &opts).map_err(|e| e.to_string())?;
    print!("{}", report.render());
    write_json(json.as_deref(), &report)
}

pub fn separate(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("separate"))?;
    let file = a.file()?;
    let json = json_out(&a, &file)?;
    let cache_dir = a.path("--cache-dir");
    let track = match a.num::<usize>("--track")? {
        Some(0) => return Err("--track is 1-based".into()),
        Some(n) => n - 1,
        None => mix_track(&file, cache_dir.clone())?,
    };
    let opts = SeparateOptions {
        cache_dir,
        device: if a.has("--cpu") {
            DevicePref::Cpu
        } else {
            DevicePref::Auto
        },
        span: Span {
            start: a.num("--start")?.unwrap_or(0.0),
            len: a.num("--duration")?,
        },
        force: a.has("--force"),
        no_speakers: a.has("--no-speakers"),
    };
    let stems = run_separate(&file, track, &opts).map_err(|e| e.to_string())?;
    print!("{}", stems.render());
    write_json(json.as_deref(), &stems)
}

/// Without `--track`, separate the track the analysis called the mix.
fn mix_track(file: &Path, cache_dir: Option<PathBuf>) -> Result<usize, String> {
    let opts = AnalyzeOptions {
        cache_dir,
        ..Default::default()
    };
    let report = analyze(file, &opts).map_err(|e| e.to_string())?;
    report
        .logical
        .iter()
        .find(|l| l.role == Role::Mix)
        .map(|l| l.members[0])
        .ok_or_else(|| "no track was identified as a mix; pick one with --track <n>".into())
}

/// `--json`, refused up front when it names the recording: the analysis can take minutes, and
/// writing over the source after it would destroy the input.
fn json_out(a: &Args, file: &Path) -> Result<Option<PathBuf>, String> {
    let out = a.path("--json");
    if let Some(out) = &out {
        output::refuse_source(file, out).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn write_json<T: Serialize>(out: Option<&Path>, value: &T) -> Result<(), String> {
    if let Some(out) = out {
        let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
        std::fs::write(out, body).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(())
}
