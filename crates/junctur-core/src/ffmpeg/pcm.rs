//! Audio samples in and out of ffmpeg as raw little-endian f32: whole windows for analysis,
//! streams for anything long enough that it must never sit in memory.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use super::{run, Process};
use crate::Result;

/// Sample rate the analysis decodes at. 16 kHz mono keeps eight windows of four tracks small while
/// keeping speech and most game audio energy; it is also what the VAD/Whisper stages will want.
pub const ANALYSIS_RATE: u32 = 16_000;

/// Little-endian f32 bytes as samples; a trailing partial sample is dropped.
fn samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
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
    Ok(samples(&out))
}

/// A section of a file to read: `start` seconds in, `len` seconds long (None = to the end).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Span {
    pub start: f64,
    pub len: Option<f64>,
}

/// Streaming decode of one audio track as interleaved f32, so a 30h track never sits in memory.
pub struct PcmReader {
    proc: Process,
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
        let mut proc = Process::spawn(&mut cmd, Stdio::null(), Stdio::piped())?;
        let out = proc.child.stdout.take().expect("piped stdout");
        Ok(Self {
            proc,
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
        Ok(samples(&bytes))
    }

    pub fn finish(mut self) -> Result<()> {
        self.proc.wait("decode")
    }
}

/// Streaming encode of interleaved f32 into an audio file; the format follows the extension.
pub struct PcmWriter {
    proc: Process,
    input: Option<ChildStdin>,
}

impl PcmWriter {
    pub fn create(path: &Path, rate: u32, channels: u32) -> Result<Self> {
        Self::create_with(path, rate, channels, &[])
    }

    /// As [`create`](Self::create), with output options (codec, bitrate) before the path.
    pub fn create_with(path: &Path, rate: u32, channels: u32, opts: &[String]) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-y", "-f", "f32le", "-ar", &rate.to_string()])
            .args(["-ac", &channels.to_string(), "-i", "-"])
            .args(opts)
            .arg(path);
        let mut proc = Process::spawn(&mut cmd, Stdio::piped(), Stdio::null())?;
        let input = proc.child.stdin.take();
        Ok(Self { proc, input })
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let r = self
            .input
            .as_mut()
            .expect("open until finish")
            .write_all(&bytes);
        // A write fails when ffmpeg stopped reading, i.e. died: say why it did.
        r.map_err(|e| {
            drop(self.input.take());
            self.proc.error(e.to_string())
        })
    }

    pub fn finish(mut self) -> Result<()> {
        drop(self.input.take());
        self.proc.wait("encode")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_failure_carries_the_ffmpeg_message() {
        let missing = std::env::temp_dir().join("junctur-no-such-file.mp4");
        let mut r = PcmReader::open(&missing, 0, 16_000, 1, Span::default()).unwrap();
        assert!(r.read(1024).unwrap().is_empty());
        let e = r.finish().unwrap_err().to_string();
        assert!(e.contains("junctur-no-such-file"), "{e}");
    }
}
