//! Video frames as small RGB pictures, for the labels ([`crate::labels`]): every keyframe of a
//! recording in one pass, and one frame anywhere by seeking. Keyframes because OBS writes one every
//! 2 s and they decode alone, so the pass skips every other frame; NVDEC because a 1080p keyframe
//! costs ~60 ms on the CPU and a few on the card (3.2 s against 19 s per 10 minutes, measured).

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::{Error, Result};

/// Bytes per pixel: RGB.
pub const CHANNELS: usize = 3;

/// The hardware decoder asked for first. When ffmpeg cannot start it, the pass is made again on the
/// CPU ([`keyframes`]); an ffmpeg that can start it but not for this stream decodes in software.
const HWACCEL: [&str; 2] = ["-hwaccel", "cuda"];

/// Set once the hardware decoder failed in this process: no later frame asks for it again (each
/// seek of [`frame_at`] would otherwise start a failing ffmpeg first).
static NO_HWACCEL: AtomicBool = AtomicBool::new(false);

/// Lines of ffmpeg's stderr kept for an error.
const KEEP_LINES: usize = 20;

/// The arguments that make every frame `size` by `size` RGB on stdout: squashed, not cropped, as
/// SigLIP's own processor resizes.
fn scaled(size: u32, info: bool) -> Vec<String> {
    let filter = if info {
        format!("scale={size}:{size}:flags=bilinear,showinfo")
    } else {
        format!("scale={size}:{size}:flags=bilinear")
    };
    [
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-dn",
        "-fps_mode",
        "passthrough",
        "-vf",
        &filter,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ]
    .map(String::from)
    .to_vec()
}

/// Every keyframe of `file`'s first video stream, in order, as `size` by `size` RGB with its time
/// in seconds from the start: `more(t, pixels)`, which returns false to stop early. The hardware
/// decoder is tried first; a pass it fails is made again on the CPU, giving only the frames after
/// the last one given, so a decoder that gives up an hour in loses nothing.
pub fn keyframes(file: &Path, size: u32, more: &mut dyn FnMut(f64, &[u8]) -> bool) -> Result<()> {
    let mut last = f64::NEG_INFINITY;
    if !NO_HWACCEL.load(Ordering::Relaxed) {
        let hw = pass(file, size, true, &mut |t, px| {
            last = t;
            more(t, px)
        });
        if hw.is_ok() {
            return hw;
        }
        NO_HWACCEL.store(true, Ordering::Relaxed);
    }
    pass(file, size, false, &mut |t, px| t <= last || more(t, px))
}

/// One keyframe pass; `hw` asks for the hardware decoder.
fn pass(file: &Path, size: u32, hw: bool, more: &mut dyn FnMut(f64, &[u8]) -> bool) -> Result<()> {
    let mut args: Vec<String> = ["-hide_banner", "-nostats", "-nostdin", "-v", "info"]
        .map(String::from)
        .to_vec();
    if hw {
        args.extend(HWACCEL.map(String::from));
    }
    args.extend(["-skip_frame", "nokey", "-i"].map(String::from));
    args.push(file.to_string_lossy().into_owned());
    args.extend(scaled(size, true));
    let mut child = Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn {
            program: "ffmpeg",
            source,
        })?;
    // `showinfo` prints each frame's time on stderr as the frame leaves the filter, before its
    // bytes reach stdout, so the n-th time read is the n-th frame's. Read on a thread: a full
    // stderr pipe would block ffmpeg while this one waits on stdout.
    let err = child.stderr.take().expect("piped stderr");
    let (times, got) = mpsc::channel::<f64>();
    let reader = std::thread::spawn(move || {
        let mut tail: Vec<String> = Vec::new();
        for line in BufReader::new(err).lines() {
            let Ok(line) = line else { break };
            if let Some(t) = pts_time(&line) {
                if times.send(t).is_err() {
                    break;
                }
            } else if !line.contains("showinfo") {
                tail.push(line);
                if tail.len() > KEEP_LINES {
                    tail.remove(0);
                }
            }
        }
        tail.join("\n")
    });
    let mut out = child.stdout.take().expect("piped stdout");
    let mut frame = vec![0u8; size as usize * size as usize * CHANNELS];
    let mut stopped = false;
    let result = loop {
        match read_whole(&mut out, &mut frame) {
            Ok(false) => break Ok(()),
            Ok(true) => {}
            Err(e) => break Err(e),
        }
        let Ok(t) = got.recv() else {
            break Err(Error::Tool {
                program: "ffmpeg",
                stderr: "a frame came without its time".into(),
            });
        };
        if !more(t, &frame) {
            stopped = true;
            break Ok(());
        }
    };
    drop(out);
    if stopped || result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    drop(got);
    let tail = reader.join().unwrap_or_default();
    result?;
    if !stopped && !status.success() {
        return Err(Error::Tool {
            program: "ffmpeg",
            stderr: if tail.trim().is_empty() {
                format!("ffmpeg exited with {status}")
            } else {
                tail
            },
        });
    }
    Ok(())
}

