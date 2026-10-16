//! "Ask, scrub, accept" (phase 7c, a prototype): a request typed over the selection (or the whole
//! recording) becomes ghost cuts and keeps the user scrubs and accepts, one step at a time. The
//! request is read as a recipe ([`crate::recipe`], by [`crate::llm`] or the keywords), the recipe
//! is run over the stretch with the detectors ([`signals`]), and what it would change of the edit
//! list as it is now waits as ghosts; nothing changes the list until they are accepted, and an
//! accepted batch keeps what the list had there, so its undo puts back exactly that (as 7a's moments
//! and 7b's like-this edits do). Markers and clips go to the Moments list instead.
//!
//! The history of requests is [`FILE`] in the project folder: each step's request, recipe, reader,
//! ghosts and what was accepted, so "next edit" stacks and a reopened recording shows them again.

pub mod look;
pub mod signals;
pub mod still;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::edit::{cut_span, keep_span, kept_in, EditList, Range};
use crate::llm::Reading;
use crate::moments::{Marker, Source};
use crate::pipeline::Project;
use crate::recipe::{self, Recipe};
use crate::{Error, Result};

/// The project file.
pub const FILE: &str = "ask.json";
/// Steps kept in the history; the oldest go first.
const MOST_STEPS: usize = 50;

/// A ghost: an edit waiting for Accept, in source seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ghost {
    pub start: f64,
    pub end: f64,
    /// Keeps what is cut there now; else cuts what is kept there.
    pub keep: bool,
    /// What the list kept over it when it was asked: a ghost whose stretch the list changed under
    /// since (an edit by hand, another request accepted) is stale, and is not applied: it would
    /// undo that edit.
    #[serde(default)]
    pub was: Vec<Range>,
}

impl Ghost {
    /// The list `now` keeps over it what it kept when it was asked.
    pub fn fresh(&self, now: &[Range]) -> bool {
        kept_in(now, self.start, self.end) == self.was
    }
}

/// Ghosts accepted together, with what the list kept over them before, for its undo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub id: u64,
    pub ghosts: Vec<Ghost>,
    pub before: Vec<Range>,
}

impl Applied {
    fn hull(&self) -> (f64, f64) {
        hull(&self.ghosts)
    }
}

fn hull(g: &[Ghost]) -> (f64, f64) {
    g.iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), g| {
            (a.min(g.start), b.max(g.end))
        })
}

/// One request and what came of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: u64,
    pub request: String,
    /// The selection it was asked over; `None`: the whole recording.
    pub range: Option<(f64, f64)>,
    /// The recipe as JSON ([`recipe::from_json`]); `None` when the request did not read.
    pub recipe: Option<serde_json::Value>,
    /// The recipe in plain words.
    pub described: String,
    pub reading: Reading,
    /// Why the recipe made nothing (a tool that cannot answer here), or why the request did not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Parts or events the recipe's condition found.
    pub found: usize,
    /// Waiting for the user's answer.
    pub ghosts: Vec<Ghost>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<(f64, f64)>,
    /// The marks or clips were sent to the Moments list.
    #[serde(default)]
    pub sent: bool,
    pub applied: Vec<Applied>,
    /// Ghosts the user turned down.
    pub rejected: usize,
    /// Seconds the detectors took, and what they read for the first time.
    pub seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Step {
    /// The recipe, read back.
    pub fn recipe(&self) -> Option<Recipe> {
        self.recipe.as_ref().and_then(|v| recipe::from_json(v).ok())
    }

    pub fn accepted(&self) -> usize {
        self.applied.iter().map(|a| a.ghosts.len()).sum()
    }
}

/// [`FILE`]: every request asked of this recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Ask {
    pub version: u32,
    pub steps: Vec<Step>,
    pub next_id: u64,
}

impl Default for Ask {
    fn default() -> Self {
        Self {
            version: 1,
            steps: Vec::new(),
            next_id: 1,
        }
    }
}

fn take_id(next: &mut u64) -> u64 {
    let id = (*next).max(1);
    *next = id + 1;
    id
}

impl Ask {
    /// Adds `step` (its id given here) to the history, the oldest beyond [`MOST_STEPS`] dropped;
    /// its id.
    pub fn push(&mut self, mut step: Step) -> u64 {
        step.id = take_id(&mut self.next_id);
        let id = step.id;
        self.steps.push(step);
        if self.steps.len() > MOST_STEPS {
            let extra = self.steps.len() - MOST_STEPS;
            self.steps.drain(..extra);
        }
        id
    }

