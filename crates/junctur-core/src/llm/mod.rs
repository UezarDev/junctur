//! The translator (phase 7c, a prototype): a typed request read as a recipe ([`crate::recipe`]) by a
//! small text model, Qwen3 1.7B (or 4B, opted into: [`Size`]) through [`qwen`], told the menu and a few examples; its answer is checked
//! by [`recipe::parse`], an answer that does not read is shown its error once and asked again, and a
//! second miss (or no model: not downloaded, refused, no way to run it) falls back to the keyword
//! reader. The model only ever writes the recipe: it never sees the video, so it cannot cut anything
//! itself. A module of its own because the model, its prompt and the fallback rule are one choice
//! the app and the command line share.

pub mod qwen;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::recipe::{self, keywords, Recipe};
use crate::runtime::{Device, DevicePref};
use crate::Result;

pub use qwen::Size;
/// A recipe is a line or two; past this the model is rambling.
const MOST_TOKENS: usize = 160;

/// The menu, the rules and the examples, as the system turn. The examples are not the research's
/// requests (those are what the reader is measured on, phase 7c's record) but their patterns: two
/// conditions at once, "N seconds each time X or Y", "what NAME says about TOPIC".
const SYSTEM: &str = r#"You turn a video editor's request into one JSON recipe. Answer with the JSON only, on one line.

A recipe is {"action": ..., "when": ..., and optional "min_seconds", "before", "after"}.

Actions:
- "cut_where": cut the parts where "when" holds.
- "keep_where": keep only the parts where "when" holds, cut the rest.
- "keep_around": keep "before" seconds before and "after" seconds after each time "when" happens, cut the rest.
- "cut_around": cut "before" seconds before and "after" seconds after each time "when" happens.
- "mark": put a marker each time "when" happens.
- "clip_each": make a separate clip of each time "when" happens.

Conditions ("when"):
- {"silence": true}: nobody is talking.
- {"still_picture": true}: the picture does not move.
- {"speaker": "NAME"}: NAME is talking. {"speaker": "anyone"}: someone is talking.
- {"words": "PHRASE"}: someone says PHRASE (a word or a short phrase, in the language it is said).
- {"like_this": "SOUND"}: a sound like the example the user marked. Use it for every game sound or game event: breaking a block, an ore, wood, a hit, an explosion, a kill, a jump.
- {"label": "PICTURE"}: the picture shows PICTURE (a menu, an inventory, a cave, a map).
- {"all": [C, C]}: every condition holds. {"any": [C, C]}: one of them holds. {"not": C}: it does not hold.

"min_seconds" keeps only parts at least that long. "before" and "after" are seconds; 1/4 second around an event is "before": 0.1, "after": 0.15.

Examples:
Request: cut the parts where nobody talks for more than 3 seconds
{"action": "cut_where", "when": {"silence": true}, "min_seconds": 3}
Request: only keep what Ana says
{"action": "keep_where", "when": {"speaker": "Ana"}}
Request: keep 2 seconds after every explosion
{"action": "keep_around", "when": {"like_this": "explosion"}, "before": 0, "after": 2}
Request: corta cuando la pantalla está quieta y nadie habla
{"action": "cut_where", "when": {"all": [{"still_picture": true}, {"silence": true}]}}
Request: keep half a second each time I shoot or reload
{"action": "keep_around", "when": {"any": [{"like_this": "shoot"}, {"like_this": "reload"}]}, "before": 0.2, "after": 0.3}
Request: cut everything Ana says about school
{"action": "cut_where", "when": {"all": [{"speaker": "Ana"}, {"words": "school"}]}}
Request: quita lo que dice Bo sobre el dinero
{"action": "cut_where", "when": {"all": [{"speaker": "Bo"}, {"words": "dinero"}]}}
Request: mark every time someone says "let's go"
{"action": "mark", "when": {"words": "let's go"}}
Request: remove the parts in the pause menu
{"action": "cut_where", "when": {"label": "pause menu"}}"#;

