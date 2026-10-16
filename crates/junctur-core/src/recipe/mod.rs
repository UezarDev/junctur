//! Recipes (phase 7c, a prototype): what a typed request ("remove the parts with silence and no
//! movement") is read as, a short program over a fixed menu of tools Junctur owns. A translator
//! (the small text model of [`crate::llm`], or the keyword reader in [`keywords`] without it) writes
//! one as JSON; [`parse`] takes only what is on the menu, refusing an unknown tool or field instead
//! of guessing; [`plan`] runs it over a stretch of the recording with the detectors' answers
//! ([`Signals`]) and gives the ghost cuts and keeps the user scrubs and accepts. The translator never
//! sees the video, so a wrong recipe only shows as wrong ghosts. A module of its own because the
//! menu, its checking and its meaning are one contract shared by both translators, the app and the
//! command line; it is pure, the detectors living in [`crate::ask`].

pub mod keywords;
mod plan;
pub mod sets;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Error, Result};

pub use plan::{plan, Plan, Signals};

/// What a recipe does with what its condition finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Cut the parts where the condition holds.
    CutWhere,
    /// Keep only those parts, cut the rest of the stretch.
    KeepWhere,
    /// Keep a little around each time it happens, cut the rest of the stretch.
    KeepAround,
    /// Cut a little around each time it happens.
    CutAround,
    /// A marker at each time it happens (the Moments list, as 7a's hotkeys).
    Mark,
    /// A clip of each time it happens (the Moments list).
    ClipEach,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::CutWhere,
        Action::KeepWhere,
        Action::KeepAround,
        Action::CutAround,
        Action::Mark,
        Action::ClipEach,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Action::CutWhere => "cut_where",
            Action::KeepWhere => "keep_where",
            Action::KeepAround => "keep_around",
            Action::CutAround => "cut_around",
            Action::Mark => "mark",
            Action::ClipEach => "clip_each",
        }
    }

    /// Parts (a span each, joined) rather than events (a point each, padded).
    pub fn over_parts(self) -> bool {
        matches!(self, Action::CutWhere | Action::KeepWhere)
    }
}

/// Where a recipe acts: a detector's answer, or answers combined.
#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    /// Nobody talks: what the automatic cut would cut ([`crate::edit::keep_ranges`]).
    Silence,
    /// The picture does not move ([`crate::ask::still`]).
    StillPicture,
    /// A speaker of the edit list talks, by name; [`ANYONE`] for any of them.
    Speaker(String),
    /// A phrase is said, as the transcript heard it.
    Words(String),
    /// A sound like the examples marked in the Like this tab (7b); the text only names it.
    LikeThis(String),
    /// The picture shows this, as SigLIP reads the frames ([`crate::labels::model`]).
    Label(String),
    All(Vec<Cond>),
    Any(Vec<Cond>),
    Not(Box<Cond>),
}

/// The speaker name that means any speaker.
pub const ANYONE: &str = "anyone";
/// Combinations nest at most this deep: past it a recipe is not one a request reads as.
const DEPTH: usize = 4;
/// The longest name, phrase or picture a condition takes.
const MAX_TEXT: usize = 100;
/// The most seconds a recipe pads or asks for.
const MAX_SECONDS: f64 = 600.0;

/// Before and after each event, when the recipe gives none: the research's quarter second for an
/// ore (7b's defaults) around an event, a couple of seconds for a clip.
pub const AROUND: (f64, f64) = (0.1, 0.15);
pub const CLIP: (f64, f64) = (2.0, 2.0);
/// A part shorter than this is not acted on when the recipe gives no `min_seconds`: a still
/// picture or a pause of a few frames is not what anyone asks to cut.
pub const MIN_PART_S: f64 = 1.0;

/// A request, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub action: Action,
    pub when: Cond,
    /// Parts shorter than this are left alone ([`Action::over_parts`] only).
    pub min_seconds: Option<f64>,
    pub before: Option<f64>,
    pub after: Option<f64>,
}

impl Recipe {
    pub fn min_seconds(&self) -> f64 {
        self.min_seconds.unwrap_or(MIN_PART_S)
    }

