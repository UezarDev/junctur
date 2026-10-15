//! Learning from the user's decisions, the part of it that needs no GPU: a cutoff and a temperature
//! for Laya, fitted to the pairs. The cutoff is Laya's own P(keep) at which this user's answer turns
//! from cut to keep (Laya out of the box leans one way); the temperature how sure its margins are.
//! The chance the windows show is the calibrated one, sigmoid((margin - m0) / T) with m0 the
//! cutoff's margin, so a part is suggested kept at one half or more whatever the fit. A real
//! fine-tune of Laya is Phase 4's.
//!
//! Rejected: P = sigmoid(margin / T) compared with the cutoff on that scale. With a sharp fit the
//! cutoff lands at a millionth, so the windows' "chance" says nothing and no clamp keeps it right.

use serde::{Deserialize, Serialize};

use super::laya::p_keep;

/// Fewer pairs, or fewer of either answer, keep the defaults: two numbers from a handful of
/// decisions would swing with every click.
pub const MIN_PAIRS: usize = 20;
pub const MIN_EACH: usize = 5;
/// "Uncertain": a calibrated chance this close to one half.
pub const UNCERTAIN: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Fit {
    pub temperature: f64,
    /// Laya's own P(keep) (temperature 1) at which the decision turns, as shown.
    pub cutoff: f64,
    /// The margin at which it turns, as fitted: what decides, kept rather than read back from the
    /// cutoff, which saturates past a margin of about 14.
    pub turn: f64,
    /// Pairs it was fitted on; 0 for the defaults.
    pub pairs: usize,
}

impl Default for Fit {
    fn default() -> Self {
        Self {
            temperature: 1.0,
            cutoff: 0.5,
            turn: 0.0,
            pairs: 0,
        }
    }
}

impl Fit {
    /// The calibrated chance a part with `margin` stays.
    pub fn p(&self, margin: f64) -> f64 {
        p_keep(margin - self.turn, self.temperature)
    }

    pub fn keeps(&self, margin: f64) -> bool {
        margin >= self.turn
    }

    pub fn uncertain(&self, margin: f64) -> bool {
        (self.p(margin) - 0.5).abs() < UNCERTAIN
    }
}

/// The fit of `pairs` (margin, kept): a logistic regression `a·m + b` of the answer on the margin,
/// read as T = 1/a (held to 0.05..20) and the turning margin -b/a, the decision's, whatever T was
/// held to. The defaults when there are too few pairs, or when
/// the slope comes out negative or flat (Laya reads this user backwards: no temperature makes that
/// right).
pub fn fit(pairs: &[(f64, bool)]) -> Fit {
    let kept = pairs.iter().filter(|p| p.1).count();
    if pairs.len() < MIN_PAIRS || kept < MIN_EACH || pairs.len() - kept < MIN_EACH {
        return Fit::default();
    }
    let (a, b) = logistic(pairs);
    if a.is_nan() || a <= 1e-3 || !b.is_finite() {
        return Fit::default();
    }
    let turn = -b / a;
    Fit {
        temperature: (1.0 / a).clamp(0.05, 20.0),
        cutoff: p_keep(turn, 1.0),
        turn,
        pairs: pairs.len(),
    }
}

/// `a·m + b` fitted on the margins standardised (mean 0, spread 1), then mapped back: Newton from
/// a = 1, b = 0 on margins far from 0 starts saturated and runs off.
fn logistic(pairs: &[(f64, bool)]) -> (f64, f64) {
    let n = pairs.len() as f64;
    let mean = pairs.iter().map(|p| p.0).sum::<f64>() / n;
    let spread = (pairs.iter().map(|p| (p.0 - mean).powi(2)).sum::<f64>() / n)
        .sqrt()
        .max(1e-6);
    let z: Vec<(f64, bool)> = pairs
        .iter()
        .map(|&(m, y)| ((m - mean) / spread, y))
        .collect();
    let (a, b) = newton(&z);
    (a / spread, b - a * mean / spread)
}

/// Newton's method on the log loss of sigmoid(a·m + b), with a small ridge on `a` so a set the margin
/// splits perfectly still has an answer.
fn newton(pairs: &[(f64, bool)]) -> (f64, f64) {
    const RIDGE: f64 = 1e-3;
    let (mut a, mut b) = (1.0f64, 0.0f64);
    for _ in 0..50 {
        let (mut ga, mut gb, mut haa, mut hab, mut hbb) = (RIDGE * a, 0.0, RIDGE, 0.0, 0.0);
        for &(m, y) in pairs {
            let p = 1.0 / (1.0 + (-(a * m + b)).exp());
            let e = p - if y { 1.0 } else { 0.0 };
            let w = (p * (1.0 - p)).max(1e-9);
            ga += e * m;
            gb += e;
            haa += w * m * m;
            hab += w * m;
            hbb += w;
        }
        let det = haa * hbb - hab * hab;
        if det.abs() < 1e-12 {
            break;
        }
        let da = (hbb * ga - hab * gb) / det;
        let db = (haa * gb - hab * ga) / det;
        a -= da;
        b -= db;
        if da.abs() + db.abs() < 1e-9 {
            break;
        }
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_pairs_or_answers_keep_the_defaults() {
        assert_eq!(fit(&[]), Fit::default());
        let one_sided: Vec<(f64, bool)> = (0..40).map(|i| (i as f64, i < 3)).collect();
        assert_eq!(fit(&one_sided), Fit::default());
    }

    #[test]
    fn an_overconfident_laya_shifted_toward_cut_is_corrected() {
        // The user keeps a part when the margin is over about -2: Laya leans cut. Spread by up to
        // 1.5 either way, so the answer near the edge is a matter of chance.
        let pairs: Vec<(f64, bool)> = (0..200)
            .map(|i| {
                let m = -10.0 + i as f64 * 0.1;
                let noise = ((i * 37) % 11) as f64 * 0.3 - 1.5;
                (m, m + noise > -2.0)
            })
            .collect();
        let f = fit(&pairs);
        assert_eq!(f.pairs, 200);
        // The decision turns near m = -2: Laya's own chance there is the cutoff, the calibrated one
        // a half.
        assert!((f.p(-2.0) - 0.5).abs() < 0.1, "{f:?}");
        assert!((f.cutoff - p_keep(-2.0, 1.0)).abs() < 0.05, "{f:?}");
        assert!(f.keeps(0.0) && !f.keeps(-4.0), "{f:?}");
        assert!(f.uncertain(-2.0) && !f.uncertain(-8.0), "{f:?}");
    }

    #[test]
    fn a_turn_far_out_still_decides_where_it_was_fitted() {
        // The user keeps only parts Laya is all but sure of: the turn is past a margin of 14, where
        // a cutoff read back as a chance would have saturated.
        let pairs: Vec<(f64, bool)> = (0..100)
            .map(|i| {
                let m = i as f64 * 0.4;
                (m, m + ((i * 37) % 11) as f64 * 0.3 - 1.5 > 20.0)
            })
            .collect();
        let f = fit(&pairs);
        assert!(f.turn > 15.0 && f.turn < 25.0, "{f:?}");
        assert!(!f.keeps(16.0) || f.turn <= 16.0);
        assert!(f.keeps(30.0) && !f.keeps(10.0), "{f:?}");
        assert!((f.p(f.turn) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_backwards_laya_keeps_the_defaults() {
        let pairs: Vec<(f64, bool)> = (0..60).map(|i| (i as f64 - 30.0, i < 30)).collect();
        assert_eq!(fit(&pairs), Fit::default());
    }
}
