//! One error type for the library, so the CLI can print any failure the same way.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    /// An external program (ffmpeg, ffprobe, whisper-cli) could not be started, usually because it
    /// is not on PATH.
    Spawn {
        program: &'static str,
        source: std::io::Error,
    },
    /// An external program ran but failed; `stderr` is its own message.
    Tool {
        program: &'static str,
        stderr: String,
    },
    /// ffprobe output we could not understand.
    Probe(String),
    /// The Whisper model file is not where it should be.
    Model(PathBuf),
    /// whisper-cli ran but its output was missing or unreadable.
    Whisper(String),
    /// The config file could not be read.
    Config(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spawn { program, source } => {
                write!(f, "could not start {program} (is it on PATH?): {source}")
            }
            Error::Tool { program, stderr } => write!(f, "{program} failed: {}", stderr.trim()),
            Error::Probe(msg) => write!(f, "unexpected ffprobe output: {msg}"),
            Error::Model(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                write!(
                    f,
                    "Whisper model not found at {}; download it from \
                     https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{name}",
                    path.display()
                )
            }
            Error::Whisper(msg) => write!(f, "unexpected whisper-cli output: {msg}"),
            Error::Config(msg) => write!(f, "bad config file {msg}"),
            Error::Io(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}
