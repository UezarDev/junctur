//! One positional file plus `--flag` / `--flag value` pairs, shared by every command.

use std::collections::HashMap;
use std::path::PathBuf;

/// Flags that take no value.
pub const SWITCHES: &[&str] = &[
    "--no-cache",
    "--no-speech",
    "--cpu",
    "--force",
    "--no-speakers",
    "--no-transcript",
    "--labels",
    "--full",
    "--rehear",
    "--no-model",
];

pub struct Args {
    pub file: Option<PathBuf>,
    values: HashMap<String, String>,
    switches: Vec<String>,
}

impl Args {
    pub fn parse(mut it: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut a = Args {
            file: None,
            values: HashMap::new(),
            switches: Vec::new(),
        };
        while let Some(x) = it.next() {
            if SWITCHES.contains(&x.as_str()) {
                a.switches.push(x);
            } else if x.starts_with("--") {
                let v = it.next().ok_or(format!("{x} needs a value"))?;
                a.values.insert(x, v);
            } else if a.file.is_none() {
                a.file = Some(PathBuf::from(x));
            } else {
                return Err(format!("unexpected argument {x}"));
            }
        }
        Ok(a)
    }

    pub fn file(&self) -> Result<PathBuf, String> {
        self.file.clone().ok_or_else(|| "missing <file>".into())
    }

    pub fn has(&self, switch: &str) -> bool {
        self.switches.iter().any(|s| s == switch)
    }

    pub fn value(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).map(String::as_str)
    }

    pub fn path(&self, flag: &str) -> Option<PathBuf> {
        self.values.get(flag).map(PathBuf::from)
    }

    pub fn num<T: std::str::FromStr>(&self, flag: &str) -> Result<Option<T>, String> {
        self.values
            .get(flag)
            .map(|v| v.parse().map_err(|_| format!("{flag}: not a number: {v}")))
            .transpose()
    }

    /// Flags this command does not know, so a typo is an error rather than silently ignored.
    pub fn reject_unknown(&self, known: &[&str]) -> Result<(), String> {
        let unknown = self
            .values
            .keys()
            .chain(&self.switches)
            .find(|k| !known.contains(&k.as_str()));
        match unknown {
            Some(k) => Err(format!("unknown flag {k} for this command")),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        Args::parse(s.split_whitespace().map(String::from))
    }

    #[test]
    fn values_switches_and_file() {
        let a = parse("in.mp4 --track 3 --cpu").unwrap();
        assert_eq!(a.file().unwrap(), PathBuf::from("in.mp4"));
        assert_eq!(a.num::<usize>("--track").unwrap(), Some(3));
        assert!(a.has("--cpu"));
        assert!(a.reject_unknown(&["--track", "--cpu"]).is_ok());
        assert!(a.reject_unknown(&["--track"]).is_err());
    }

    #[test]
    fn errors() {
        assert!(parse("a b").is_err());
        assert!(parse("a --track").is_err());
        assert!(parse("a --track x")
            .unwrap()
            .num::<usize>("--track")
            .is_err());
    }
}
