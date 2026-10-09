//! The human-readable report printed by `junctur tracks`.

use super::TrackReport;

impl TrackReport {
    /// Human-readable report, tracks numbered 1-based as OBS does.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "{}", self.file.display());
        let _ = writeln!(
            s,
            "duration {}, {} audio track(s), sampled {} x {:.0}s window(s){}",
            hms(self.duration),
            self.tracks.len(),
            self.windows.len(),
            self.window_seconds,
            if self.from_cache { " (cached)" } else { "" }
        );
        for t in &self.tracks {
            let title = t
                .title
                .as_ref()
                .map(|x| format!(" \"{x}\""))
                .unwrap_or_default();
            let _ = writeln!(
                s,
                "  track {}: {} {}ch {} Hz{title}",
                t.index + 1,
                t.codec,
                t.channels,
                t.sample_rate
            );
        }
        if let Some(e) = &self.speech_error {
            let _ = writeln!(
                s,
                "speech detection unavailable, roles from loudness only: {e}"
            );
        }
        let _ = writeln!(s, "logical tracks:");
        for l in &self.logical {
            let names: Vec<String> = l.members.iter().map(|m| (m + 1).to_string()).collect();
            let _ = writeln!(
                s,
                "  [{}] {:?} ({:.0}%)  mean {:.1} dB, peak {:.1} dB, near-silent {:.0}%",
                names.join("="),
                l.role,
                l.confidence * 100.0,
                l.activity.mean_db,
                l.activity.peak_db,
                l.activity.near_silent_share * 100.0
            );
            for r in &l.reasons {
                let _ = writeln!(s, "      - {r}");
            }
        }
        s
    }
}

fn hms(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}