/// Who read a request, and how it went: shown beside the recipe and kept in the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// [`Size::name`], [`keywords::NAME`], or "edited" for a recipe the user changed.
    pub reader: String,
    /// Why the model did not read it, when the keywords did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// The model's own answers, as it wrote them (one, or two after a retry).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<String>,
    /// Seconds to open the model, and to answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
}

impl Reading {
    /// Read by `reader`, nothing more known yet.
    pub fn by(reader: &str) -> Self {
        Self {
            reader: reader.into(),
            fallback: None,
            answers: Vec::new(),
            load_s: None,
            answer_s: None,
            device: None,
        }
    }
}

/// The chat prompt for `request`: the system turn with the recording's speakers named, the request,
/// and (on a retry) the answer that did not read with its error. A thinking model's thinking is
/// switched off as Qwen3's template does it: an empty think block opens the reply.
fn prompt(request: &str, speakers: &[String], retry: Option<(&str, &str)>, thinks: bool) -> String {
    let mut system = SYSTEM.to_string();
    if !speakers.is_empty() {
        system.push_str("\n\nSpeakers in this recording: ");
        system.push_str(&speakers.join(", "));
        system.push('.');
    }
    let mut p =
        format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{request}<|im_end|>\n");
    if let Some((answer, error)) = retry {
        p.push_str(&format!(
            "<|im_start|>assistant\n{answer}<|im_end|>\n<|im_start|>user\nThat is not a valid recipe: {error}. Answer with one valid recipe as JSON only.<|im_end|>\n"
        ));
    }
    p.push_str("<|im_start|>assistant\n");
    if thinks {
        p.push_str("<think>\n\n</think>\n\n");
    }
    p
}

/// The model, open.
pub struct Translator {
    model: qwen::Qwen,
    pub size: Size,
    pub load_s: f64,
    /// It has read a request: the next ones did not open it.
    read_once: bool,
}

impl Translator {
    /// Fetches the model on first use and opens it, the GPU first.
    pub fn open(cache_dir: &Path, size: Size, pref: DevicePref, stop: &AtomicBool) -> Result<Self> {
        let t = Instant::now();
        let model = qwen::Qwen::open(cache_dir, size, pref, stop)?;
        Ok(Self {
            model,
            size,
            load_s: t.elapsed().as_secs_f64(),
            read_once: false,
        })
    }

    pub fn device(&self) -> Device {
        self.model.device
    }

    /// `request` as a recipe: the model's answer, or its second when the first did not read; why
    /// not, with both answers, when neither did.
    pub fn read(&mut self, request: &str, speakers: &[String]) -> Read {
        let started = Instant::now();
        let opened = !std::mem::replace(&mut self.read_once, true);
        let mut reading = Reading {
            load_s: opened.then(|| round(self.load_s)),
            device: Some(self.model.device),
            ..Reading::by(self.size.name())
        };
        let done = |s: &str| recipe::first_object(s).is_some();
        let mut retry: Option<String> = None;
        let mut last_error = String::new();
        for _ in 0..2 {
            let again = retry.as_deref().map(|a| (a, last_error.as_str()));
            let p = prompt(request, speakers, again, self.size.thinks());
            let answer = match self.model.reply(&p, MOST_TOKENS, &done) {
                Ok(a) => a.trim().to_string(),
                Err(e) => {
                    reading.answer_s = Some(round(started.elapsed().as_secs_f64()));
                    return Read::failed(format!("the model failed: {e}"), reading);
                }
            };
            reading.answers.push(answer.clone());
            match recipe::parse(&answer) {
                Ok(r) => {
                    reading.answer_s = Some(round(started.elapsed().as_secs_f64()));
                    return Read {
                        recipe: Ok(r),
                        reading,
                    };
                }
                Err(e) => {
                    last_error = e.to_string();
                    retry = Some(answer);
                }
            }
        }
        reading.answer_s = Some(round(started.elapsed().as_secs_f64()));
        Read::failed(
            format!("the model's answers did not read as a recipe ({last_error})"),
            reading,
        )
    }
}

fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// A request read as a recipe, or why it did not read; and who read or tried it.
#[derive(Debug, Clone)]
pub struct Read {
    pub recipe: std::result::Result<Recipe, String>,
    pub reading: Reading,
}

impl Read {
    pub fn failed(why: String, reading: Reading) -> Self {
        Self {
            recipe: Err(why),
            reading,
        }
    }
}

/// `request` read by the keywords, with why the model did not read it (`why`, none when it was not
/// asked) and what it tried.
pub fn by_keywords(
    request: &str,
    speakers: &[String],
    why: Option<String>,
    tried: Option<Reading>,
) -> Read {
    let mut reading = tried.unwrap_or_else(|| Reading::by(""));
    reading.reader = keywords::NAME.into();
    reading.fallback = why;
    let recipe = keywords::read(request, speakers).map_err(|e| match &reading.fallback {
        Some(model) => format!("{e} ({model})"),
        None => e.to_string(),
    });
    Read { recipe, reading }
}

/// `request` read by the model when `model` holds one (opened by the caller, which decides when to
/// load and drop it), else by the keywords; a model that fails falls back to them too, saying why.
pub fn read(
    model: Option<&mut Translator>,
    request: &str,
    speakers: &[String],
    no_model: Option<String>,
) -> Read {
    match model {
        Some(m) => match m.read(request, speakers) {
            Read {
                recipe: Err(why),
                reading,
            } => by_keywords(request, speakers, Some(why), Some(reading)),
            read => read,
        },
        None => by_keywords(request, speakers, no_model, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_the_speakers_and_switches_thinking_off() {
        let p = prompt("cut it", &["Ana".into(), "Bo".into()], None, true);
        assert!(p.contains("Speakers in this recording: Ana, Bo."));
        assert!(p.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
        let p = prompt("cut it", &[], Some(("{\"a\":1}", "no field \"a\"")), false);
        assert!(p.ends_with("<|im_start|>assistant\n"));
        assert!(p.contains("That is not a valid recipe: no field \"a\""));
        assert!(!p.contains("Speakers in this recording"));
    }

    #[test]
    fn without_a_model_the_keywords_read_it_and_say_why() {
        let got = read(None, "cut the silences", &[], Some("not downloaded".into()));
        let (r, how) = (got.recipe.unwrap(), got.reading);
        assert_eq!(r.action, recipe::Action::CutWhere);
        assert_eq!(how.reader, keywords::NAME);
        assert_eq!(how.fallback.as_deref(), Some("not downloaded"));
    }

    /// The research's requests through the real models; run by hand with them in the cache
    /// (`cargo test -p junctur-core --release --features cuda -- --ignored llm::`).
    #[test]
    #[ignore = "needs the 1.4 GB and 2.9 GB models in the user cache"]
    fn the_models_read_the_research_requests() {
        for size in [Size::Small, Size::Large] {
            research(size);
        }
    }

    fn research(size: Size) {
        let cache = crate::cache::default_dir();
        let stop = AtomicBool::new(false);
        let mut t = Translator::open(&cache, size, DevicePref::Auto, &stop).unwrap();
        eprintln!(
            "{} opened on {:?} in {:.1} s",
            size.name(),
            t.device(),
            t.load_s
        );
        for request in [
            "remove parts with silence and no movement",
            "keep only 1/4 second each time I break an ore, wood, or hit an enemy",
            "cut everything Tsunheto says about food",
            "cut when the inventory is open",
        ] {
            let got = t.read(request, &["Tsunheto".into(), "Uezar".into()]);
            match (got.recipe, got.reading) {
                (Ok(r), how) => eprintln!(
                    "{request}\n  {}\n  {}\n  {:?} s",
                    r.to_json(),
                    r.describe(),
                    how.answer_s
                ),
                (Err(why), how) => panic!("{request}: {why} {:?}", how.answers),
            }
        }
    }
}
