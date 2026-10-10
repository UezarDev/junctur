//! Free GPU memory, asked of `nvidia-smi` (ships with the NVIDIA driver).
//!
//! Another app (a game, OBS) often holds most of the card, and a model that does not fit fails or
//! thrashes, so the model choice is made from what is free right now, not from the card's size.

use std::process::{Command, Stdio};

/// Free MiB on the first GPU, or None when there is no NVIDIA driver or it cannot be asked.
pub fn free_mib() -> Option<u64> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

fn parse(s: &str) -> Option<u64> {
    s.lines().next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_first_gpu() {
        assert_eq!(super::parse("3386\n11000\n"), Some(3386));
        assert_eq!(super::parse("N/A\n"), None);
    }
}
