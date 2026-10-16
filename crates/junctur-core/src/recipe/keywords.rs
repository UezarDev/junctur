//! The keyword reader: a recipe from a request without the model (not downloaded, no GPU, or two
//! answers that did not read), for the requests people type most, in English and Spanish. It looks
//! for the action's verb, the tools' words and a number of seconds, and combines what it finds with
//! AND; a request it finds no tool in is refused, never guessed. The app says which reader read a
//! request, so a misreading is seen as one.

use super::{Action, Cond, Recipe, AROUND};
use crate::text::folded;
use crate::{Error, Result};

/// The name the app and the history give this reader.
pub const NAME: &str = "keywords";

/// Words of each tool, matched as whole folded words or phrases (`" no movement "`).
const SILENCE: &[&str] = &[
    "silence",
    "silent",
    "silences",
    "nobody talks",
    "nobody is talking",
    "no one talks",
    "no talking",
    "dead air",
    "quiet",
    "pauses",
    "silencio",
    "silencios",
    "nadie habla",
    "no hablo",
    "no habla",
    "no hablamos",
    "no hablan",
    "sin hablar",
    "callados",
    "pausas",
];
const STILL: &[&str] = &[
    "no movement",
    "not moving",
    "don t move",
    "doesn t move",
    "nothing moves",
    "still picture",
    "still screen",
    "frozen",
    "afk",
    "static",
    "no me muevo",
    "no se mueve",
    "nada se mueve",
    "quieto",
    "quieta",
    "sin movimiento",
    "congelado",
    "congelada",
    "pantalla quieta",
];
/// Game events a model-free reader can only send to "find all like this": the sound the user marks.
const EVENTS: &[&str] = &[
    "break",
    "breaking",
    "ore",
    "ores",
    "wood",
    "hit",
    "hits",
    "enemy",
    "enemies",
    "kill",
    "kills",
    "mine",
    "mining",
    "chop",
    "shoot",
    "shot",
    "explosion",
    "jump",
    "sound",
    "like this",
    "rompo",
    "romper",
    "mineral",
    "madera",
    "golpe",
    "golpeo",
    "enemigo",
    "mato",
    "disparo",
    "salto",
    "sonido",
    "como este",
];
/// Pictures the labels can look for, as the reader names them.
const PICTURES: &[(&str, &str)] = &[
    ("inventory", "an open inventory"),
    ("inventario", "an open inventory"),
    ("menu", "a menu"),
    ("pause", "a pause menu"),
    ("pausa", "a pause menu"),
    ("map", "a map"),
    ("mapa", "a map"),
    ("loading", "a loading screen"),
    ("cargando", "a loading screen"),
    ("lobby", "a lobby"),
    ("cave", "a cave"),
    ("cueva", "a cave"),
];
const CUT: &[&str] = &[
    "cut", "remove", "delete", "drop", "trim", "quita", "quitar", "corta", "cortar", "elimina",
    "eliminar", "borra", "borrar", "saca", "sacar",
];
const KEEP: &[&str] = &[
    "keep", "only", "leave", "deja", "dejar", "solo", "manten", "mantener", "guarda", "conserva",
];
const MARK: &[&str] = &["mark", "marker", "markers", "marca", "marcar", "marcador"];
const CLIP: &[&str] = &["clip", "clips", "clipea", "clipear"];
const EACH: &[&str] = &[
    "each time",
    "every time",
    "whenever",
    "each",
    "every",
    "cada vez",
    "cada",
    "siempre que",
];
const SAYS: &[&str] = &["says", "say", "said", "dice", "dicen", "digo", "decimos"];
const ABOUT: &[&str] = &["about", "sobre", "acerca de", "de que", "habla de"];

fn has(f: &str, words: &[&str]) -> bool {
    words.iter().any(|w| f.contains(&format!(" {w} ")))
}

/// The first of `words` in `f` and where it ends.
fn find_end(f: &str, words: &[&str]) -> Option<usize> {
    words
        .iter()
        .filter_map(|w| f.find(&format!(" {w} ")).map(|i| (i, i + w.len() + 1)))
        .min_by_key(|x| x.0)
        .map(|x| x.1)
}

