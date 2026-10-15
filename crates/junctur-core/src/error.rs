//! One error type for the library, so the CLI can print any failure the same way.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// ffmpeg/ffprobe could not be started (usually: not on PATH).
    Spawn {
        program: &'static str,
        source: std::io::Error,
    },
    /// ffmpeg/ffprobe ran but failed; `stderr` is its own message.
    Tool {
        program: &'static str,
        stderr: String,
    },
    /// ffprobe output we could not understand.
    Probe(String),
    /// A model could not be fetched or failed its checksum.
    Download(String),
    /// ONNX Runtime refused to load or run a model.
    Model(String),
    /// The user stopped it (e.g. a model download for a cancelled translation).
    Cancelled,
    /// An edit list that cannot be read, or a stage asked for before its inputs exist.
    Edit(String),
    /// The renderer cannot produce the requested output (e.g. it would overwrite the source).
    Render(String),
    /// An output path that must not be written (e.g. it is the source recording).
    Output(String),
    /// The user's config file is broken, or named and missing ([`crate::config`]).
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
            Error::Download(msg) => write!(f, "model download failed: {msg}"),
            Error::Model(msg) => write!(f, "model error: {msg}"),
            Error::Cancelled => write!(f, "cancelled"),
            Error::Edit(msg) => write!(f, "{msg}"),
            Error::Render(msg) => write!(f, "render: {msg}"),
            Error::Output(msg) => write!(f, "{msg}"),
            Error::Config(msg) => write!(f, "{msg}"),
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

impl<T> From<ort::Error<T>> for Error {
    fn from(e: ort::Error<T>) -> Self {
        Error::Model(e.to_string())
    }
}
