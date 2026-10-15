//! Joining spans that come in sorted by start: the one loop the cuts, speech, pins and frames all
//! use, with the rule for how near the next must start named at each call.

use std::ops::Sub;

/// A span's ends, in seconds or frames.
pub trait Span {
    type At: Copy + PartialOrd + Sub<Output = Self::At>;
    fn start(&self) -> Self::At;
    fn end(&self) -> Self::At;
    /// Moves the end out to `end` when that is later.
    fn reach(&mut self, end: Self::At);
}

/// When the next span joins the one before.
#[derive(Debug, Clone, Copy)]
pub enum Join<A> {
    /// It starts at or before the other's end.
    Touching,
    /// It starts less than this after the other's end.
    Within(A),
}

impl<A: Copy + PartialOrd + Sub<Output = A>> Join<A> {
    fn joins(self, end: A, start: A) -> bool {
        match self {
            Join::Touching => start <= end,
            Join::Within(gap) => start - end < gap,
        }
    }
}

/// Adds `s` to `out` (sorted by start), joined onto the last span when `join` says so.
pub fn push_joined<S: Span>(out: &mut Vec<S>, s: S, join: Join<S::At>) {
    match out.last_mut() {
        Some(last) if join.joins(last.end(), s.start()) => last.reach(s.end()),
        _ => out.push(s),
    }
}

/// `sorted` (by start) with the spans `join` puts together as one.
pub fn joined<S: Span>(sorted: impl IntoIterator<Item = S>, join: Join<S::At>) -> Vec<S> {
    let mut out = Vec::new();
    for s in sorted {
        push_joined(&mut out, s, join);
    }
    out
}

impl Span for (f64, f64) {
    type At = f64;
    fn start(&self) -> f64 {
        self.0
    }
    fn end(&self) -> f64 {
        self.1
    }
    fn reach(&mut self, end: f64) {
        self.1 = self.1.max(end);
    }
}

impl Span for (u64, u64) {
    type At = u64;
    fn start(&self) -> u64 {
        self.0
    }
    fn end(&self) -> u64 {
        self.1
    }
    fn reach(&mut self, end: u64) {
        self.1 = self.1.max(end);
    }
}

impl Span for crate::vad::Region {
    type At = f64;
    fn start(&self) -> f64 {
        self.start
    }
    fn end(&self) -> f64 {
        self.end
    }
    fn reach(&mut self, end: f64) {
        self.end = self.end.max(end);
    }
}

impl Span for crate::edit::Range {
    type At = f64;
    fn start(&self) -> f64 {
        self.start
    }
    fn end(&self) -> f64 {
        self.end
    }
    fn reach(&mut self, end: f64) {
        self.end = self.end.max(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_and_within_a_gap() {
        let s = [(0.0, 1.0), (1.0, 2.0), (2.25, 3.0), (2.5, 2.75), (3.5, 4.0)];
        let apart = [(0.0, 2.0), (2.25, 3.0), (3.5, 4.0)];
        assert_eq!(joined(s, Join::Touching), apart);
        assert_eq!(joined(s, Join::Within(0.5)), [(0.0, 3.0), (3.5, 4.0)]);
        assert_eq!(joined(s, Join::Within(0.25)), apart);
        assert_eq!(
            joined([(0u64, 3), (3, 5), (6, 7)], Join::Touching),
            [(0, 5), (6, 7)]
        );
        assert!(joined(Vec::<(f64, f64)>::new(), Join::Touching).is_empty());
    }
}