    pub fn step(&self, id: u64) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    fn step_mut(&mut self, id: u64) -> Result<&mut Step> {
        self.steps
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::Recipe("that request is no longer in the history".into()))
    }

    /// Accepts step `id`'s waiting ghosts (those at `which`, or all) against the list's kept ranges
    /// `now`: their edits, in order, recorded with what `now` keeps over them. Stale ones
    /// ([`Ghost::fresh`]) are dropped, counted as rejected, and refused when none is left. Marks and
    /// clips are sent to the Moments list instead (no edit).
    pub fn accept(
        &mut self,
        id: u64,
        which: Option<&[usize]>,
        now: &[Range],
    ) -> Result<Vec<(f64, f64, bool)>> {
        let applied_id = take_id(&mut self.next_id);
        let s = self.step_mut(id)?;
        if s.ghosts.is_empty() {
            if s.marks.is_empty() && s.clips.is_empty() {
                return Err(Error::Recipe("nothing is waiting in that request".into()));
            }
            if s.sent {
                return Err(Error::Recipe("those were sent to Moments already".into()));
            }
            s.sent = true;
            return Ok(Vec::new());
        }
        let (taken, left): (Vec<_>, Vec<_>) = s
            .ghosts
            .iter()
            .cloned()
            .enumerate()
            .partition(|(i, _)| which.is_none_or(|w| w.contains(i)));
        if taken.is_empty() {
            return Err(Error::Recipe("those ghosts are no longer waiting".into()));
        }
        let (ghosts, stale): (Vec<Ghost>, Vec<Ghost>) =
            taken.into_iter().map(|x| x.1).partition(|g| g.fresh(now));
        s.ghosts = left.into_iter().map(|x| x.1).collect();
        s.rejected += stale.len();
        if ghosts.is_empty() {
            return Err(Error::Recipe(
                "the list changed there since this was asked, so it was dropped: ask again".into(),
            ));
        }
        let (a, b) = hull(&ghosts);
        let edits = ghosts.iter().map(|g| (g.start, g.end, g.keep)).collect();
        s.applied.push(Applied {
            id: applied_id,
            before: kept_in(now, a, b),
            ghosts,
        });
        Ok(edits)
    }

    /// Turns down step `id`'s ghosts at `which`, or all of them (and its marks and clips).
    pub fn reject(&mut self, id: u64, which: Option<&[usize]>) -> Result<()> {
        let s = self.step_mut(id)?;
        let before = s.ghosts.len();
        let mut i = 0;
        s.ghosts.retain(|_| {
            let keep = which.is_some_and(|w| !w.contains(&i));
            i += 1;
            keep
        });
        s.rejected += before - s.ghosts.len();
        if which.is_none() && !s.sent {
            s.rejected += s.marks.len() + s.clips.len();
            s.marks.clear();
            s.clips.clear();
        }
        Ok(())
    }

    /// The edits that undo applied batch `applied` of step `id` (its hull cut, then what was kept
    /// there kept again), and it forgotten; or a sent batch of marks taken back out of Moments.
    /// Refused while a batch accepted after it reaches into its stretch: putting back what was kept
    /// before would take that one back too (undo the later one first).
    pub fn undo(&mut self, id: u64, applied: Option<u64>) -> Result<Vec<(f64, f64, bool)>> {
        if let Some(a) = applied {
            let this = self
                .steps
                .iter()
                .flat_map(|s| &s.applied)
                .find(|x| x.id == a)
                .map(Applied::hull);
            if let Some((from, to)) = this {
                let later = self
                    .steps
                    .iter()
                    .flat_map(|s| &s.applied)
                    .any(|x| x.id > a && x.hull().0 < to && x.hull().1 > from);
                if later {
                    return Err(Error::Recipe(
                        "an edit accepted after this one reaches into it: undo that one first"
                            .into(),
                    ));
                }
            }
        }
        let s = self.step_mut(id)?;
        let Some(applied) = applied else {
            if !s.sent {
                return Err(Error::Recipe("nothing of that request to undo".into()));
            }
            s.sent = false;
            return Ok(Vec::new());
        };
        let i = s
            .applied
            .iter()
            .position(|a| a.id == applied)
            .ok_or_else(|| Error::Recipe("that edit is no longer there to undo".into()))?;
        let a = s.applied.remove(i);
        let (from, to) = a.hull();
        Ok(std::iter::once((from, to, false))
            .chain(a.before.iter().map(|r| (r.start, r.end, true)))
            .collect())
    }

    /// The kept ranges `now` would become with every waiting ghost accepted, oldest step first:
    /// what the preview plays.
    pub fn preview(&self, now: &[Range]) -> Vec<Range> {
        let mut keep = now.to_vec();
        for g in self
            .steps
            .iter()
            .flat_map(|s| &s.ghosts)
            .filter(|g| g.fresh(now))
        {
            keep = if g.keep {
                keep_span(&keep, g.start, g.end, 0.0)
            } else {
                cut_span(&keep, g.start, g.end, 0.0)
            };
        }
        keep
    }

    /// Whether any ghost waits.
    pub fn waiting(&self) -> bool {
        self.steps.iter().any(|s| !s.ghosts.is_empty())
    }

    /// The Moments list's markers of every sent step.
    pub fn markers(&self) -> Vec<Marker> {
        let mut out = Vec::new();
        for s in self.steps.iter().filter(|s| s.sent) {
            let text = || format!("Asked: {}", s.request);
            out.extend(s.marks.iter().map(|&at| Marker {
                at,
                source: Source::Ask,
                cut: None,
                text: text(),
                span: None,
            }));
            out.extend(s.clips.iter().map(|&(a, b)| Marker {
                at: a,
                source: Source::Ask,
                cut: None,
                text: text(),
                span: Some((a, b)),
            }));
        }
        out
    }
}