    /// Seconds before and after each event this recipe pads with.
    pub fn around(&self) -> (f64, f64) {
        let d = if self.action == Action::ClipEach {
            CLIP
        } else {
            AROUND
        };
        (self.before.unwrap_or(d.0), self.after.unwrap_or(d.1))
    }

    /// The recipe as JSON, the form [`parse`] reads back.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("action".into(), self.action.name().into());
        m.insert("when".into(), cond_json(&self.when));
        for (k, v) in [
            ("min_seconds", self.min_seconds),
            ("before", self.before),
            ("after", self.after),
        ] {
            if let Some(v) = v {
                m.insert(k.into(), v.into());
            }
        }
        Value::Object(m)
    }

    /// The recipe in plain words, as the app shows it under the prompt ("Cut where: nobody talks
    /// AND the picture is still, longer than 1 s").
    pub fn describe(&self) -> String {
        let when = describe(&self.when, true);
        let (b, a) = self.around();
        match self.action {
            Action::CutWhere | Action::KeepWhere => format!(
                "{} where: {when}, longer than {} s",
                if self.action == Action::CutWhere {
                    "Cut"
                } else {
                    "Keep only"
                },
                secs(self.min_seconds())
            ),
            Action::KeepAround => format!(
                "Keep {} s before to {} s after each time: {when}; cut the rest",
                secs(b),
                secs(a)
            ),
            Action::CutAround => format!(
                "Cut {} s before to {} s after each time: {when}",
                secs(b),
                secs(a)
            ),
            Action::Mark => format!("Mark each time: {when}"),
            Action::ClipEach => format!(
                "Clip {} s before to {} s after each time: {when}",
                secs(b),
                secs(a)
            ),
        }
    }

    /// Every speaker, phrase, sound and picture it names, for a translator's check.
    pub fn conds(&self) -> Vec<&Cond> {
        let mut out = Vec::new();
        walk(&self.when, &mut out);
        out
    }
}

fn walk<'a>(c: &'a Cond, out: &mut Vec<&'a Cond>) {
    match c {
        Cond::All(v) | Cond::Any(v) => v.iter().for_each(|c| walk(c, out)),
        Cond::Not(c) => walk(c, out),
        c => out.push(c),
    }
}

