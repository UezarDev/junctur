//! The only place that runs ffmpeg/ffprobe.
//!
//! They are external programs found on PATH (a DECISIONS-level choice for now), so every call goes
//! through here: one spot to swap for bundled binaries or a native decoder later.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::tool::run;
use crate::{Error, Result};

/// Sample rate the analysis decodes at. 16 kHz mono keeps eight windows of four tracks small while
/// keeping speech and most game audio energy; it is also what the VAD/Whisper stages will want.
pub const ANALYSIS_RATE: u32 = 16_000;

/// One audio stream as ffprobe reports it. `index` is the audio ordinal (0 = first audio track),
/// which is what `-map 0:a:N` takes; the report shows it 1-based as OBS does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStream {
    pub index: usize,
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub duration: Option<f64>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub duration: f64,
    pub audio: Vec<AudioStream>,
}

pub fn probe(file: &Path) -> Result<Probe> {
    let path = file.to_string_lossy();
    let out = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-select_streams",
            "a",
            &path,
        ],
    )?;
    parse_probe(&out)
}

fn parse_probe(json: &[u8]) -> Result<Probe> {
    let v: serde_json::Value = serde_json::from_slice(json)?;
    let num = |x: &serde_json::Value| -> Option<f64> {
        x.as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| x.as_f64())
    };
    let duration =
        num(&v["format"]["duration"]).ok_or_else(|| Error::Probe("no format duration".into()))?;
    let audio = v["streams"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, s)| AudioStream {
            index: i,
            codec: s["codec_name"].as_str().unwrap_or("unknown").to_string(),
            channels: s["channels"].as_u64().unwrap_or(0) as u32,
            sample_rate: num(&s["sample_rate"]).unwrap_or(0.0) as u32,
            duration: num(&s["duration"]),
            title: s["tags"]["title"].as_str().map(str::to_string),
        })
        .collect();
    Ok(Probe { duration, audio })
}

/// Decode `len` seconds of audio track `track` starting at `start`, as mono f32 at
/// [`ANALYSIS_RATE`]. `-ss` before `-i` seeks the demuxer, so a window deep into a 30h file costs
/// about the same as one at the start.
pub fn decode_window(file: &Path, track: usize, start: f64, len: f64) -> Result<Vec<f32>> {
    let path = file.to_string_lossy();
    let (ss, t, map, rate) = (
        format!("{start:.3}"),
        format!("{len:.3}"),
        format!("0:a:{track}"),
        ANALYSIS_RATE.to_string(),
    );
    let out = run(
        "ffmpeg",
        &[
            "-v", "error", "-nostdin", "-ss", &ss, "-t", &t, "-i", &path, "-map", &map, "-ac", "1",
            "-ar", &rate, "-f", "f32le", "-",
        ],
    )?;
    Ok(out
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// Decode all of audio track `track` as mono f32 at [`ANALYSIS_RATE`], handing it to `sink` in
/// blocks of whole `block` samples (the last may be shorter). Streams, because a 30h track would be
/// about 7 GB of samples held at once.
pub fn stream_track(
    file: &Path,
    track: usize,
    block: usize,
    mut sink: impl FnMut(&[f32]),
) -> Result<()> {
    let program = "ffmpeg";
    let (map, rate) = (format!("0:a:{track}"), ANALYSIS_RATE.to_string());
    let mut child = Command::new(program)
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", &map, "-ac", "1", "-ar", &rate, "-f", "f32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn { program, source })?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    // Drain stderr on a thread so a chatty ffmpeg cannot block on a full pipe.
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let mut bytes = vec![0u8; block.max(1) * 4];
    let mut samples = Vec::with_capacity(block.max(1));
    let mut filled = 0;
    loop {
        let n = stdout.read(&mut bytes[filled..])?;
        filled += n;
        if filled == bytes.len() || (n == 0 && filled >= 4) {
            let whole = filled / 4 * 4;
            samples.clear();
            samples.extend(
                bytes[..whole]
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
            sink(&samples);
            bytes.copy_within(whole..filled, 0);
            filled -= whole;
        }
        if n == 0 {
            break;
        }
    }
    let status = child.wait()?;
    let stderr = errs.join().unwrap_or_default();
    if !status.success() {
        return Err(Error::Tool { program, stderr });
    }
    Ok(())
}

/// Write `len` seconds of audio track `track` from `start` to `out` as 16 kHz mono 16-bit WAV, the
/// input whisper.cpp reads.
pub fn extract_wav(file: &Path, track: usize, start: f64, len: f64, out: &Path) -> Result<()> {
    let (path, out) = (file.to_string_lossy(), out.to_string_lossy());
    let (ss, t, map, rate) = (
        format!("{start:.3}"),
        format!("{len:.3}"),
        format!("0:a:{track}"),
        ANALYSIS_RATE.to_string(),
    );
    run(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-ss",
            &ss,
            "-t",
            &t,
            "-i",
            &path,
            "-map",
            &map,
            "-ac",
            "1",
            "-ar",
            &rate,
            "-c:a",
            "pcm_s16le",
            &out,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_json_without_titles() {
        let json = br#"{"streams":[
            {"codec_name":"aac","channels":2,"sample_rate":"48000","duration":"10.5"},
            {"codec_name":"aac","channels":2,"sample_rate":"48000","tags":{"title":"Mic"}}],
            "format":{"duration":"10.6"}}"#;
        let p = parse_probe(json).unwrap();
        assert_eq!(p.duration, 10.6);
        assert_eq!(p.audio.len(), 2);
        assert_eq!(p.audio[0].title, None);
        assert_eq!(p.audio[1].title.as_deref(), Some("Mic"));
        assert_eq!(p.audio[1].index, 1);
    }

    #[test]
    fn file_without_audio_has_no_streams() {
        let p = parse_probe(br#"{"streams":[],"format":{"duration":"3"}}"#).unwrap();
        assert!(p.audio.is_empty());
    }
}
