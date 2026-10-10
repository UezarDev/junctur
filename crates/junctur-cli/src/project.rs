//! The pipeline commands: `analyze`, `status`, `cut`, `export`, `render`. They share one project per
//! recording (`--project <dir>` to keep it somewhere git can track it).

use junctur_core::edit::{CutSettings, EditList};
use junctur_core::export;
use junctur_core::pipeline::{self, PipelineOptions, Project, StageState, STAGES};
use junctur_core::render::{self, RenderOptions};
use junctur_core::runtime::DevicePref;
use junctur_core::transcribe::ModelPref;

use crate::args::Args;

const PROJECT_FLAGS: &[&str] = &["--project", "--cache-dir"];
const CUT_FLAGS: &[&str] = &["--pad-before", "--pad-after", "--merge-gap", "--crossfade"];

fn project(a: &Args) -> Result<Project, String> {
    Project::open(&a.file()?, a.path("--cache-dir"), a.path("--project")).map_err(|e| e.to_string())
}

fn cut_settings(a: &Args) -> Result<CutSettings, String> {
    let mut s = CutSettings::default();
    if let Some(v) = a.num("--pad-before")? {
        s.pad_before = v;
    }
    if let Some(v) = a.num("--pad-after")? {
        s.pad_after = v;
    }
    if let Some(v) = a.num("--merge-gap")? {
        s.merge_gap = v;
    }
    if let Some(ms) = a.num::<f64>("--crossfade")? {
        s.crossfade = ms / 1000.0;
    }
    Ok(s)
}

fn known(extra: &[&'static str]) -> Vec<&'static str> {
    PROJECT_FLAGS.iter().chain(extra).copied().collect()
}

pub fn analyze(a: Args) -> Result<(), String> {
    let mut k = known(&["--cpu", "--model", "--force", "--no-transcript"]);
    k.extend(CUT_FLAGS);
    a.reject_unknown(&k)?;
    let p = project(&a)?;
    let opts = PipelineOptions {
        device: if a.has("--cpu") {
            DevicePref::Cpu
        } else {
            DevicePref::Auto
        },
        model: match a.path("--model").as_deref().and_then(|m| m.to_str()) {
            None | Some("auto") => ModelPref::Auto,
            Some("large-v3") => ModelPref::LargeV3,
            Some("large-v3-q5_0") => ModelPref::LargeV3Q5,
            Some(m) => {
                return Err(format!(
                    "--model: unknown model {m} (auto, large-v3, large-v3-q5_0)"
                ))
            }
        },
        cut: cut_settings(&a)?,
        force: a.has("--force"),
        no_transcript: a.has("--no-transcript"),
        ..Default::default()
    };
    let out = pipeline::run(&p, &opts, &mut |line| eprintln!("junctur: {line}"))
        .map_err(|e| e.to_string())?;
    if let Some(t) = &out.transcript {
        println!(
            "transcript: {} chunks, {:.0} s of speech audio, {} on {:?} ({:.0} s this run{})",
            t.segments.len(),
            t.audio_s,
            t.choice.model,
            t.choice.device,
            t.elapsed_s,
            if t.resumed_chunks > 0 {
                format!(", {} chunks resumed", t.resumed_chunks)
            } else {
                String::new()
            }
        );
        println!("  {}", p.dir.join("transcript.txt").display());
    }
    print_edit(&p, &out.edit);
    Ok(())
}

pub fn status(a: Args) -> Result<(), String> {
    a.reject_unknown(PROJECT_FLAGS)?;
    let p = project(&a)?;
    let jobs = p.jobs();
    println!("{}", p.file.display());
    println!("project: {}", p.dir.display());
    for name in STAGES {
        let r = jobs.get(name);
        let state = match r.map(|r| r.state) {
            None | Some(StageState::Pending) => "pending",
            Some(StageState::Running) => "running (or interrupted: rerun analyze to resume)",
            Some(StageState::Done) => "done",
            Some(StageState::Failed) => "failed",
        };
        let secs = r
            .and_then(|r| r.seconds)
            .map(|s| format!(" in {s:.1} s"))
            .unwrap_or_default();
        let note = r
            .and_then(|r| r.note.clone())
            .map(|n| format!(" - {n}"))
            .unwrap_or_default();
        println!("  {name:<11} {state}{secs}{note}");
    }
    if let Ok(e) = p.edit() {
        print_edit(&p, &e);
    }
    Ok(())
}

pub fn cut(a: Args) -> Result<(), String> {
    let mut k = known(&[]);
    k.extend(CUT_FLAGS);
    a.reject_unknown(&k)?;
    let p = project(&a)?;
    let e = pipeline::recut(&p, &cut_settings(&a)?).map_err(|e| e.to_string())?;
    print_edit(&p, &e);
    Ok(())
}

pub fn export(a: Args) -> Result<(), String> {
    a.reject_unknown(&known(&["--fcpxml"]))?;
    let out = a
        .path("--fcpxml")
        .ok_or("export needs --fcpxml <out.fcpxml>")?;
    let p = project(&a)?;
    let e = p.edit().map_err(|e| e.to_string())?;
    export::write_fcpxml(&e, &out).map_err(|e| e.to_string())?;
    println!("{} ({} clips)", out.display(), e.keep.len());
    Ok(())
}

pub fn render(a: Args) -> Result<(), String> {
    a.reject_unknown(&known(&["--out", "--full", "--force", "--jobs"]))?;
    let out = a.path("--out").ok_or("render needs --out <out.mp4>")?;
    let p = project(&a)?;
    let e = p.edit().map_err(|e| e.to_string())?;
    let opts = RenderOptions {
        full: a.has("--full"),
        overwrite: a.has("--force"),
        jobs: a.num("--jobs")?.unwrap_or(RenderOptions::default().jobs),
    };
    let r = render::render(&e, &out, &opts, &mut |line| eprintln!("junctur: {line}"))
        .map_err(|e| e.to_string())?;
    let total = (r.frames_copied + r.frames_encoded).max(1);
    match &r.full_reason {
        Some(why) => println!("full re-encode ({why})"),
        None => println!(
            "smart render: {:.0}% of frames copied, {} encoded, in {} pieces",
            r.frames_copied as f64 / total as f64 * 100.0,
            r.frames_encoded,
            r.pieces
        ),
    }
    println!(
        "{} ({:.1} s, {} audio tracks) in {:.1} s",
        out.display(),
        r.duration,
        r.audio_tracks,
        r.seconds
    );
    Ok(())
}

fn print_edit(p: &Project, e: &EditList) {
    let pct = if e.duration > 0.0 {
        e.kept() / e.duration * 100.0
    } else {
        0.0
    };
    println!(
        "edit list: keeps {:.0} s of {:.0} s ({pct:.0}%) in {} ranges",
        e.kept(),
        e.duration,
        e.keep.len()
    );
    println!("  {}", p.edit_path().display());
}
