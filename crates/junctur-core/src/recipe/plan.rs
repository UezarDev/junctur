//! Running a recipe over a stretch: each tool asked of the detectors, combined, and turned into the
//! ghost cuts and keeps (or the markers and clips) its action makes.

use super::sets::{self, Spans};
use super::{Action, Cond, Recipe};
use crate::Result;

/// The detectors a recipe's tools ask, each answering for `range` (source seconds) with the spans
/// where it holds, sorted and apart ([`sets::normal`]). An `Err` is a tool that cannot answer here
/// (no transcript yet, no example marked, a speaker nobody is called), said to the user as it is.
pub trait Signals {
    fn silence(&mut self, range: (f64, f64)) -> Result<Spans>;
    fn still(&mut self, range: (f64, f64)) -> Result<Spans>;
    fn speaker(&mut self, name: &str, range: (f64, f64)) -> Result<Spans>;
    /// Where `phrase` is said: each saying alone, or (`remark`) the whole remark it is said in, from
    /// the pause before to the pause after, which is what "what X says about Y" means.
    fn words(&mut self, phrase: &str, remark: bool, range: (f64, f64)) -> Result<Spans>;
    /// Each match of the examples marked in the Like this tab, from its point.
    fn like_this(&mut self, range: (f64, f64)) -> Result<Spans>;
    fn label(&mut self, picture: &str, range: (f64, f64)) -> Result<Spans>;
}

/// What a recipe makes of a stretch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Ghost cuts and keeps, sorted and apart; a keep only where an action keeps (the rest cut).
    pub cuts: Spans,
    pub keeps: Spans,
    /// Markers ([`Action::Mark`]) and clips ([`Action::ClipEach`]) for the Moments list.
    pub marks: Vec<f64>,
    pub clips: Spans,
    /// The parts or events the condition found (before `min_seconds`).
    pub found: usize,
}

fn eval(c: &Cond, events: bool, range: (f64, f64), s: &mut dyn Signals) -> Result<Spans> {
    let got = match c {
        Cond::Silence => s.silence(range)?,
        Cond::StillPicture => s.still(range)?,
        Cond::Speaker(n) => s.speaker(n, range)?,
        Cond::Words(p) => s.words(p, !events, range)?,
        Cond::LikeThis(_) => s.like_this(range)?,
        Cond::Label(p) => s.label(p, range)?,
        Cond::All(v) => {
            let mut acc: Option<Spans> = None;
            for c in v {
                let x = eval(c, events, range, s)?;
                acc = Some(match acc {
                    None => x,
                    Some(a) => sets::and(&a, &x),
                });
            }
            acc.unwrap_or_default()
        }
        Cond::Any(v) => {
            let mut acc = Spans::new();
            for c in v {
                acc = sets::or(&acc, &eval(c, events, range, s)?);
            }
            acc
        }
        Cond::Not(c) => {
            let x = eval(c, events, range, s)?;
            sets::not(&x, range)
        }
    };
    Ok(sets::clip(&sets::normal(got), range))
}

/// `recipe` over source seconds `range` (`a < b`) with the detectors' answers.
pub fn plan(recipe: &Recipe, range: (f64, f64), s: &mut dyn Signals) -> Result<Plan> {
    let events = !recipe.action.over_parts();
    let found = eval(&recipe.when, events, range, s)?;
    let mut p = Plan {
        found: found.len(),
        ..Plan::default()
    };
    let (before, after) = recipe.around();
    match recipe.action {
        Action::CutWhere => p.cuts = sets::longer(found, recipe.min_seconds()),
        Action::KeepWhere => {
            p.keeps = sets::longer(found, recipe.min_seconds());
            p.cuts = sets::not(&p.keeps, range);
        }
        Action::KeepAround => {
            p.keeps = sets::around(&found, before, after, range);
            p.cuts = sets::not(&p.keeps, range);
        }
        Action::CutAround => p.cuts = sets::around(&found, before, after, range),
        Action::Mark => p.marks = found.iter().map(|s| s.0).collect(),
        Action::ClipEach => p.clips = sets::padded(&found, before, after, range),
    }
    p.cuts = sets::ms(p.cuts);
    p.keeps = sets::ms(p.keeps);
    p.clips = sets::ms(p.clips);
    Ok(p)
}
