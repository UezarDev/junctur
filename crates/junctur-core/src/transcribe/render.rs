//! The human-readable transcript printed by `junctur transcribe`.

use super::Transcript;

impl Transcript {
    /// One block per transcribed track (numbered 1-based as OBS does), one line per segment.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "{}", self.file.display());
        let _ = writeln!(
            s,
            "model {}, language {}{}",
            self.model,
            self.language,
            if self.from_cache { " (cached)" } else { "" }
        );
        if self.tracks.is_empty() {
            let _ = writeln!(s, "no speech track found");
        }
        for t in &self.tracks {
            let speech: f64 = t.chunks.iter().map(|c| c.len()).sum();
            let _ = writeln!(
                s,
                "track {} ({:?}): {} chunk(s), {} of speech",
                t.track + 1,
                t.role,
                t.chunks.len(),
                clock(speech)
            );
            for seg in &t.segments {
                let _ = writeln!(
                    s,
                    "  [{} - {}] {}",
                    clock(seg.start),
                    clock(seg.end),
                    seg.text
                );
            }
        }
        s
    }
}

/// h:mm:ss.t, tenths so neighbouring lines inside one chunk stay distinguishable.
fn clock(secs: f64) -> String {
    let tenths = (secs.max(0.0) * 10.0).round() as u64;
    let t = tenths / 10;
    format!(
        "{}:{:02}:{:02}.{}",
        t / 3600,
        t / 60 % 60,
        t % 60,
        tenths % 10
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_has_tenths() {
        assert_eq!(clock(3723.46), "1:02:03.5");
        assert_eq!(clock(-1.0), "0:00:00.0");
    }
}
