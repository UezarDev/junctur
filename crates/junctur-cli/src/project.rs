//! The pipeline commands: `analyze`, `status`, `cut`, `export`, `render`. They share one project per
//! recording (`--project <dir>` to keep it somewhere git can track it).

use junctur_core::edit::{Captions, CutSettings, EditList};
use junctur_core::pipeline::{self, Project, StageName, StageState};
use junctur_core::render::{self, RenderOptions};
use junctur_core::runtime::DevicePref;
use junctur_core::transcribe;
use junctur_core::{config, export, voices};

use crate::args::Args;
use crate::flags;

fn project(a: &Args) -> Result<Project, String> {
    Project::open(&a.file()?, a.path("--cache-dir"), a.path("--project")).map_err(|e| e.to_string())
}

/// The caption settings `e`'s cues are cut with: its own, else the config file's defaults, as
/// the app shows them, so a render and an export cut the cues the app would. The config file is
/// read only for a list without its own.
fn captions(e: &EditList) -> Result<Captions, String> {
    config::captions(e.captions.clone(), config::load).map_err(|e| e.to_string())
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

pub fn analyze(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("analyze"))?;
    let p = project(&a)?;
    // The model (`--model`, else the config file's `whisper_model`, else `auto`), as the app's
    // analysis takes it too; then this command's own flags.
    let model = a.path("--model").map(|m| m.to_string_lossy().into_owned());
    let no_speakers = a.has("--no-speakers");
    let mut opts = config::analysis_options(model.as_deref(), a.has("--no-transcript"), true)
        .map_err(|e| e.to_string())?;
    // The voices are read here rather than by `analysis_options`: read only when speakers are told
    // apart, and a broken voices file is said and skipped instead of stopping the analysis.
    opts.no_speakers = no_speakers;
    if !no_speakers {
        opts.voices = voices::remembered_or_none(&mut |m| eprintln!("junctur: {m}"));
    }
    if a.has("--cpu") {
        opts.device = DevicePref::Cpu;
    }
    opts.cut = cut_settings(&a)?;
    opts.force = a.has("--force");
    // Labels for the silent parts this run, with the config file's kind and added labels, as when
    // the file turns them on.
    if a.has("--labels") {
        opts.labels.on = true;
    }
    // Smart keep reads this run with this prompt, kept as the recording's and as the last one.
    opts.keep.prompt = a.value("--keep").map(str::to_string);
    if let Some(prompt) = &opts.keep.prompt {
        crate::judge::remember(prompt);
    }
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
    a.reject_unknown(&flags::known("status"))?;
    let p = project(&a)?;
    let jobs = p.jobs();
    println!("{}", p.file.display());
    println!("project: {}", p.dir.display());
    for stage in StageName::ALL {
        let name = stage.name();
        let r = jobs.get(stage);
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
    a.reject_unknown(&flags::known("cut"))?;
    let p = project(&a)?;
    // The voices may have changed since `analyze`: label the speakers again first (cheap). A
    // project heard by an older Junctur cannot be; its speakers stay as they were.
    if let Some(f) = p.speakers() {
        if f.heard_by_an_older_junctur() {
            eprintln!("junctur: speakers were found by an older Junctur: run `junctur analyze` to hear them again");
        }
        let voices = voices::remembered_or_none(&mut |m| eprintln!("junctur: {m}"));
        if let Err(e) = pipeline::relabel(&p, &voices) {
            eprintln!("junctur: speakers not relabelled: {e}");
        }
    }
    let e = pipeline::recut(&p, &cut_settings(&a)?).map_err(|e| e.to_string())?;
    print_edit(&p, &e);
    Ok(())
}

pub fn export(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("export"))?;
    let (fcpxml, srt) = (a.path("--fcpxml"), a.path("--srt"));
    if fcpxml.is_none() && srt.is_none() {
        return Err("export needs --fcpxml <out.fcpxml> and/or --srt <out.srt>".into());
    }
    let p = project(&a)?;
    let e = p.edit().map_err(|e| e.to_string())?;
    if let Some(out) = fcpxml {
        export::write_fcpxml(&e, &out).map_err(|e| e.to_string())?;
        println!("{} ({} clips)", out.display(), e.keep.len());
    }
    if let Some(out) = srt {
        // Only the captions need the config file, so a broken one never blocks an FCPXML.
        write_captions(&p, &e, &captions(&e)?, &out)?;
    }
    Ok(())
}

