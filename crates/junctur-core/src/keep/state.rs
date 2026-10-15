//! What Laya reads about a part: the video the user described, the part (who says what, or what the
//! picture and the game audio show where nobody talks) and the talk just before and after it, as the
//! JSON object Laya's SDK would write for the same dict. And the one question it is asked.

use super::laya::Question;
use super::parts::{Kind, Part};
use crate::labels::{Labelled, Labels};

/// Bumped when the question's wording or the state's shape changes, so every part is judged again.
pub const VERSION: u32 = 1;

/// Characters of the part itself, and of the talk before and after it: with the question this keeps
/// the whole inside Laya's 1024 tokens (Spanish runs about 3.5 characters a token).
const THIS_MAX: usize = 2400;
const AROUND_MAX: usize = 160;
/// Talk further than this from the part is not its context.
const AROUND_S: f64 = 60.0;
/// The video's description, at most.
const PROMPT_MAX: usize = 400;

/// The question every part is asked. The video goes in the state, not here, so the question is the
/// same for every prompt.
pub fn question() -> Question {
    Question {
        instructions: "Should this part of the recording stay in the video described in the state?"
            .into(),
        keep: "keep it: it fits that video, something happens or is said that matters to it".into(),
        cut: "cut it: the video does not need it, dead air, waiting, menus, small talk or nothing happening"
            .into(),
    }
}

/// A word said, with who said it.
#[derive(Debug, Clone, PartialEq)]
pub struct Spoken {
    pub start: f64,
    pub speaker: String,
    pub text: String,
}

/// What every state of a recording is built from: `words` in time order.
pub struct Context<'a> {
    pub prompt: &'a str,
    pub words: &'a [Spoken],
    pub labels: Option<&'a Labels>,
}

/// `part`'s state.
pub fn state(c: &Context, part: &Part) -> String {
    let w = c.words;
    let first = w.partition_point(|x| x.start < part.start);
    let past = w.partition_point(|x| x.start < part.end);
    let this = match part.kind {
        Kind::Talk if first == past => format!("{:.0} s of talk, no words made out.", part.len()),
        Kind::Talk => format!(
            "{:.0} s of talk. {}",
            part.len(),
            said(&w[first..past], THIS_MAX)
        ),
        Kind::Silent => silent(part, c.labels),
    };
    let near_before = w[..first].partition_point(|x| x.start < part.start - AROUND_S);
    let near_after = past + w[past..].partition_point(|x| x.start < part.end + AROUND_S);
    dumps(&[
        ("video", &shorten(c.prompt.trim(), PROMPT_MAX)),
        ("before", &said_ending(&w[near_before..first], AROUND_MAX)),
        ("this part", &this),
        ("after", &said(&w[past..near_after], AROUND_MAX)),
    ])
}

/// A silent part: its length, its top labels, how loud the game is.
fn silent(part: &Part, labels: Option<&Labels>) -> String {
    let mut s = format!("{:.0} s, nobody talks.", part.len());
    let Some(l) = labels.and_then(|l| labelled(l, part)) else {
        return s;
    };
    if !l.top.is_empty() {
        let looks: Vec<String> = l
            .top
            .iter()
            .map(|x| format!("{} {:.0}%", x.label, x.score * 100.0))
            .collect();
        s.push_str(&format!(" Looks like: {}.", looks.join(", ")));
    }
    if let Some(b) = l.busy {
        let how = if b < 1.0 / 3.0 {
            "quiet"
        } else if b > 2.0 / 3.0 {
            "loud"
        } else {
            "moderate"
        };
        s.push_str(&format!(
            " Game audio: {how}, busier than {:.0}% of the recording.",
            b * 100.0
        ));
    }
    s
}

/// The labels' piece that is `part` (both are cut from the same speech by the same rule).
fn labelled<'a>(l: &'a Labels, part: &Part) -> Option<&'a Labelled> {
    let at = l.pieces.partition_point(|p| p.end <= part.start + 1e-6);
    l.pieces
        .get(at)
        .filter(|p| (p.start - part.start).abs() < 1e-6 && (p.end - part.end).abs() < 1e-6)
}

/// `words` as "Name: word word Other: word", up to about `max` characters (an ellipsis after a cut).
fn said(words: &[Spoken], max: usize) -> String {
    let mut out = String::new();
    let mut who: Option<&str> = None;
    for x in words {
        if out.chars().count() >= max {
            out.push('…');
            break;
        }
        if who != Some(x.speaker.as_str()) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&x.speaker);
            out.push(':');
            who = Some(&x.speaker);
        }
        out.push(' ');
        out.push_str(&x.text);
    }
    out
}

