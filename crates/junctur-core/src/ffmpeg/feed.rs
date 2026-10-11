//! An ffmpeg fed through stdin (e.g. a muxer reading a byte stream), with its stderr kept for errors.

use std::io::{Read, Write};
use std::process::{ChildStdin, Command, Stdio};

use super::Process;
use crate::Result;

pub struct Feed {
    proc: Process,
    input: Option<ChildStdin>,
}

impl Feed {
    pub fn spawn(args: &[String]) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(args);
        let mut proc = Process::spawn(&mut cmd, Stdio::piped(), Stdio::null())?;
        let input = proc.child.stdin.take();
        Ok(Self { proc, input })
    }

    /// Everything `from` holds, streamed: a muxed piece can be gigabytes and never sits in memory.
    /// A failed read is our error; any failed write is ffmpeg having stopped, reported with its stderr.
    pub fn copy_from(&mut self, from: &mut dyn Read) -> Result<()> {
        let mut block = vec![0u8; 1 << 16];
        loop {
            let n = match from.read(&mut block) {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            let input = self.input.as_mut().expect("open until finish");
            if let Err(e) = input.write_all(&block[..n]) {
                drop(self.input.take());
                return Err(self.proc.error(e.to_string()));
            }
        }
    }

    pub fn finish(mut self) -> Result<()> {
        drop(self.input.take());
        self.proc.wait("mux")
    }
}