fn secs(x: f64) -> String {
    let s = format!("{:.2}", x);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn describe(c: &Cond, top: bool) -> String {
    let joined = |v: &[Cond], word: &str| {
        let s: Vec<String> = v.iter().map(|c| describe(c, false)).collect();
        if top || v.len() == 1 {
            s.join(word)
        } else {
            format!("({})", s.join(word))
        }
    };
    match c {
        Cond::Silence => "nobody talks".into(),
        Cond::StillPicture => "the picture is still".into(),
        Cond::Speaker(n) if n == ANYONE => "someone talks".into(),
        Cond::Speaker(n) => format!("{n} talks"),
        Cond::Words(p) => format!("\"{p}\" is said"),
        Cond::LikeThis(s) => format!("a sound like the marked one ({s})"),
        Cond::Label(p) => format!("the picture shows {p}"),
        Cond::All(v) => joined(v, " AND "),
        Cond::Any(v) => joined(v, " OR "),
        Cond::Not(c) => format!("NOT {}", describe(c, false)),
    }
}

fn cond_json(c: &Cond) -> Value {
    let one = |k: &str, v: Value| {
        let mut m = Map::new();
        m.insert(k.into(), v);
        Value::Object(m)
    };
    match c {
        Cond::Silence => one("silence", true.into()),
        Cond::StillPicture => one("still_picture", true.into()),
        Cond::Speaker(s) => one("speaker", s.as_str().into()),
        Cond::Words(s) => one("words", s.as_str().into()),
        Cond::LikeThis(s) => one("like_this", s.as_str().into()),
        Cond::Label(s) => one("label", s.as_str().into()),
        Cond::All(v) => one("all", v.iter().map(cond_json).collect()),
        Cond::Any(v) => one("any", v.iter().map(cond_json).collect()),
        Cond::Not(c) => one("not", cond_json(c)),
    }
}

fn bad(why: String) -> Error {
    Error::Recipe(why)
}

/// The first JSON object in `text` (a translator may wrap it in a code fence or say more after),
/// read as a recipe. Refused, with the reason, when it is not one: an unknown action, tool or
/// field, a wrong type, a number out of range.
pub fn parse(text: &str) -> Result<Recipe> {
    let object = first_object(text).ok_or_else(|| bad("no JSON object in the answer".into()))?;
    let v: Value =
        serde_json::from_str(object).map_err(|e| bad(format!("the answer is not JSON: {e}")))?;
    from_json(&v)
}

/// The text of the first balanced `{...}` in `text`, braces inside strings ignored.
pub fn first_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (i, ch) in text[start..].char_indices() {
        if in_str {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// A recipe from its JSON form, strictly ([`parse`]).
pub fn from_json(v: &Value) -> Result<Recipe> {
    let m = v
        .as_object()
        .ok_or_else(|| bad("a recipe is a JSON object".into()))?;
    for k in m.keys() {
        if !["action", "when", "min_seconds", "before", "after"].contains(&k.as_str()) {
            return Err(bad(format!("no field {k:?} in a recipe")));
        }
    }
    let name = m
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("the recipe has no \"action\"".into()))?;
    let action = Action::ALL
        .into_iter()
        .find(|a| a.name() == name)
        .ok_or_else(|| bad(format!("no action {name:?}")))?;
    let when = cond(
        m.get("when")
            .ok_or_else(|| bad("the recipe has no \"when\"".into()))?,
        1,
    )?;
    let num = |k: &str| -> Result<Option<f64>> {
        match m.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => match v.as_f64() {
                Some(x) if x.is_finite() && (0.0..=MAX_SECONDS).contains(&x) => Ok(Some(x)),
                _ => Err(bad(format!(
                    "\"{k}\" is seconds from 0 to {MAX_SECONDS}, not {v}"
                ))),
            },
        }
    };
    Ok(Recipe {
        action,
        when,
        min_seconds: num("min_seconds")?,
        before: num("before")?,
        after: num("after")?,
    })
}

fn cond(v: &Value, depth: usize) -> Result<Cond> {
    if depth > DEPTH {
        return Err(bad(format!(
            "conditions nest at most {DEPTH} deep in a recipe"
        )));
    }
    let m = v.as_object().filter(|m| !m.is_empty()).ok_or_else(|| {
        bad(format!(
            "a condition is a tool, like {{\"silence\": true}}, not {v}"
        ))
    })?;
    // Several tools in one object are all of them: the small model writes "silence and no movement"
    // as {"silence": true, "still_picture": true} about as often as with "all". A rule, not a
    // guess: each tool is still checked as itself.
    if m.len() > 1 {
        let all = m
            .iter()
            .map(|(k, v)| {
                let mut one = Map::new();
                one.insert(k.clone(), v.clone());
                cond(&Value::Object(one), depth + 1)
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(Cond::All(all));
    }
    let (k, v) = m.iter().next().expect("one entry");
    let text = || -> Result<String> {
        let s = v
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad(format!("\"{k}\" takes a word or a name, not {v}")))?;
        if s.chars().count() > MAX_TEXT {
            return Err(bad(format!("\"{k}\" is too long")));
        }
        Ok(s.to_string())
    };
    let flag = || -> Result<()> {
        match v {
            Value::Bool(true) => Ok(()),
            _ => Err(bad(format!("\"{k}\" takes true, not {v}"))),
        }
    };
    let list = || -> Result<Vec<Cond>> {
        let items = v
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| bad(format!("\"{k}\" takes a list of conditions")))?;
        items.iter().map(|c| cond(c, depth + 1)).collect()
    };
    Ok(match k.as_str() {
        "silence" => flag().map(|_| Cond::Silence)?,
        "still_picture" => flag().map(|_| Cond::StillPicture)?,
        "speaker" => Cond::Speaker(text()?),
        "words" => Cond::Words(text()?),
        "like_this" => Cond::LikeThis(text()?),
        "label" => Cond::Label(text()?),
        "all" => Cond::All(list()?),
        "any" => Cond::Any(list()?),
        "not" => Cond::Not(Box::new(cond(v, depth + 1)?)),
        other => return Err(bad(format!("no tool {other:?} on the menu"))),
    })
}

#[cfg(test)]
mod tests;