/// The ghosts of `plan` that would change `now` (sorted kept ranges): a cut only where something is
/// kept, a keep only where something is cut.
pub fn ghosts(plan: &recipe::Plan, now: &[Range]) -> Vec<Ghost> {
    let kept = |a: f64, b: f64| kept_in(now, a, b).iter().map(Range::len).sum::<f64>();
    let mut out: Vec<Ghost> = plan
        .cuts
        .iter()
        .filter(|&&(a, b)| kept(a, b) > 0.0005)
        .map(|&(start, end)| Ghost {
            start,
            end,
            keep: false,
            was: kept_in(now, start, end),
        })
        .chain(
            plan.keeps
                .iter()
                .filter(|&&(a, b)| kept(a, b) < b - a - 0.0005)
                .map(|&(start, end)| Ghost {
                    start,
                    end,
                    keep: true,
                    was: kept_in(now, start, end),
                }),
        )
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The speakers' names of `list`, for the translators.
pub fn speakers(list: &EditList) -> Vec<String> {
    list.speakers.names.iter().map(|n| n.name.clone()).collect()
}

/// `request`, read as `read` (or not: the reason), run over `range` of project `p` (the whole
/// recording when `None`) against `list`: the history step, its ghosts waiting. A tool that cannot
/// answer is the step's error, not a failure: the history shows what was asked and why it made
/// nothing.
pub fn propose(
    p: &Project,
    list: &EditList,
    request: &str,
    range: Option<(f64, f64)>,
    read: crate::llm::Read,
) -> Step {
    let started = std::time::Instant::now();
    let mut step = Step {
        id: 0,
        request: request.trim().to_string(),
        range,
        recipe: None,
        described: String::new(),
        reading: read.reading,
        error: None,
        found: 0,
        ghosts: Vec::new(),
        marks: Vec::new(),
        clips: Vec::new(),
        sent: false,
        applied: Vec::new(),
        rejected: 0,
        seconds: 0.0,
        notes: Vec::new(),
    };
    let recipe = match read.recipe {
        Ok(r) => r,
        Err(why) => {
            step.error = Some(why);
            return step;
        }
    };
    step.recipe = Some(recipe.to_json());
    step.described = recipe.describe();
    let whole = (0.0, list.duration);
    let stretch = range
        .map(|(a, b)| (a.min(b).max(0.0), a.max(b).min(list.duration)))
        .filter(|(a, b)| b > a)
        .unwrap_or(whole);
    let mut d = signals::Detectors {
        p,
        list,
        notes: Vec::new(),
        segments: None,
    };
    match recipe::plan(&recipe, stretch, &mut d) {
        Ok(plan) => {
            step.found = plan.found;
            step.ghosts = ghosts(&plan, &list.keep);
            step.marks = plan.marks;
            step.clips = plan.clips;
            if step.ghosts.is_empty() && step.marks.is_empty() && step.clips.is_empty() {
                step.error = Some(if plan.found == 0 {
                    "nothing in the stretch matches it".into()
                } else {
                    "it matches, but the edit list already is that way there".into()
                });
            }
        }
        Err(e) => step.error = Some(e.to_string()),
    }
    step.notes = d.notes;
    step.seconds = (started.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    step
}

pub fn load(dir: &Path) -> Result<Ask> {
    match std::fs::read(dir.join(FILE)) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ask::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(dir: &Path, a: &Ask) -> Result<()> {
    crate::output::write_replacing(&dir.join(FILE), serde_json::to_vec_pretty(a)?)?;
    Ok(())
}

#[cfg(test)]
mod tests;