/// The caption file for `e` at `out`, from the project's transcript.
fn write_captions(
    p: &Project,
    e: &EditList,
    c: &Captions,
    out: &std::path::Path,
) -> Result<(), String> {
    let t = transcribe::load(&p.dir)
        .ok_or("captions need a transcript: run `junctur analyze` on this file first")?;
    let tr = translations(p, e, c, &t)?;
    let files = export::write_srt(e, c, &t, &tr, out).map_err(|e| e.to_string())?;
    print_captions(out, &files);
    Ok(())
}

/// The translations captions set to be translated need, made first where missing, with a
/// progress line; the model is fetched on first use.
fn translations(
    p: &Project,
    e: &EditList,
    c: &Captions,
    t: &transcribe::Transcript,
) -> Result<junctur_core::translate::Translations, String> {
    let mut last = None;
    let (tr, done) =
        export::translate_captions(e, c, t, &p.dir, &p.cache_dir, &mut |done, todo| {
            // A line per tenth, not per sentence.
            let tenth = (done * 10).checked_div(todo);
            if todo > 0 && tenth != last {
                last = tenth;
                eprintln!("junctur: translating captions: {done} of {todo} sentences");
            }
        })
        .map_err(|e| e.to_string())?;
    if done.unknown > 0 {
        eprintln!(
            "junctur: {} caption sentences have no transcript language to translate from",
            done.unknown
        );
    }
    if done.failed > 0 {
        eprintln!(
            "junctur: the model gave no usable translation of {} caption sentences",
            done.failed
        );
    }
    Ok(tr)
}

fn print_captions(out: &std::path::Path, files: &export::CaptionFiles) {
    println!("{} ({} captions)", out.display(), files.cues);
    for path in &files.stale {
        println!(
            "{} is a speaker file from an earlier export: delete it before importing",
            path.display()
        );
    }
}

pub fn render(a: Args) -> Result<(), String> {
    a.reject_unknown(&flags::known("render"))?;
    let out = a.path("--out").ok_or("render needs --out <out.mp4>")?;
    let p = project(&a)?;
    let e = p.edit().map_err(|e| e.to_string())?;
    let c = captions(&e)?;
    // With captions on, what the caption file needs is checked before an encode that can take an
    // hour, not after it.
    let captions = if c.on {
        let srt = out.with_extension("srt");
        junctur_core::output::refuse_source(&e.source, &srt).map_err(|e| e.to_string())?;
        let t = transcribe::load(&p.dir)
            .ok_or("captions are on but there is no transcript: run `junctur analyze` first")?;
        let tr = translations(&p, &e, &c, &t)?;
        Some((srt, t, tr))
    } else {
        None
    };
    let opts = RenderOptions {
        full: a.has("--full"),
        overwrite: a.has("--force"),
        jobs: a.num("--jobs")?.unwrap_or(RenderOptions::default().jobs),
        cache_dir: Some(p.cache_dir.clone()),
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
    // Captions are an optional toggle in the edit list; on, they go beside the video.
    if let Some((srt, t, tr)) = captions {
        let files = export::write_srt(&e, &c, &t, &tr, &srt).map_err(|e| e.to_string())?;
        print_captions(&srt, &files);
    }
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
    if !e.speakers.names.is_empty() {
        let names: Vec<&str> = e.speakers.names.iter().map(|n| n.name.as_str()).collect();
        println!(
            "speakers: {} ({} turns; rename them in the edit list's `speaker` lines)",
            names.join(", "),
            e.speakers.turns.len()
        );
    }
    println!("  {}", p.edit_path().display());
}
