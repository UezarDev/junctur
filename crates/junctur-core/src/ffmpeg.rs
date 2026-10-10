//! The only place that runs ffmpeg/ffprobe.
//!
//! They are external programs found on PATH (a DECISIONS-level choice for now), so every call goes
//! through here: one spot to swap for bundled binaries or a native decoder later.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::{Deserialize, Serialize};

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

fn run(program: &'static str, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Spawn { program, source })?;
    if !out.status.success() {
        return Err(Error::Tool {
            program,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(out.stdout)
}

/// Run ffmpeg to completion; its stderr becomes the error.
pub fn ffmpeg(args: &[String]) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run("ffmpeg", &args).map(drop)
}

/// Raw ffprobe output, for callers that parse a query of their own.
pub fn ffprobe(args: &[&str]) -> Result<Vec<u8>> {
    run("ffprobe", args)
}

/// Whether this ffmpeg build has an encoder (e.g. `h264_nvenc`; the build may lack it, or the
/// driver may be missing, which shows only when encoding starts).
pub fn has_encoder(name: &str) -> Result<bool> {
    let out = run("ffmpeg", &["-hide_banner", "-encoders"])?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .any(|l| l.split_whitespace().nth(1) == Some(name)))
}

/// An ffmpeg fed through stdin (e.g. a muxer reading a byte stream), with its stderr kept for errors.
pub struct Feed {
    child: Child,
    input: Option<ChildStdin>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

impl Feed {
    pub fn spawn(args: &[String]) -> Result<Self> {
        let mut child = Command::new("ffmpeg")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| Error::Spawn {
                program: "ffmpeg",
                source,
            })?;
        let input = child.stdin.take();
        // Drained on a thread: a full stderr pipe would block ffmpeg while we block on its stdin.
        let mut err = child.stderr.take().expect("piped stderr");
        let stderr = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        Ok(Self {
            child,
            input,
            stderr: Some(stderr),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let r = self
            .input
            .as_mut()
            .expect("open until finish")
            .write_all(bytes);
        r.map_err(|e| self.fail(e.to_string()))
    }

    fn fail(&mut self, fallback: String) -> Error {
        drop(self.input.take());
        let _ = self.child.wait();
        let stderr = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(fallback);
        Error::Tool {
            program: "ffmpeg",
            stderr,
        }
    }

    pub fn finish(mut self) -> Result<()> {
        drop(self.input.take());
        let status = self.child.wait()?;
        if !status.success() {
            return Err(self.fail(format!("exited with {status}")));
        }
        Ok(())
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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

/// The first video stream: size and frame rate (as a fraction, e.g. 30000/1001).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VideoStream {
    pub width: u32,
    pub height: u32,
    pub fps_num: u64,
    pub fps_den: u64,
}

/// None for an audio-only file.
pub fn probe_video(file: &Path) -> Result<Option<VideoStream>> {
    let path = file.to_string_lossy();
    let out = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-select_streams",
            "v:0",
            &path,
        ],
    )?;
    parse_video(&out)
}

fn parse_video(json: &[u8]) -> Result<Option<VideoStream>> {
    let v: serde_json::Value = serde_json::from_slice(json)?;
    let Some(s) = v["streams"].as_array().and_then(|a| a.first()) else {
        return Ok(None);
    };
    let rate = s["r_frame_rate"].as_str().unwrap_or("");
    let (n, d) = rate
        .split_once('/')
        .and_then(|(n, d)| Some((n.parse().ok()?, d.parse().ok()?)))
        .filter(|&(n, d): &(u64, u64)| n > 0 && d > 0)
        .ok_or_else(|| Error::Probe(format!("frame rate {rate:?}")))?;
    Ok(Some(VideoStream {
        width: s["width"].as_u64().unwrap_or(0) as u32,
        height: s["height"].as_u64().unwrap_or(0) as u32,
        fps_num: n,
        fps_den: d,
    }))
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

/// A section of a file to read: `start` seconds in, `len` seconds long (None = to the end).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Span {
    pub start: f64,
    pub len: Option<f64>,
}

/// Streaming decode of one audio track as interleaved f32, so a 30h track never sits in memory.
pub struct PcmReader {
    child: Child,
    out: ChildStdout,
    pub rate: u32,
    pub channels: u32,
}

impl PcmReader {
    pub fn open(file: &Path, track: usize, rate: u32, channels: u32, span: Span) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-v",
            "error",
            "-nostdin",
            "-ss",
            &format!("{:.3}", span.start),
        ]);
        if let Some(len) = span.len {
            cmd.args(["-t", &format!("{len:.3}")]);
        }
        cmd.arg("-i").arg(file).args([
            "-map",
            &format!("0:a:{track}"),
            "-ac",
            &channels.to_string(),
            "-ar",
            &rate.to_string(),
            "-f",
            "f32le",
            "-",
        ]);
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| Error::Spawn {
                program: "ffmpeg",
                source,
            })?;
        let out = child.stdout.take().expect("piped stdout");
        Ok(Self {
            child,
            out,
            rate,
            channels,
        })
    }

    /// Up to `n` interleaved samples; fewer only at the end of the stream, empty when done.
    pub fn read(&mut self, n: usize) -> Result<Vec<f32>> {
        let mut bytes = vec![0u8; n * 4];
        let mut got = 0;
        while got < bytes.len() {
            match self.out.read(&mut bytes[got..])? {
                0 => break,
                k => got += k,
            }
        }
        bytes.truncate(got - got % 4);
        Ok(bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect())
    }

    pub fn finish(mut self) -> Result<()> {
        let status = self.child.wait()?;
        if !status.success() {
            return Err(Error::Tool {
                program: "ffmpeg",
                stderr: format!("decode exited with {status}"),
            });
        }
        Ok(())
    }
}

// An abandoned stream (an error mid-way) must not leave ffmpeg running.
impl Drop for PcmReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PcmWriter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Streaming encode of interleaved f32 into an audio file; the format follows the extension.
pub struct PcmWriter {
    child: Child,
    input: Option<ChildStdin>,
}

impl PcmWriter {
    pub fn create(path: &Path, rate: u32, channels: u32) -> Result<Self> {
        Self::create_with(path, rate, channels, &[])
    }

    /// As [`create`](Self::create), with output options (codec, bitrate) before the path.
    pub fn create_with(path: &Path, rate: u32, channels: u32, opts: &[String]) -> Result<Self> {
        let mut child = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "f32le", "-ar", &rate.to_string()])
            .args(["-ac", &channels.to_string(), "-i", "-"])
            .args(opts)
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| Error::Spawn {
                program: "ffmpeg",
                source,
            })?;
        let input = child.stdin.take();
        Ok(Self { child, input })
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.input
            .as_mut()
            .expect("open until finish")
            .write_all(&bytes)?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        drop(self.input.take());
        let status = self.child.wait()?;
        if !status.success() {
            return Err(Error::Tool {
                program: "ffmpeg",
                stderr: format!("encode exited with {status}"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_video_stream() {
        let j = br#"{"streams":[{"width":1920,"height":1080,"r_frame_rate":"30000/1001"}]}"#;
        let v = parse_video(j).unwrap().unwrap();
        assert_eq!((v.width, v.fps_num, v.fps_den), (1920, 30000, 1001));
        assert_eq!(parse_video(br#"{"streams":[]}"#).unwrap(), None);
        assert!(parse_video(br#"{"streams":[{"r_frame_rate":"0/0"}]}"#).is_err());
    }

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
