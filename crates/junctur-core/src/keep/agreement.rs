//! How often Laya agrees with an edit list (`junctur judge --agreement`, the phase's first test): a
//! part's answer is what the list does with it. Before and after a fit on every other part, scored on
//! the parts it was not fitted on, beside always keeping.

use serde::Serialize;

use super::fit::{self, Fit};
use super::parts::Kind;
use super::Judged;
use crate::edit::Range;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Score {
    /// Parts in the set, and kept by the list.
    pub parts: usize,
    pub kept: usize,
    /// Share of agreement over every part: always keeping, Laya at temperature 1 and cutoff 0.5.
    pub always_keep: f64,
    pub laya: f64,
    /// On the held-out half (the odd parts in time order): the same two, and Laya fitted on the even
    /// half.
    pub held_out: usize,
    pub always_keep_held: f64,
    pub laya_held: f64,
    pub fitted_held: f64,
    pub fit: Fit,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Agreement {
    pub all: Score,
    pub talk: Score,
    pub silent: Score,
    /// Parts the list does otherwise than the automatic reduction (hand edits), and how many of them
    /// Laya at the defaults agrees with.
    pub hand: usize,
    pub hand_agree: usize,
}

/// `j` against the list's kept ranges `now`; `auto` the reduction's, for the hand edits.
pub fn agreement(j: &Judged, now: &[Range], auto: &[Range]) -> Agreement {
    let rows: Vec<(Kind, f64, bool, bool)> = j
        .parts
        .iter()
        .map(|x| {
            let p = x.part();
            (x.kind, x.margin, p.kept_by(now), p.kept_by(auto))
        })
        .collect();
    let set = |keep: &dyn Fn(Kind) -> bool| {
        let v: Vec<(f64, bool)> = rows
            .iter()
            .filter(|r| keep(r.0))
            .map(|r| (r.1, r.2))
            .collect();
        score(&v)
    };
    let d = Fit::default();
    let hand: Vec<&(Kind, f64, bool, bool)> = rows.iter().filter(|r| r.2 != r.3).collect();
    Agreement {
        all: set(&|_| true),
        talk: set(&|k| k == Kind::Talk),
        silent: set(&|k| k == Kind::Silent),
        hand: hand.len(),
        hand_agree: hand.iter().filter(|r| d.keeps(r.1) == r.2).count(),
    }
}

fn score(v: &[(f64, bool)]) -> Score {
    let share = |rows: &[(f64, bool)], f: &dyn Fn(f64) -> bool| {
        if rows.is_empty() {
            return 0.0;
        }
        rows.iter().filter(|(m, k)| f(*m) == *k).count() as f64 / rows.len() as f64
    };
    let d = Fit::default();
    let train: Vec<(f64, bool)> = v.iter().copied().step_by(2).collect();
    let held: Vec<(f64, bool)> = v.iter().copied().skip(1).step_by(2).collect();
    let fit = fit::fit(&train);
    Score {
        parts: v.len(),
        kept: v.iter().filter(|r| r.1).count(),
        always_keep: share(v, &|_| true),
        laya: share(v, &|m| d.keeps(m)),
        held_out: held.len(),
        always_keep_held: share(&held, &|_| true),
        laya_held: share(&held, &|m| d.keeps(m)),
        fitted_held: share(&held, &|m| fit.keeps(m)),
        fit,
    }
}
