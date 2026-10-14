//! Audio samples in and out of ffmpeg as raw little-endian f32: whole windows for analysis,
//! streams for anything long enough that it must never sit in memory.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};

use super::{run, Process};
use crate::Result;

/// Sample rate the analysis decodes at. 16 kHz mono keeps eight windows of four tracks small while
/// keeping speech and most game audio energy; it is also what VAD and Whisper take.
pub const ANALYSIS_RATE: u32 = 16_000;

// ffmpeg writes `f32le`; the samples are read and written in place as native f32.
#[cfg(not(target_endian = "little"))]
compile_error!("PCM is exchanged with ffmpeg as native little-endian f32");

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
    Ok(decode_windows(file, &[track], start, len)?.remove(0))
}

/// [`decode_window`] for several tracks with one ffmpeg: each track is downmixed and resampled on
/// its own (`aformat`, the same conversion `-ac 1 -ar` makes), `amerge` lays them side by side as
/// channels, and they are split apart here. One process per window instead of one per track per
/// window: each start opens the whole MP4. Every track comes back as long as the shortest.
pub fn decode_windows(
    file: &Path,
    tracks: &[usize],
    start: f64,
    len: f64,
) -> Result<Vec<Vec<f32>>> {
    let n = tracks.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let path = file.to_string_lossy();
    let (ss, t) = (format!("{start:.3}"), format!("{len:.3}"));
    let mut args: Vec<String> = ["-v", "error", "-nostdin", "-ss", &ss, "-t", &t, "-i", &path]
        .map(String::from)
        .to_vec();
    // Windows stop at the shortest track: the analysis compares tracks sample by sample, and a
    // window past one track's end has nothing of it to compare (`window::align` cuts them so too).
    args.extend(merge_args(tracks, ANALYSIS_RATE, None));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let all = samples(&run("ffmpeg", &args)?);
    let mut out = vec![Vec::with_capacity(all.len() / n); n];
    for frame in all.chunks_exact(n) {
        for (track, &x) in out.iter_mut().zip(frame) {
            track.push(x);
        }
    }
    Ok(out)
}

