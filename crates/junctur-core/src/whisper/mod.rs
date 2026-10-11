//! Whisper transcription through whisper.cpp's `whisper-cli`, found on PATH like ffmpeg.
//!
//! One run takes many chunk files, so the model loads once per batch rather than once per chunk.

mod model;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tool::run;
use crate::{Error, Result};

pub use model::{default_models_dir, Model};

pub const PROGRAM: &str = "whisper-cli";

/// One line of text, with times relative to the start of the file Whisper heard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub model_file: PathBuf,
    /// A Whisper language code, or "auto" to detect it per chunk.
    pub language: String,
    pub threads: usize,
}

/// Transcribe each WAV in `wavs`; the result lines up with the input.
pub fn transcribe(settings: &Settings, wavs: &[PathBuf]) -> Result<Vec<Vec<Segment>>> {
    if wavs.is_empty() {
        return Ok(Vec::new());
    }
    check_model(&settings.model_file)?;
    let args = args(settings, wavs);
    run(
        PROGRAM,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    wavs.iter()
        .map(|w| {
            let out = json_path(w);
            let bytes = std::fs::read(&out)
                .map_err(|e| Error::Whisper(format!("{}: {e}", out.display())))?;
            let _ = std::fs::remove_file(&out);
            parse(&bytes)
        })
        .collect()
}

/// Fail early, before hours of scanning, when the model file is missing.
pub fn check_model(model_file: &Path) -> Result<()> {
    if model_file.is_file() {
        Ok(())
    } else {
        Err(Error::Model(model_file.to_path_buf()))
    }
}

fn args(s: &Settings, wavs: &[PathBuf]) -> Vec<String> {
    let mut a: Vec<String> = [
        "-m",
        &s.model_file.to_string_lossy(),
        "-l",
        &s.language,
        "-t",
        &s.threads.max(1).to_string(),
        // No text carried between windows: the main guard against turbo repeating a line.
        "-mc",
        "0",
        "-sns",
        "-np",
        "-oj",
    ]
    .iter()
    .map(|x| x.to_string())
    .collect();
    for w in wavs {
        a.push("-f".into());
        a.push(w.to_string_lossy().into_owned());
    }
    a
}

/// whisper-cli writes `<input>.json` beside each input.
fn json_path(wav: &Path) -> PathBuf {
    let mut p = wav.as_os_str().to_owned();
    p.push(".json");
    PathBuf::from(p)
}

#[derive(Deserialize)]
struct Output {
    transcription: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    offsets: Offsets,
    text: String,
}

#[derive(Deserialize)]
struct Offsets {
    from: i64,
    to: i64,
}

fn parse(bytes: &[u8]) -> Result<Vec<Segment>> {
    // A token can end mid-character, so the file is not always valid UTF-8.
    let text = String::from_utf8_lossy(bytes);
    let out: Output =
        serde_json::from_str(&text).map_err(|e| Error::Whisper(format!("bad output: {e}")))?;
    Ok(out
        .transcription
        .into_iter()
        .map(|i| Segment {
            start: i.offsets.from as f64 / 1000.0,
            end: i.offsets.to as f64 / 1000.0,
            text: i.text.trim().to_string(),
        })
        .filter(|s| !s.text.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whisper_cli_json() {
        let json = br#"{"systeminfo":"x","model":{},"params":{},"result":{"language":"en"},
            "transcription":[
              {"timestamps":{"from":"00:00:00,000","to":"00:00:02,500"},
               "offsets":{"from":0,"to":2500},"text":" Hello there."},
              {"timestamps":{"from":"00:00:02,500","to":"00:00:03,000"},
               "offsets":{"from":2500,"to":3000},"text":"   "}]}"#;
        let s = parse(json).unwrap();
        assert_eq!(
            s,
            vec![Segment {
                start: 0.0,
                end: 2.5,
                text: "Hello there.".into()
            }]
        );
    }

    #[test]
    fn broken_utf8_does_not_fail_the_chunk() {
        let mut json = br#"{"transcription":[{"offsets":{"from":0,"to":10},"text":"a"#.to_vec();
        json.push(0xE2);
        json.extend_from_slice(br#""}]}"#);
        assert_eq!(parse(&json).unwrap()[0].text, "a\u{FFFD}");
    }

    #[test]
    fn args_reset_context_and_list_every_chunk() {
        let s = Settings {
            model_file: "m.bin".into(),
            language: "auto".into(),
            threads: 8,
        };
        let a = args(&s, &["a.wav".into(), "b.wav".into()]);
        let joined = a.join(" ");
        assert!(
            joined.starts_with("-m m.bin -l auto -t 8 -mc 0"),
            "{joined}"
        );
        assert!(joined.ends_with("-f a.wav -f b.wav"), "{joined}");
        assert_eq!(json_path(Path::new("a.wav")), PathBuf::from("a.wav.json"));
    }

    #[test]
    fn missing_model_is_reported_before_running() {
        let s = Settings {
            model_file: "/no/such/ggml-large-v3-turbo.bin".into(),
            language: "auto".into(),
            threads: 1,
        };
        assert!(matches!(
            transcribe(&s, &["a.wav".into()]),
            Err(Error::Model(_))
        ));
    }
}
