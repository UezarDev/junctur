//! `junctur voices`: the remembered voices listed, enrolled from stretches given by hand, or
//! forgotten. `--rehear` is the app's "Analyse speakers again", cut at the pins.

use junctur_core::pipeline::{self, Found, Project};
use junctur_core::voices::{self, Voice, Voices};

use crate::args::Args;

/// The voices file this environment points at ([`voices::default_path`]).
pub fn path() -> Result<std::path::PathBuf, String> {
    voices::default_path()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no config folder to keep voices in (set JUNCTUR_VOICES)".into())
}

/// What the speakers are now (`describe`: a line on how they were found), and the edit list
/// updated with them.
fn print_speakers(p: &Project, found: &Found, describe: bool) -> Result<(), String> {
    if describe {
        println!("{}", pipeline::describe(found));
    }
    if let Some((e, backup)) = pipeline::respeak(p, found).map_err(|e| e.to_string())? {
        let names: Vec<&str> = e.speakers.names.iter().map(|n| n.name.as_str()).collect();
        println!("speakers: {}", names.join(", "));
        if let Some(b) = backup {
            println!("the previous edit list was kept as {}", b.display());
        }
    }
    Ok(())
}

/// `<start>-<end>[,<start>-<end>...]` in seconds.
fn stretches(s: &str) -> Result<Vec<(f64, f64)>, String> {
    s.split(',')
        .map(|part| {
            let bad = || format!("--at: {part:?} is not <start>-<end> in seconds");
            let (a, b) = part.trim().split_once('-').ok_or_else(bad)?;
            let (a, b): (f64, f64) = (
                a.trim().parse().map_err(|_| bad())?,
                b.trim().parse().map_err(|_| bad())?,
            );
            (a.is_finite() && b > a).then_some((a, b)).ok_or_else(bad)
        })
        .collect()
}

pub fn run(a: Args) -> Result<(), String> {
    a.reject_unknown(&[
        "--add",
        "--at",
        "--forget",
        "--rehear",
        // Speakers always run on the CPU; still accepted so older scripts keep working.
        "--cpu",
        "--project",
        "--cache-dir",
    ])?;
    let file = path()?;
    let mut v = Voices::load(&file).map_err(|e| e.to_string())?;
    if let Some(name) = a.value("--forget") {
        if !v.forget(name) {
            return Err(format!("no voice is called {name}"));
        }
        v.save(&file).map_err(|e| e.to_string())?;
        println!("forgot {name}; run `junctur analyze` (or `cut`) on a recording to relabel it");
    } else if let Some(name) = a.value("--add") {
        let at = a
            .value("--at")
            .ok_or("--add needs --at <start>-<end>[,...]")?;
        let p = Project::open(&a.file()?, a.path("--cache-dir"), a.path("--project"))
            .map_err(|e| e.to_string())?;
        let (found, added) =
            pipeline::enroll(&p, &mut v, name, &stretches(at)?).map_err(|e| e.to_string())?;
        // The voices first: labels are only saved once the voices they stand on are.
        v.save(&file).map_err(|e| e.to_string())?;
        pipeline::save_speakers(&p, &found).map_err(|e| e.to_string())?;
        let voice = v.get(name).expect("just enrolled");
        println!(
            "remembered {} from {added:.1} s ({:.1} s of speech in all; {}); those seconds are pinned to them in this recording",
            voice.name,
            voice.seconds,
            file.display()
        );
        if voice.seconds < voices::SHORT_SECONDS {
            println!(
                "a very short clip: other recordings may not recognise {} until more is added",
                voice.name
            );
        }
        print_speakers(&p, &found, true)?;
        return Ok(());
    } else if a.has("--rehear") {
        let p = Project::open(&a.file()?, a.path("--cache-dir"), a.path("--project"))
            .map_err(|e| e.to_string())?;
        let usable: Vec<Voice> = v.usable().into_iter().cloned().collect();
        // The stage's log already ends with what it found.
        let found =
            pipeline::rehear(&p, &usable, &mut |l| println!("{l}")).map_err(|e| e.to_string())?;
        print_speakers(&p, &found, false)?;
        return Ok(());
    }
    if v.voices.is_empty() {
        println!("no voices remembered ({})", file.display());
    }
    for voice in &v.voices {
        println!("{} {} ({:.1} s)", voice.color, voice.name, voice.seconds);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretches_parse_and_bad_ones_are_errors() {
        assert_eq!(
            stretches("1-2.5, 10-12").unwrap(),
            vec![(1.0, 2.5), (10.0, 12.0)]
        );
        assert!(stretches("2-1").is_err());
        assert!(stretches("abc").is_err());
        assert!(stretches("1-").is_err());
    }
}
