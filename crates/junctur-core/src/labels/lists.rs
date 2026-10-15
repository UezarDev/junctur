//! The labels a recording's silent pieces are scored against: built-in lists per kind of video, and
//! the ones the user adds. Each label is shown by its text and scored by its prompt: SigLIP 2 base
//! reads a game frame far better against "a video game fight" than against "fighting" (33 against
//! 18 of 42 frames of the 1h45 sample read right), and naming the game in a prompt pulls every frame
//! of that game towards it, so the built-in prompts share one context and name no game.

use serde::{Deserialize, Serialize};

/// The kind of video, which picks the built-in list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Any game or screen recording.
    #[default]
    Generic,
    /// League of Legends: the generic labels and League's own.
    League,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Generic, Kind::League];

    /// Its name in the config file and `labels.json`.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Generic => "generic",
            Kind::League => "league",
        }
    }

    /// The built-in labels of this kind, in order: (shown, prompt).
    pub fn builtin(self) -> Vec<(&'static str, &'static str)> {
        let mut v = GENERIC.to_vec();
        if self == Kind::League {
            v.extend(LEAGUE);
        }
        v
    }
}

/// Every kind's labels. Tuned on frames of the 1h45 sample; see the phase 3a notes.
const GENERIC: [(&str, &str); 6] = [
    ("menu or loading screen", "a video game menu"),
    (
        "walking or travelling",
        "a video game character walking alone",
    ),
    ("fighting", "a video game fight"),
    ("cutscene", "a video game cutscene"),
    ("idle or AFK", "a video game character standing still"),
    ("desktop", "streaming software on a computer desktop"),
];

/// League's own, after the generic ones.
const LEAGUE: [(&str, &str); 5] = [
    ("teamfight", "a video game teamfight"),
    ("laning, no fight", "a video game lane with minions"),
    (
        "walking on the map",
        "a video game map with trees and paths",
    ),
    ("in base or shopping", "a video game item shop"),
    ("dead and waiting", "a video game screen in black and white"),
];

/// Labels the user added, per kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Added {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub generic: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub league: Vec<String>,
}

impl Added {
    pub fn of(&self, kind: Kind) -> &[String] {
        match kind {
            Kind::Generic => &self.generic,
            Kind::League => &self.league,
        }
    }
}

/// The labels settings in the config file (`labels`): off unless turned on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Label the silent parts at all. Off by default: it fetches 1.5 GB of model on first use.
    pub on: bool,
    pub kind: Kind,
    #[serde(skip_serializing_if = "is_empty")]
    pub added: Added,
}

fn is_empty(a: &Added) -> bool {
    a == &Added::default()
}

/// One label as it is scored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// What the lane and `labels.json` show.
    pub text: String,
    /// What the model reads.
    pub prompt: String,
}

impl Settings {
    /// The labels of this kind: the built-in ones, then the user's (trimmed, each once, none
    /// repeating a built-in one), scored by their own text.
    pub fn labels(&self) -> Vec<Label> {
        let mut out: Vec<Label> = self
            .kind
            .builtin()
            .into_iter()
            .map(|(text, prompt)| Label {
                text: text.into(),
                prompt: prompt.into(),
            })
            .collect();
        for a in self.added.of(self.kind) {
            let a = a.trim();
            if a.is_empty() || out.iter().any(|l| l.text.eq_ignore_ascii_case(a)) {
                continue;
            }
            out.push(Label {
                text: a.into(),
                prompt: a.into(),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn league_is_the_generic_list_and_its_own() {
        let g = Kind::Generic.builtin();
        let l = Kind::League.builtin();
        assert_eq!(g.len(), 6);
        assert_eq!(&l[..6], &g[..]);
        assert!(l.iter().any(|(t, _)| *t == "teamfight"));
        // Every built-in label is told apart by its text.
        let mut texts: Vec<_> = l.iter().map(|(t, _)| *t).collect();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), l.len());
    }

    #[test]
    fn added_labels_follow_the_kind_trimmed_and_once() {
        let s = Settings {
            on: true,
            kind: Kind::League,
            added: Added {
                generic: vec!["cooking".into()],
                league: vec![
                    " baron fight ".into(),
                    "".into(),
                    "Teamfight".into(),
                    "baron fight".into(),
                ],
            },
        };
        let l = s.labels();
        assert_eq!(l.len(), 12);
        assert_eq!(
            l.last().unwrap(),
            &Label {
                text: "baron fight".into(),
                prompt: "baron fight".into()
            }
        );
        assert!(!l.iter().any(|x| x.text == "cooking"));
    }

    #[test]
    fn off_and_generic_by_default_and_written_short() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
        assert!(!s.on);
        assert_eq!(
            serde_json::to_string(&Settings {
                on: true,
                ..Default::default()
            })
            .unwrap(),
            r#"{"on":true,"kind":"generic"}"#
        );
        assert!(serde_json::from_str::<Settings>(r#"{"of":true}"#).is_err());
    }
}
