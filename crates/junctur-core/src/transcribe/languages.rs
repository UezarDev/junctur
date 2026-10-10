//! Which languages a recording may contain. Kept as data, not code, so adding a language is a
//! setting: Whisper detects among these only (detecting among all 99 turns accented Spanish into
//! Portuguese or Galician now and then), and the first one wins a near tie.

#[derive(Debug, Clone, PartialEq)]
pub struct Languages {
    /// Whisper language codes; the first is the primary language.
    pub allowed: Vec<String>,
}

impl Default for Languages {
    /// The user's streams: Spanish, with English mixed in.
    fn default() -> Self {
        Self {
            allowed: vec!["es".into(), "en".into()],
        }
    }
}

/// A rival must beat the primary language by this factor to win.
const PRIMARY_BIAS: f32 = 1.5;

impl Languages {
    pub fn key(&self) -> String {
        self.allowed.join("+")
    }

    /// Pick from `(code, probability)` pairs for the allowed languages.
    pub fn choose(&self, probs: &[(String, f32)]) -> String {
        let p = |c: &str| probs.iter().find(|(x, _)| x == c).map_or(0.0, |(_, p)| *p);
        let primary = &self.allowed[0];
        let best = self
            .allowed
            .iter()
            .max_by(|a, b| p(a).total_cmp(&p(b)))
            .unwrap_or(primary);
        if p(best) > p(primary) * PRIMARY_BIAS {
            best.clone()
        } else {
            primary.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probs(es: f32, en: f32) -> Vec<(String, f32)> {
        vec![("es".into(), es), ("en".into(), en)]
    }

    #[test]
    fn primary_wins_near_ties() {
        let l = Languages::default();
        assert_eq!(l.choose(&probs(0.45, 0.55)), "es");
        assert_eq!(l.choose(&probs(0.1, 0.9)), "en");
        assert_eq!(l.choose(&[]), "es");
    }
}