/// `request` read with the keywords; `speakers` are the recording's names, for "what Ana says".
pub fn read(request: &str, speakers: &[String]) -> Result<Recipe> {
    // The tools and verbs are read outside a quoted phrase: "mark each time someone says "clip
    // that"" marks.
    let quoted = phrase(request, "");
    let f = folded(&match &quoted {
        Some(q) => request.replacen(q.as_str(), " ", 1),
        None => request.to_string(),
    });
    let mut conds = Vec::new();
    if has(&f, SILENCE) {
        conds.push(Cond::Silence);
    }
    if has(&f, STILL) {
        conds.push(Cond::StillPicture);
    }
    for name in speakers {
        let n = folded(name);
        if n.trim().len() >= 2 && f.contains(&n) {
            conds.push(Cond::Speaker(name.clone()));
        }
    }
    if let Some(p) = quoted.or_else(|| phrase(request, &f)) {
        conds.push(Cond::Words(p));
    }
    if let Some((_, p)) = PICTURES.iter().find(|(w, _)| has(&f, &[*w])) {
        conds.push(Cond::Label((*p).to_string()));
    }
    let seconds = seconds(request);
    let each = has(&f, EACH);
    if conds.is_empty() && has(&f, EVENTS) {
        conds.push(Cond::LikeThis("the marked sound".into()));
    }
    let when = match conds.len() {
        0 => {
            return Err(Error::Recipe(
                "the keyword reader found nothing it knows in that: try words like silence, no movement, a speaker's name, says \"...\", or each time (with an example marked in Like this)".into(),
            ))
        }
        1 => conds.remove(0),
        _ => Cond::All(conds),
    };
    let verb = [(CLIP, Action::ClipEach), (MARK, Action::Mark)]
        .into_iter()
        .find(|(w, _)| has(&f, w))
        .map(|x| x.1);
    let keep = !has(&f, CUT) && has(&f, KEEP);
    let events = each && !matches!(when, Cond::Silence | Cond::StillPicture);
    let action = match (verb, events, keep) {
        (Some(a), _, _) => a,
        (None, true, true) => Action::KeepAround,
        (None, true, false) => Action::CutAround,
        (None, false, true) => Action::KeepWhere,
        (None, false, false) => Action::CutWhere,
    };
    let (mut before, mut after, mut min_seconds) = (None, None, None);
    match (action, seconds) {
        (Action::KeepAround | Action::CutAround | Action::ClipEach, Some(s)) => {
            // Split as the research's quarter second is: 0.1 before, 0.15 after.
            let share = AROUND.0 / (AROUND.0 + AROUND.1);
            before = Some(round(s * share));
            after = Some(round(s - s * share));
        }
        (Action::CutWhere | Action::KeepWhere, Some(s)) => min_seconds = Some(s),
        _ => {}
    }
    Ok(Recipe {
        action,
        when,
        min_seconds,
        before,
        after,
    })
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// A quoted phrase, else the words after "says ... about", in the request's own spelling.
fn phrase(request: &str, f: &str) -> Option<String> {
    // Not the apostrophe: "I'm quiet and don't talk" quotes nothing.
    for (open, close) in [('"', '"'), ('“', '”'), ('«', '»')] {
        if let Some(a) = request.find(open) {
            let rest = &request[a + open.len_utf8()..];
            if let Some(b) = rest.find(close) {
                let p = rest[..b].trim();
                if !p.is_empty() && p.len() <= 100 {
                    return Some(p.to_string());
                }
            }
        }
    }
    if f.is_empty() || !has(f, SAYS) && !has(f, &["habla de", "talks about", "talk about"]) {
        return None;
    }
    let end = find_end(f, ABOUT).or_else(|| find_end(f, SAYS))?;
    let p = f[end..].trim();
    (!p.is_empty() && p.split(' ').count() <= 4).then(|| p.to_string())
}

/// A number of seconds in the request ("1/4 second", "half a second", "0.5 s", "dos segundos",
/// "más de 5 segundos"), if any.
pub fn seconds(request: &str) -> Option<f64> {
    let lower = request.to_lowercase();
    let f = folded(&lower);
    for (words, s) in [
        (
            &[
                "quarter second",
                "quarter of a second",
                "cuarto de segundo",
                "un cuarto",
            ][..],
            0.25,
        ),
        (&["half a second", "half second", "medio segundo"][..], 0.5),
    ] {
        if has(&f, words) {
            return Some(s);
        }
    }
    // Tokens that keep a number's '.', ',' and '/'.
    let toks: Vec<&str> = lower
        .split(|c: char| !(c.is_alphanumeric() || c == '.' || c == ',' || c == '/'))
        .filter(|t| !t.is_empty())
        .collect();
    for (i, t) in toks.iter().enumerate() {
        let unit = toks.get(i + 1).copied().unwrap_or("");
        let (num, glued) = match t.find(|c: char| c.is_alphabetic()) {
            Some(k) if k > 0 => (&t[..k], &t[k..]),
            _ => (*t, ""),
        };
        let Some(x) = number(num.trim_end_matches(['.', ','])) else {
            continue;
        };
        let is_unit = |u: &str| {
            [
                "s", "sec", "secs", "second", "seconds", "seg", "segundo", "segundos",
            ]
            .contains(&u)
        };
        if is_unit(glued) || is_unit(unit.trim_end_matches(['.', ','])) {
            return Some(x).filter(|x| x.is_finite() && *x > 0.0 && *x <= 600.0);
        }
    }
    None
}

fn number(t: &str) -> Option<f64> {
    if let Some((a, b)) = t.split_once('/') {
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        return (b != 0.0).then(|| a / b);
    }
    let words = [
        ("one", 1.0),
        ("two", 2.0),
        ("three", 3.0),
        ("five", 5.0),
        ("ten", 10.0),
        ("un", 1.0),
        ("uno", 1.0),
        ("dos", 2.0),
        ("tres", 3.0),
        ("cinco", 5.0),
        ("diez", 10.0),
    ];
    if let Some((_, x)) = words.iter().find(|(w, _)| *w == t) {
        return Some(*x);
    }
    t.replace(',', ".").parse().ok()
}
