//! The only place that runs ffmpeg/ffprobe.
//!
//! They are external programs found on PATH (a DECISIONS-level choice for now), so every call goes
//! through here: one spot to swap for bundled binaries or a native decoder later. [`probe`] asks
//! what a file holds, [`pcm`] streams audio samples in and out, [`feed`] pipes bytes into a muxer.

mod feed;
mod pcm;
pub mod probe;

use std::io::Read;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use crate::{Error, Result};

pub use feed::Feed;
pub use pcm::{decode_window, decode_windows, PcmReader, PcmWriter, Span, ANALYSIS_RATE};
pub use probe::{probe, probe_video, AudioStream, Probe, VideoStream};

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

/// Whether this ffmpeg build has an encoder (e.g. `h264_nvenc`; the build may lack it, or the
/// driver may be missing, which shows only when encoding starts).
pub fn has_encoder(name: &str) -> Result<bool> {
    let out = run("ffmpeg", &["-hide_banner", "-encoders"])?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .any(|l| l.split_whitespace().nth(1) == Some(name)))
}

/// [`has_encoder`], asked once per process for each name: the answer cannot change while we run,
/// and a GUI that renders again would otherwise start an ffmpeg just to ask.
pub fn has_encoder_cached(name: &str) -> Result<bool> {
    static KNOWN: OnceLock<Mutex<Vec<(String, bool)>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Mutex::default);
    if let Some(&(_, yes)) = known.lock().unwrap().iter().find(|(n, _)| n == name) {
        return Ok(yes);
    }
    let yes = has_encoder(name)?;
    known.lock().unwrap().push((name.to_string(), yes));
    Ok(yes)
}

/// The end of a running ffmpeg's stderr, drained on a thread: a full stderr pipe would block
/// ffmpeg while we block on its stdin or stdout. Only the tail is kept, so 30 h of decode warnings
/// cannot grow without bound; the error that ended the run is at the end.
struct StderrTail(Option<JoinHandle<String>>);

/// Bytes of stderr kept.
const STDERR_TAIL: usize = 4096;

impl StderrTail {
    fn drain(mut err: ChildStderr) -> Self {
        Self(Some(std::thread::spawn(move || {
            let mut tail = Vec::new();
            let mut block = [0u8; 1024];
            while let Ok(n @ 1..) = err.read(&mut block) {
                tail.extend_from_slice(&block[..n]);
                if tail.len() > 2 * STDERR_TAIL {
                    tail.drain(..tail.len() - STDERR_TAIL);
                }
            }
            let start = tail.len().saturating_sub(STDERR_TAIL);
            String::from_utf8_lossy(&tail[start..]).into_owned()
        })))
    }
}

/// A running ffmpeg with its stderr kept for errors. Dropping it kills and reaps the process, so
/// an abandoned stream (an error mid-way) never leaves ffmpeg running.
struct Process {
    child: Child,
    stderr: StderrTail,
}

impl Process {
    /// Start `cmd` (an ffmpeg); stdin and stdout as given, stderr kept.
    fn spawn(cmd: &mut Command, stdin: Stdio, stdout: Stdio) -> Result<Self> {
        let mut child = cmd
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| Error::Spawn {
                program: "ffmpeg",
                source,
            })?;
        let stderr = StderrTail::drain(child.stderr.take().expect("piped stderr"));
        Ok(Self { child, stderr })
    }

    /// Wait for the exit; a failure carries ffmpeg's stderr, or `what` and the status when it said
    /// nothing.
    fn wait(&mut self, what: &str) -> Result<()> {
        let status = self.child.wait()?;
        if !status.success() {
            return Err(self.error(format!("{what} exited with {status}")));
        }
        Ok(())
    }

    /// The process failed: its stderr, or `fallback` when it said nothing. Waits for the exit.
    fn error(&mut self, fallback: String) -> Error {
        let _ = self.child.wait();
        let stderr = self
            .stderr
            .0
            .take()
            .and_then(|h| h.join().ok())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(fallback);
        Error::Tool {
            program: "ffmpeg",
            stderr,
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
