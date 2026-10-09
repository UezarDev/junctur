//! The human-readable summary printed by `junctur separate`.

use super::Stems;

impl Stems {
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "{}", self.file.display());
        let _ = writeln!(
            s,
            "track {}: {:.0} s separated from {:.0} s on {:?}{}",
            self.track + 1,
            self.seconds,
            self.start,
            self.device,
            if self.from_cache { " (cached)" } else { "" }
        );
        let _ = writeln!(s, "  vocals:     {}", self.vocals.display());
        let _ = writeln!(s, "  background: {}", self.background.display());
        let _ = writeln!(
            s,
            "speech: {:.0}% of the time, {} region(s)",
            self.speech_share * 100.0,
            self.speech.len()
        );
        if let Some(d) = &self.speakers {
            let (main, minor): (Vec<_>, Vec<_>) = d
                .speakers
                .iter()
                .partition(|sp| !sp.label.starts_with("minor"));
            let _ = writeln!(s, "speakers: {}", main.len());
            for sp in main {
                let _ = writeln!(
                    s,
                    "  speaker {}: {:.0} s ({:.0}%) - {}",
                    sp.id + 1,
                    sp.seconds,
                    sp.share * 100.0,
                    sp.label
                );
            }
            if !minor.is_empty() {
                let secs: f64 = minor.iter().map(|sp| sp.seconds).sum();
                let _ = writeln!(
                    s,
                    "  + {} minor cluster(s), {secs:.0} s in all (laughs, crosstalk, noise)",
                    minor.len()
                );
            }
        }
        let t = &self.timings;
        let rt = |x: f64| if x > 0.0 { self.seconds / x } else { 0.0 };
        let _ = writeln!(
            s,
            "time: separate {:.1} s ({:.0}x realtime), speech {:.1} s, speakers {:.1} s",
            t.separate_s,
            rt(t.separate_s),
            t.vad_s,
            t.diarize_s
        );
        s
    }
}
