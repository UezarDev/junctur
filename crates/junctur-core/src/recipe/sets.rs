//! Sets of seconds, as sorted spans that neither overlap nor touch: what every detector answers and
//! what a recipe's `all`, `any` and `not` combine.

use crate::spans::{joined, Join};

pub type Spans = Vec<(f64, f64)>;

/// `spans` in any order, sorted, empty ones dropped, overlapping and touching ones joined.
pub fn normal(mut spans: Spans) -> Spans {
    spans.retain(|s| s.1 > s.0);
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    joined(spans, Join::Touching)
}

/// `spans` inside `a..b`.
pub fn clip(spans: &[(f64, f64)], (a, b): (f64, f64)) -> Spans {
    spans
        .iter()
        .map(|&(s, e)| (s.max(a), e.min(b)))
        .filter(|s| s.1 > s.0)
        .collect()
}

/// What is in both (normal inputs).
pub fn and(x: &[(f64, f64)], y: &[(f64, f64)]) -> Spans {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < x.len() && j < y.len() {
        let (a, b) = (x[i].0.max(y[j].0), x[i].1.min(y[j].1));
        if b > a {
            out.push((a, b));
        }
        if x[i].1 < y[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// What is in either.
pub fn or(x: &[(f64, f64)], y: &[(f64, f64)]) -> Spans {
    normal(x.iter().chain(y).copied().collect())
}

/// `range` without `x` (normal).
pub fn not(x: &[(f64, f64)], (a, b): (f64, f64)) -> Spans {
    let mut out = Vec::new();
    let mut at = a;
    for &(s, e) in x {
        if e <= at {
            continue;
        }
        if s >= b {
            break;
        }
        if s > at {
            out.push((at, s));
        }
        at = at.max(e);
    }
    if at < b {
        out.push((at, b));
    }
    out
}

/// Each span `before` earlier and `after` later, inside `range`, joined where they meet.
pub fn padded(x: &[(f64, f64)], before: f64, after: f64, range: (f64, f64)) -> Spans {
    normal(clip(
        &x.iter()
            .map(|&(s, e)| (s - before, e + after))
            .collect::<Vec<_>>(),
        range,
    ))
}

/// Each event (its start) `before` earlier to `after` later, inside `range`, joined where they meet.
pub fn around(x: &[(f64, f64)], before: f64, after: f64, range: (f64, f64)) -> Spans {
    normal(clip(
        &x.iter()
            .map(|&(s, _)| (s - before, s + after))
            .collect::<Vec<_>>(),
        range,
    ))
}

/// Spans at least `min` seconds long.
pub fn longer(x: Spans, min: f64) -> Spans {
    x.into_iter().filter(|s| s.1 - s.0 >= min).collect()
}

/// Seconds rounded to the millisecond, as the edit list keeps them.
pub fn ms(spans: Spans) -> Spans {
    let r = |x: f64| (x * 1000.0).round() / 1000.0;
    normal(spans.into_iter().map(|(a, b)| (r(a), r(b))).collect())
}
