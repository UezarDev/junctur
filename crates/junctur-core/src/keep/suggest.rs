//! Laya's choices as suggestions: a part whose decision differs from what the edit list does with it,
//! where the list still does what the automatic reduction did (a hand edit wins) and the user has not
//! answered it. Accepting one is an edit of the part; dismissing writes none. An accepted part stays
//! listed so it can be undone alone.

use serde::Serialize;

use super::fit::Fit;
use super::parts::{Kind, Part};
use super::{Answer, Choices, How, Judged};
use crate::edit::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    /// Laya would do otherwise than the list does: waiting for the user.
    Suggested,
    /// The user took Laya's choice: it can be undone.
    Accepted,
}

/// A part the editor shows in the Suggestions lane.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub start: f64,
    pub end: f64,
    pub kind: Kind,
    /// Sigmoid of the margin at the fit: Laya's calibrated P(keep), or how sure the rules are (0.5 at
    /// the line to 0.98 at the top), which is no chance (3e).
    pub p: f64,
    /// Laya's choice: keep (true) or cut.
    pub keep: bool,
    pub uncertain: bool,
    pub state: ItemState,
}

/// How many suggestions wait, and how long they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub cut: usize,
    pub cut_s: f64,
    pub keep: usize,
    pub keep_s: f64,
    pub uncertain: usize,
    pub accepted: usize,
}

/// The suggestions and accepted parts of `j`, in time order, with their summary. `now` is the edit
/// list's kept ranges, `auto` what the automatic reduction keeps (both sorted). Only answers the
/// list still does count ([`Choices::standing`]).
pub fn suggestions(
    j: &Judged,
    fit: &Fit,
    now: &[Range],
    auto: &[Range],
    choices: &Choices,
) -> (Vec<Item>, Summary) {
    let mut items = Vec::new();
    let mut sum = Summary::default();
    for x in &j.parts {
        let part = x.part();
        // An accepted part is what the user took, even when a refit would now choose otherwise.
        let (state, keep) = match choices.standing(&part, now) {
            Some(a) if a.how == How::Accepted => (ItemState::Accepted, a.keep),
            Some(_) => continue,
            None => {
                let keep = fit.keeps(x.margin);
                let kept = part.kept_by(now);
                if kept != part.kept_by(auto) || kept == keep {
                    continue;
                }
                (ItemState::Suggested, keep)
            }
        };
        let item = Item {
            start: x.start,
            end: x.end,
            kind: x.kind,
            p: fit.p(x.margin),
            keep,
            uncertain: fit.uncertain(x.margin),
            state,
        };
        match (state, keep) {
            (ItemState::Accepted, _) => sum.accepted += 1,
            (ItemState::Suggested, true) => {
                sum.keep += 1;
                sum.keep_s += part.len();
            }
            (ItemState::Suggested, false) => {
                sum.cut += 1;
                sum.cut_s += part.len();
            }
        }
        if state == ItemState::Suggested && item.uncertain {
            sum.uncertain += 1;
        }
        items.push(item);
    }
    (items, sum)
}

/// The span an edit of `item` covers. A cut of talk covers the automatic cut's kept ranges (`auto`,
/// sorted) that reach into it, held out of other talk: those are its speech with the pads kept
/// around it, and a cut of the part alone would leave slivers of silence kept either side. It stops at
/// a silent neighbour that is the user's (the list does otherwise than the automatic cut with it, or
/// an answer of theirs stands for it): their edit wins. Anything else is the part itself.
fn span(item: &Item, j: &Judged, auto: &[Range], now: &[Range], choices: &Choices) -> (f64, f64) {
    if item.keep || item.kind != Kind::Talk {
        return (item.start, item.end);
    }
    let at = j.parts.partition_point(|p| p.start < item.start - 1e-6);
    let theirs = |p: &Part| p.kept_by(now) != p.kept_by(auto) || choices.standing(p, now).is_some();
    let near = |k: Option<usize>| {
        k.and_then(|k| j.parts.get(k))
            .map(|x| x.part())
            .filter(|p| !theirs(p))
    };
    let lo = near(at.checked_sub(1))
        .filter(|p| p.kind == Kind::Silent)
        .map_or(item.start, |p| p.start);
    let hi = near(Some(at + 1))
        .filter(|p| p.kind == Kind::Silent)
        .map_or(item.end, |p| p.end);
    let from = auto.partition_point(|r| r.end <= item.start);
    let reach = auto[from..].iter().take_while(|r| r.start < item.end);
    let (a, b) = reach.fold((item.start, item.end), |(a, b), r| {
        (a.min(r.start), b.max(r.end))
    });
    (a.max(lo), b.min(hi))
}

/// The edits that accept `items` (start, end, keep), cuts first so a kept part next to a cut one
/// stays whole, and their answers recorded in `choices`. Only waiting suggestions are taken. `bulk`:
/// Accept all, whose answers are no training pairs ([`super::Answer::bulk`]).
pub fn accept(
    items: &[Item],
    j: &Judged,
    (auto, now): (&[Range], &[Range]),
    choices: &mut Choices,
    bulk: bool,
) -> Vec<(f64, f64, bool)> {
    let mut edits = Vec::new();
    for i in items.iter().filter(|i| i.state == ItemState::Suggested) {
        let (a, b) = span(i, j, auto, now, choices);
        edits.push((a, b, i.keep));
        choices.set(Answer {
            start: i.start,
            end: i.end,
            keep: i.keep,
            how: How::Accepted,
            bulk,
            spans: super::rules::SPANS,
        });
    }
    edits.sort_by_key(|e| e.2);
    edits
}

/// Turns down a waiting suggestion: no edit, the list's own choice recorded as the user's.
pub fn dismiss(item: &Item, choices: &mut Choices) {
    choices.set(Answer {
        start: item.start,
        end: item.end,
        keep: !item.keep,
        how: How::Dismissed,
        bulk: false,
        spans: super::rules::SPANS,
    });
}

/// Undoes an accepted part alone, the answer turned down: the span its accept edited is cut, then
/// what the automatic cut (`auto`, sorted) keeps of it is kept again, which is what the list did
/// there before (a suggestion is only made where the list does what the automatic cut does).
pub fn undo(
    item: &Item,
    j: &Judged,
    (auto, now): (&[Range], &[Range]),
    choices: &mut Choices,
) -> Vec<(f64, f64, bool)> {
    if item.state != ItemState::Accepted {
        return Vec::new();
    }
    let (a, b) = span(item, j, auto, now, choices);
    dismiss(item, choices);
    let mut edits = vec![(a, b, false)];
    let from = auto.partition_point(|r| r.end <= a);
    edits.extend(
        auto[from..]
            .iter()
            .take_while(|r| r.start < b)
            .map(|r| (r.start.max(a), r.end.min(b), true)),
    );
    edits
}