/// [`said`] of the last words of `words`, about `max` characters of them.
fn said_ending(words: &[Spoken], max: usize) -> String {
    let mut chars = 0;
    let mut from = words.len();
    while from > 0 && chars < max {
        from -= 1;
        chars += words[from].text.chars().count() + 1;
    }
    let text = said(&words[from..], usize::MAX);
    if from > 0 {
        format!("…{text}")
    } else {
        text
    }
}

fn shorten(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// A flat object of strings as Python's `json.dumps(ensure_ascii=False)` writes it: `", "` between
/// members and `": "` after each key. Laya was trained on that spelling.
pub fn dumps(members: &[(&str, &str)]) -> String {
    let quoted = |s: &str| serde_json::to_string(s).expect("a string always serialises");
    let body: Vec<String> = members
        .iter()
        .map(|(k, v)| format!("{}: {}", quoted(k), quoted(v)))
        .collect();
    format!("{{{}}}", body.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::{Kind as LabelKind, Share};

    fn word(start: f64, speaker: &str, text: &str) -> Spoken {
        Spoken {
            start,
            speaker: speaker.into(),
            text: text.into(),
        }
    }

    fn part(start: f64, end: f64, kind: Kind) -> Part {
        Part { start, end, kind }
    }

    #[test]
    fn dumps_spells_the_object_as_python_does() {
        assert_eq!(
            dumps(&[("video", "a \"b\"\nñ"), ("x", "")]),
            r#"{"video": "a \"b\"\nñ", "x": ""}"#
        );
    }

    #[test]
    fn talk_names_each_speaker_once_per_run_with_its_context() {
        let words = [
            word(1.0, "Delay", "lo"),
            word(1.5, "Delay", "tengo"),
            word(10.0, "Uezar", "vamos,"),
            word(10.5, "Uezar", "flash!"),
            word(11.0, "Delay", "jajaja"),
            word(30.0, "Speaker 3", "buena"),
        ];
        let c = Context {
            prompt: " League highlights ",
            words: &words,
            labels: None,
        };
        let s = state(&c, &part(9.0, 21.0, Kind::Talk));
        assert_eq!(
            s,
            r#"{"video": "League highlights", "before": "Delay: lo tengo", "this part": "12 s of talk. Uezar: vamos, flash! Delay: jajaja", "after": "Speaker 3: buena"}"#
        );
        // Nothing heard in a talk part, and no context further than a minute away.
        let far = state(&c, &part(100.0, 104.0, Kind::Talk));
        assert!(far.contains(r#""before": """#) && far.contains("no words made out"));
    }

    #[test]
    fn silence_reads_its_labels_and_how_busy_the_game_is() {
        let labels = Labels {
            model: "m".into(),
            kind: LabelKind::League,
            labels: vec![],
            prompts: vec![],
            busy_track: None,
            pieces: vec![Labelled {
                start: 20.0,
                end: 48.0,
                frames: 14,
                top: vec![
                    Share {
                        label: "teamfight".into(),
                        score: 0.62,
                    },
                    Share {
                        label: "walking on the map".into(),
                        score: 0.21,
                    },
                ],
                busy: Some(0.81),
                level_db: Some(-14.0),
            }],
        };
        let c = Context {
            prompt: "x",
            words: &[],
            labels: Some(&labels),
        };
        let s = state(&c, &part(20.0, 48.0, Kind::Silent));
        assert!(s.contains("28 s, nobody talks. Looks like: teamfight 62%, walking on the map 21%. Game audio: loud, busier than 81% of the recording."), "{s}");
        // A piece the labels do not have is its length alone.
        let bare = state(&c, &part(50.0, 53.0, Kind::Silent));
        assert!(
            bare.contains(r#""this part": "3 s, nobody talks.""#),
            "{bare}"
        );
    }

    #[test]
    fn long_talk_and_long_context_are_cut_with_an_ellipsis() {
        let words: Vec<Spoken> = (0..2000)
            .map(|i| word(i as f64 * 0.1, "Uezar", "palabra"))
            .collect();
        let c = Context {
            prompt: &"p".repeat(500),
            words: &words,
            labels: None,
        };
        let s = state(&c, &part(50.0, 80.0, Kind::Talk));
        assert!(s.chars().count() < THIS_MAX + 2 * AROUND_MAX + PROMPT_MAX + 120);
        assert!(s.contains(r#""before": "…Uezar: palabra"#), "{s}");
        assert!(s.contains("palabra…\""));
    }
}