/// The output side of an ffmpeg that decodes `tracks` (at least one) as mono f32 at `rate`, side by
/// side as channels in that order. `amerge` ends with the shortest input; with `pad` (seconds) every
/// track is padded with silence to that length, so a track that ends early does not cut the others.
fn merge_args(tracks: &[usize], rate: u32, pad: Option<f64>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let ([t], None) = (tracks, pad) {
        args.extend(["-map".into(), format!("0:a:{t}"), "-ac".into(), "1".into()]);
        args.extend(["-ar".into(), rate.to_string()]);
    } else {
        let apad = if pad.is_some() { ",apad" } else { "" };
        let mut g = String::new();
        for (i, t) in tracks.iter().enumerate() {
            g += &format!(
                "[0:a:{t}]aformat=sample_fmts=flt:sample_rates={rate}:channel_layouts=mono{apad}[a{i}];"
            );
        }
        if tracks.len() == 1 {
            g.pop();
            g = g.replace("[a0]", "[out]");
        } else {
            for i in 0..tracks.len() {
                g += &format!("[a{i}]");
            }
            g += &format!("amerge=inputs={}[out]", tracks.len());
        }
        args.extend(["-filter_complex".into(), g, "-map".into(), "[out]".into()]);
        if let Some(len) = pad {
            args.extend(["-t".into(), format!("{len:.3}")]);
        }
    }
    args.extend(["-f".into(), "f32le".into(), "-".into()]);
    args
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

    /// Every one of `tracks` (at least one) from the start, mono at `rate`, interleaved one sample
    /// per track in that order (`channels` is their count): one decode for all, as
    /// [`decode_windows`] does, streamed for a pass over a whole recording (the waveforms). Every
    /// track lasts `len` seconds when given (the recording's length), padded with silence where it
    /// ends early; without, all stop with the shortest.
    pub fn merged(file: &Path, tracks: &[usize], rate: u32, len: Option<f64>) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-nostdin", "-i"])
            .arg(file)
            .args(merge_args(tracks, rate, len));
        let mut proc = Process::spawn(&mut cmd, Stdio::null(), Stdio::piped())?;
        let out = proc.child.stdout.take().expect("piped stdout");
        Ok(Self {
            proc,
            out,
            rate,
            channels: tracks.len() as u32,
        })
    }

    /// Up to `n` interleaved samples; fewer only at the end of the stream, empty when done.
    pub fn read(&mut self, n: usize) -> Result<Vec<f32>> {
        let mut buf = Vec::new();
        self.read_into(&mut buf, n)?;
        Ok(buf)
    }

    /// [`read`](Self::read) into `buf` (replacing what it held), reusing its memory: the pipe's
    /// bytes land straight in the f32 storage, no byte vector in between.
    pub fn read_into(&mut self, buf: &mut Vec<f32>, n: usize) -> Result<()> {
        buf.clear();
        buf.resize(n, 0.0);
        let bytes: &mut [u8] = bytemuck::cast_slice_mut(buf.as_mut_slice());
        let mut got = 0;
        while got < bytes.len() {
            match self.out.read(&mut bytes[got..]) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        // A sample split across the end (ffmpeg killed mid-write) is dropped.
        buf.truncate(got / 4);
        Ok(())
    }

    /// Wait for the decoder to exit: Err, with its message, when the decode failed. For a stream
    /// read to its end.
    pub fn wait(&mut self) -> Result<()> {
        self.proc.wait("decode")
    }

    pub fn finish(mut self) -> Result<()> {
        self.wait()
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
        let r = self
            .input
            .as_mut()
            .expect("open until finish")
            .write_all(bytemuck::cast_slice(samples));
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

    /// Reference: each case's arguments written out on their own.
    fn merge_args_before(tracks: &[usize], rate: u32, pad: Option<f64>) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if let Some(len) = pad {
            let mut g = String::new();
            for (i, t) in tracks.iter().enumerate() {
                g += &format!(
                    "[0:a:{t}]aformat=sample_fmts=flt:sample_rates={rate}:channel_layouts=mono,apad[a{i}];"
                );
            }
            if tracks.len() == 1 {
                g.pop();
                g = g.replace("[a0]", "[out]");
            } else {
                for i in 0..tracks.len() {
                    g += &format!("[a{i}]");
                }
                g += &format!("amerge=inputs={}[out]", tracks.len());
            }
            args.extend(["-filter_complex".into(), g, "-map".into(), "[out]".into()]);
            args.extend(["-t".into(), format!("{len:.3}")]);
        } else if let [t] = tracks {
            args.extend(["-map".into(), format!("0:a:{t}"), "-ac".into(), "1".into()]);
            args.extend(["-ar".into(), rate.to_string()]);
        } else {
            let mut g = String::new();
            for (i, t) in tracks.iter().enumerate() {
                g += &format!(
                    "[0:a:{t}]aformat=sample_fmts=flt:sample_rates={rate}:channel_layouts=mono[a{i}];"
                );
            }
            for i in 0..tracks.len() {
                g += &format!("[a{i}]");
            }
            g += &format!("amerge=inputs={}[out]", tracks.len());
            args.extend(["-filter_complex".into(), g, "-map".into(), "[out]".into()]);
        }
        args.extend(["-f".into(), "f32le".into(), "-".into()]);
        args
    }

    #[test]
    fn merge_args_are_the_reference_ones() {
        for tracks in [&[0][..], &[2], &[0, 1], &[3, 0, 1]] {
            for pad in [None, Some(12.0), Some(3600.25)] {
                assert_eq!(
                    merge_args(tracks, 16_000, pad),
                    merge_args_before(tracks, 16_000, pad),
                    "{tracks:?} {pad:?}"
                );
            }
        }
        assert_eq!(
            merge_args(&[1], 8000, Some(2.0)).join(" "),
            "-filter_complex [0:a:1]aformat=sample_fmts=flt:sample_rates=8000:channel_layouts=mono,apad[out] -map [out] -t 2.000 -f f32le -"
        );
    }

    #[test]
    fn decode_failure_carries_the_ffmpeg_message() {
        let missing = std::env::temp_dir().join("junctur-no-such-file.mp4");
        let mut r = PcmReader::open(&missing, 0, 16_000, 1, Span::default()).unwrap();
        assert!(r.read(1024).unwrap().is_empty());
        let e = r.finish().unwrap_err().to_string();
        assert!(e.contains("junctur-no-such-file"), "{e}");
    }

    #[test]
    fn pcm_round_trips_through_ffmpeg() {
        let dir = std::env::temp_dir().join(format!("junctur-pcm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("ramp.wav");
        let ramp: Vec<f32> = (0..32_000).map(|i| (i as f32 / 32_000.0) - 0.5).collect();
        let mut w =
            PcmWriter::create_with(&wav, 16_000, 1, &["-c:a".into(), "pcm_f32le".into()]).unwrap();
        w.write(&ramp).unwrap();
        w.finish().unwrap();
        let mut r = PcmReader::open(&wav, 0, 16_000, 1, Span::default()).unwrap();
        let mut buf = vec![9.0; 3];
        r.read_into(&mut buf, 20_000).unwrap();
        assert_eq!(buf, ramp[..20_000]);
        r.read_into(&mut buf, 20_000).unwrap();
        assert_eq!(buf, ramp[20_000..]);
        r.read_into(&mut buf, 20_000).unwrap();
        assert!(buf.is_empty());
        r.finish().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_ffmpeg_decodes_every_track_as_separate_ones_did() {
        let dir = std::env::temp_dir().join(format!("junctur-windows-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("three.mka");
        // Stereo with different channels, mono, and stereo at another rate.
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=300:d=4"])
            .args(["-f", "lavfi", "-i", "sine=f=500:d=4", "-f", "lavfi", "-i"])
            .args(["sine=f=700:d=3.5:sample_rate=44100", "-filter_complex"])
            .args(["[0:a][1:a]join=inputs=2:channel_layout=stereo[s];[2:a]asplit=2[m][x];[x]pan=stereo|c0=c0|c1=0.3*c0[t]"])
            .args(["-map", "[s]", "-map", "[m]", "-map", "[t]", "-c:a", "flac"])
            .arg(&f)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let together = decode_windows(&f, &[0, 1, 2], 1.0, 3.0).unwrap();
        let alone: Vec<Vec<f32>> = (0..3)
            .map(|i| decode_window(&f, i, 1.0, 3.0).unwrap())
            .collect();
        // Every track as long as the shortest (the 3.5 s one), as `window::align` cut them.
        let shortest = alone.iter().map(Vec::len).min().unwrap();
        for (i, got) in together.iter().enumerate() {
            assert_eq!(got.len(), shortest, "track {i}");
            // The same samples, but for the resampler's first millisecond, which settles from a
            // different start (17 samples, at most 0.0024 apart).
            let (head, rest) = got.split_at(32);
            assert_eq!(rest, &alone[i][32..shortest], "track {i}");
            assert!(head
                .iter()
                .zip(&alone[i])
                .all(|(a, b)| (a - b).abs() < 0.01));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
