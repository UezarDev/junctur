//! Which Whisper model transcribes, and where its ggml file lives.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// large-v3-turbo is the default: in the SmartCutter benchmark it matched large-v3 closely at about
/// 5x the speed. Its known weakness, repeating lines, is held off by short VAD chunks transcribed
/// with no carried context. large-v3 stays available for the best quality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Model {
    #[default]
    #[serde(alias = "turbo")]
    LargeV3Turbo,
    #[serde(alias = "best")]
    LargeV3,
}

impl Model {
    pub const ALL: [Model; 2] = [Model::LargeV3Turbo, Model::LargeV3];

    pub fn name(self) -> &'static str {
        match self {
            Model::LargeV3Turbo => "large-v3-turbo",
            Model::LargeV3 => "large-v3",
        }
    }

    /// The file name whisper.cpp publishes the model under.
    pub fn file_name(self) -> String {
        format!("ggml-{}.bin", self.name())
    }

    pub fn path_in(self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.file_name())
    }

    pub fn download_url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.file_name()
        )
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Model {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "large-v3-turbo" | "turbo" => Ok(Model::LargeV3Turbo),
            "large-v3" | "best" => Ok(Model::LargeV3),
            _ => Err(format!(
                "unknown model {s}; use large-v3-turbo (default, alias turbo) or large-v3 (alias best)"
            )),
        }
    }
}

/// Order: `JUNCTUR_MODELS_DIR`, then the OS user data folder.
pub fn default_models_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("JUNCTUR_MODELS_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("junctur").join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turbo_is_the_default() {
        assert_eq!(Model::default(), Model::LargeV3Turbo);
        assert_eq!(Model::default().file_name(), "ggml-large-v3-turbo.bin");
    }

    #[test]
    fn names_and_aliases_parse() {
        for m in Model::ALL {
            assert_eq!(m.name().parse::<Model>().unwrap(), m);
        }
        assert_eq!("best".parse::<Model>().unwrap(), Model::LargeV3);
        assert_eq!("turbo".parse::<Model>().unwrap(), Model::LargeV3Turbo);
        assert!("large-v2".parse::<Model>().is_err());
    }

    #[test]
    fn serde_uses_the_same_names() {
        let json = serde_json::to_string(&Model::LargeV3).unwrap();
        assert_eq!(json, "\"large-v3\"");
        assert_eq!(
            serde_json::from_str::<Model>("\"best\"").unwrap(),
            Model::LargeV3
        );
    }
}
