//! Every command's flags in one table, so what a command accepts and what `USAGE` shows can be
//! checked against each other (the test below) instead of drifting apart.

const PROJECT: &[&str] = &["--project", "--cache-dir"];
const CUT: &[&str] = &["--pad-before", "--pad-after", "--merge-gap", "--crossfade"];

/// The flags `command` accepts; any other is refused ([`crate::args::Args::reject_unknown`]).
pub fn known(command: &str) -> Vec<&'static str> {
    let (project, cut, own): (bool, bool, &[&'static str]) = match command {
        "tracks" => (
            false,
            false,
            &[
                "--json",
                "--cache-dir",
                "--no-cache",
                "--no-speech",
                "--windows",
                "--window-seconds",
            ],
        ),
        "separate" => (
            false,
            false,
            &[
                "--track",
                "--start",
                "--duration",
                "--cpu",
                "--force",
                "--no-speakers",
                "--json",
                "--cache-dir",
            ],
        ),
        "analyze" => (
            true,
            true,
            &[
                "--cpu",
                "--model",
                "--force",
                "--no-transcript",
                "--no-speakers",
                "--labels",
                "--keep",
            ],
        ),
        "judge" => (
            true,
            false,
            &["--prompt", "--agreement", "--cpu", "--force"],
        ),
        "status" => (true, false, &[]),
        "cut" => (true, true, &[]),
        "export" => (true, false, &["--fcpxml", "--srt"]),
        "render" => (true, false, &["--out", "--full", "--force", "--jobs"]),
        "voices" => (
            true,
            false,
            // Speakers always run on the CPU; `--cpu` is still accepted so older scripts keep working.
            &["--add", "--at", "--forget", "--rehear", "--cpu"],
        ),
        _ => (false, false, &[]),
    };
    let mut out: Vec<&'static str> = Vec::new();
    if project {
        out.extend(PROJECT);
    }
    out.extend(own);
    if cut {
        out.extend(CUT);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The flags `USAGE` shows for `command`, `[cut flags]` spelled out.
    fn shown(command: &str) -> BTreeSet<String> {
        let flags = |text: &str| -> Vec<String> {
            text.split(|c: char| c.is_whitespace() || "[]|".contains(c))
                .filter(|w| w.starts_with("--"))
                .map(String::from)
                .collect()
        };
        let mut lines = crate::USAGE.lines().skip(1).peekable();
        let mut text = String::new();
        while let Some(l) = lines.next() {
            if l.trim_start().starts_with(&format!("junctur {command} "))
                || l.trim() == format!("junctur {command}")
            {
                text.push_str(l);
                while let Some(more) = lines.next_if(|m| m.starts_with("     ")) {
                    text.push_str(more);
                }
            }
        }
        assert!(!text.is_empty(), "{command} is in USAGE");
        let mut out: BTreeSet<String> = flags(&text).into_iter().collect();
        if text.contains("[cut flags]") {
            let cut = crate::USAGE
                .lines()
                .find(|l| l.starts_with("cut flags:"))
                .unwrap();
            out.extend(flags(cut));
        }
        out
    }

    #[test]
    fn usage_shows_every_flag_a_command_takes_and_no_other() {
        for command in [
            "tracks", "separate", "analyze", "judge", "status", "cut", "export", "render", "voices",
        ] {
            let mut takes: BTreeSet<String> =
                known(command).into_iter().map(String::from).collect();
            if command == "voices" {
                takes.remove("--cpu");
            }
            assert_eq!(shown(command), takes, "{command}");
        }
    }

    #[test]
    fn every_switch_is_a_flag_some_command_takes() {
        let all: BTreeSet<&str> = [
            "tracks", "separate", "analyze", "judge", "status", "cut", "export", "render", "voices",
        ]
        .into_iter()
        .flat_map(known)
        .collect();
        for s in crate::args::SWITCHES {
            assert!(all.contains(s), "{s}");
        }
    }
}