/// Fills `buf` from `r`: true when whole, false at the end of the stream before any byte; a frame
/// cut short is an error.
fn read_whole(r: &mut impl Read, buf: &mut [u8]) -> Result<bool> {
    let mut at = 0;
    while at < buf.len() {
        match r.read(&mut buf[at..])? {
            0 if at == 0 => return Ok(false),
            0 => {
                return Err(Error::Tool {
                    program: "ffmpeg",
                    stderr: "the output ended mid-frame".into(),
                })
            }
            n => at += n,
        }
    }
    Ok(true)
}

/// The `pts_time:` of a `showinfo` line.
fn pts_time(line: &str) -> Option<f64> {
    if !line.contains("showinfo") {
        return None;
    }
    let rest = &line[line.find("pts_time:")? + "pts_time:".len()..];
    rest.split_whitespace().next()?.parse().ok()
}

/// The frame shown at `t` seconds into `file`, `size` by `size` RGB; `None` past its end. The
/// fallback for a stretch with no keyframe near a point it samples: an input seek decodes from the
/// keyframe before `t`, so this costs up to a keyframe interval of decoding.
pub fn frame_at(file: &Path, t: f64, size: u32) -> Result<Option<Vec<u8>>> {
    let mut args: Vec<String> = ["-v", "error", "-nostdin"].map(String::from).to_vec();
    let hw = !NO_HWACCEL.load(Ordering::Relaxed);
    if hw {
        args.extend(HWACCEL.map(String::from));
    }
    args.extend(["-ss".to_string(), format!("{t:.3}"), "-i".to_string()]);
    args.push(file.to_string_lossy().into_owned());
    args.extend(["-frames:v", "1"].map(String::from));
    args.extend(scaled(size, false));
    let want = size as usize * size as usize * CHANNELS;
    let one = |args: &[String]| -> Result<Vec<u8>> {
        let out = Command::new("ffmpeg")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|source| Error::Spawn {
                program: "ffmpeg",
                source,
            })?;
        if !out.status.success() {
            return Err(Error::Tool {
                program: "ffmpeg",
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        Ok(out.stdout)
    };
    let bytes = match one(&args) {
        Ok(b) => b,
        // Without the hardware decoder, as the keyframe pass falls back; remembered.
        Err(_) if hw => {
            NO_HWACCEL.store(true, Ordering::Relaxed);
            let soft: Vec<String> = args
                .iter()
                .filter(|a| !HWACCEL.contains(&a.as_str()))
                .cloned()
                .collect();
            one(&soft)?
        }
        Err(e) => return Err(e),
    };
    Ok((bytes.len() >= want).then(|| bytes[..want].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_come_from_showinfo_lines_only() {
        let line =
            "[Parsed_showinfo_1 @ 000001] n:   3 pts:  180000 pts_time:2.5     duration: 512";
        assert_eq!(pts_time(line), Some(2.5));
        assert_eq!(
            pts_time("[Parsed_showinfo_1 @ 0] config in time_base: 1/90000"),
            None
        );
        assert_eq!(pts_time("frame pts_time:4 elsewhere"), None);
    }

    #[test]
    fn a_frame_is_whole_or_an_error() {
        let mut buf = [0u8; 4];
        assert!(read_whole(&mut &[1u8, 2, 3, 4][..], &mut buf).unwrap());
        assert!(!read_whole(&mut &[][..], &mut buf).unwrap());
        assert!(read_whole(&mut &[1u8, 2][..], &mut buf).is_err());
    }
}
