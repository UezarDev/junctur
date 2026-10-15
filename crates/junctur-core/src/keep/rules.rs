//! Smart keep's rules (phase 3d), which replaced Laya's answer: talk stays as the automatic cut leaves
//! it, and the windows of a silent stretch that are busier than this recording usually is (its game
//! audio, or 3a's fight labels) are suggested as keeps, padded and merged. How many is read from the
//! prompt's words ([`Strictness`]). A module of its own because it is the whole judgement smart keep
//! now makes, pure (no file, no model) so every edge is a unit test; the stage reads the peaks and
//! writes the file around it.
//!
//! Rejected: Laya over the silent parts (its margin never followed busy or the fight labels in any
//! wording tried, ideas.md); a fixed busy number (busy is a rank in the recording already, and a dB
//! line would follow the game's volume); whole stretches (one fight in two minutes of walking would
//! bring back the two minutes); every silent part kept or cut in `keep.json` (the automatic cut keeps
//! silence beside talk, which a "cut" would then suggest cutting: only keeps are suggested).

use super::parts::Kind;
use super::{Judged, Judgement};
use crate::edit::Window;
use crate::labels::busy;
use crate::labels::pieces::{self, Piece};
use crate::labels::Labels;
use crate::spans::{joined, Join};
use crate::vad::Region;

/// The judge's name in `keep.json` ([`Judged::model`]).
pub const NAME: &str = "rules";
/// Bumped when the rules change, so a `keep.json` they wrote before is judged again.
pub const VERSION: u32 = 1;
/// A silent stretch is looked at in windows of at most this many seconds (the score windows' grain).
pub const WINDOW: f64 = 5.0;
/// A kept window is padded by this much either side, inside its stretch, so a fight is not cut at its
/// first blow.
pub const PAD: f64 = 2.0;
/// Kept spans this close (after padding) are one.
pub const MERGE: f64 = 3.0;
/// The labels that say "fight" (3a's generic and League lists).
const FIGHT: [&str; 2] = ["fighting", "teamfight"];
/// A span's margin at the top of the recording: its chance shows 0.5 at the line to 0.98 at the top.
const TOP_MARGIN: f64 = 4.0;

/// How much of the silence a prompt keeps, read from its words ([`Strictness::read`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// No silent part: the talk is the video.
    TalkOnly,
    /// The busiest tenth of the silence.
    Strict,
    /// The busiest quarter: no word says otherwise.
    Usual,
    /// The busiest half: only the dead air goes.
    Loose,
}

/// The words of each level, accents folded and lowercase ([`fold`]), English then Spanish. The first
/// level any phrase of which is in the prompt wins, in this order, so "podcast highlights" is talk
/// only and "highlights of the whole stream" strict.
const PHRASES: [(Strictness, &[&str]); 3] = [
    (
        Strictness::TalkOnly,
        &[
            "podcast",
            "talk only",
            "only talk",
            "only the talk",
            "just the talk",
            "just talk",
            "cut the game",
            "no gameplay",
            "without the game",
            "interview",
            "solo la charla",
            "solo charla",
            "solo hablar",
            "solo hablando",
            "solo la conversacion",
            "sin el juego",
            "sin gameplay",
            "corta el juego",
            "entrevista",
            "tertulia",
        ],
    ),
    (
        Strictness::Strict,
        &[
            "highlight",
            "highlights",
            "only fights",
            "only the fights",
            "just the fights",
            "only the best",
            "best moments",
            "best parts",
            "best plays",
            "short video",
            "montage",
            "destacados",
            "mejores momentos",
            "mejores jugadas",
            "lo mejor",
            "solo peleas",
            "solo las peleas",
            "solo lo mejor",
            "resumen",
            "video corto",
        ],
    ),
    (
        Strictness::Loose,
        &[
            "dead air",
            "whole stream",
            "full stream",
            "whole vod",
            "full vod",
            "as it happened",
            "anything where",
            "keep most",
            "tiempo muerto",
            "tiempos muertos",
            "stream completo",
            "directo completo",
            "todo el stream",
            "todo el directo",
            "todo lo que pasa",
            "casi todo",
        ],
    ),
];

impl Strictness {
    /// The level `prompt`'s words name; [`Strictness::Usual`] when none does. A negation ("not only
    /// the fights") reads as its phrase: words are matched, not understood.
    pub fn read(prompt: &str) -> Strictness {
        let text = fold(prompt);
        PHRASES
            .iter()
            .find(|(_, words)| words.iter().any(|w| text.contains(&format!(" {w} "))))
            .map_or(Strictness::Usual, |(level, _)| *level)
    }

    /// The share of the recording's silent windows kept.
    pub fn share(self) -> f64 {
        match self {
            Strictness::TalkOnly => 0.0,
            Strictness::Strict => 0.10,
            Strictness::Usual => 0.25,
            Strictness::Loose => 0.50,
        }
    }

    /// How it reads in the app and the log.
    pub fn describe(self) -> String {
        match self {
            Strictness::TalkOnly => "talk only, no silent part kept".into(),
            s => {
                let name = match s {
                    Strictness::Strict => "strict",
                    Strictness::Loose => "loose",
                    _ => "usual",
                };
                format!(
                    "{name}, the busiest {:.0}% of the silence",
                    s.share() * 100.0
                )
            }
        }
    }
}

/// `s` lowercase, its accents folded (ñ kept), anything not a letter or digit a space, one space
/// between words and one at either end, so a phrase is matched as whole words.
fn fold(s: &str) -> String {
    let mut out = String::from(" ");
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        if c != ' ' || !out.ends_with(' ') {
            out.push(c);
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// What the rules read about a recording.
pub struct Input<'a> {
    pub prompt: &'a str,
    /// Every speech track's regions together.
    pub speech: &'a [Region],
    pub duration: f64,
    pub labels: Option<&'a Labels>,
    /// The busy track's peaks ([`busy::peaks`]); `None` when there is no busy track or it could not
    /// be read, and then each piece's busy from the labels stands for its windows.
    pub peaks: Option<&'a [u8]>,
}

/// One window of a silent stretch, with its two signals.
#[derive(Debug, Clone, Copy)]
struct Look {
    start: f64,
    end: f64,
    /// Which stretch it is in.
    stretch: usize,
    /// Busy (a rank among the recording's seconds), `None` when unknown.
    busy: Option<f64>,
    /// The fight labels' share, 0 without labels.
    fight: f64,
}

/// `input`'s busy silent spans, as `keep.json` holds them: only keeps, in time order.
pub fn judge(input: &Input) -> Judged {
    let strictness = Strictness::read(input.prompt);
    let stretches: Vec<(f64, f64)> = joined(
        pieces::pieces(input.speech, input.duration)
            .into_iter()
            .map(|p| (p.start, p.end)),
        Join::Touching,
    );
    let looks = looks(&stretches, input);
    let parts = spans(&looks, &stretches, strictness.share());
    Judged {
        model: NAME.into(),
        prompt: input.prompt.into(),
        version: VERSION,
        parts,
    }
}

/// The windows of every stretch, each with its busy (from the peaks, else its labels piece's) and
/// fight share (its labels piece's: the one holding its middle).
fn looks(stretches: &[(f64, f64)], input: &Input) -> Vec<Look> {
    let mut windows: Vec<(usize, Piece)> = Vec::new();
    for (k, &(a, b)) in stretches.iter().enumerate() {
        let n = ((b - a) / WINDOW).ceil().max(1.0) as usize;
        let step = (b - a) / n as f64;
        for i in 0..n {
            let start = a + i as f64 * step;
            let end = if i + 1 == n { b } else { start + step };
            windows.push((k, Piece { start, end }));
        }
    }
    let pieces: Vec<Piece> = windows.iter().map(|w| w.1).collect();
    let heard = input
        .peaks
        .map(|p| busy::scores(p, &pieces))
        .unwrap_or_default();
    windows
        .iter()
        .enumerate()
        .map(|(i, &(stretch, w))| {
            let piece = input.labels.and_then(|l| {
                let mid = (w.start + w.end) / 2.0;
                l.pieces.iter().find(|p| p.start <= mid && mid < p.end)
            });
            let busy = heard
                .get(i)
                .copied()
                .flatten()
                .map(|s| s.0)
                .or_else(|| piece.and_then(|p| p.busy));
            let fight = piece.map_or(0.0, |p| {
                p.top
                    .iter()
                    .filter(|s| FIGHT.contains(&s.label.as_str()))
                    .map(|s| s.score)
                    .sum()
            });
            Look {
                start: w.start,
                end: w.end,
                stretch,
                busy,
                fight,
            }
        })
        .collect()
}

/// Each value's rank among `values`: the share of them strictly lower. `None` is lowest.
fn ranks(values: &[Option<f64>]) -> Vec<f64> {
    let mut sorted: Vec<f64> = values.iter().flatten().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let missing = values.len() - sorted.len();
    let n = values.len().max(1) as f64;
    values
        .iter()
        .map(|v| match v {
            None => 0.0,
            Some(x) => (missing + sorted.partition_point(|s| s < x)) as f64 / n,
        })
        .collect()
}

/// The kept windows of `looks` as spans: each window's score the higher of its two ranks, the line
/// the score that leaves the top `share` of the windows at or above it, a window lowest on both
/// signals (score 0) never kept; each kept window padded by [`PAD`] inside its stretch and those
/// within [`MERGE`] of each other joined.
fn spans(looks: &[Look], stretches: &[(f64, f64)], share: f64) -> Vec<Judgement> {
    let keep = (share * looks.len() as f64).round() as usize;
    if keep == 0 {
        return Vec::new();
    }
    let busy = ranks(&looks.iter().map(|l| l.busy).collect::<Vec<_>>());
    let fight = ranks(&looks.iter().map(|l| Some(l.fight)).collect::<Vec<_>>());
    let score: Vec<f64> = busy.iter().zip(&fight).map(|(b, f)| b.max(*f)).collect();
    let mut sorted = score.clone();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let line = sorted[keep - 1];
    let kept = |s: f64| s >= line && s > 0.0;
    // The best window left out: a kept window's sureness is how far it stands above it (0 there, 1
    // at the top), so a close call at the line is uncertain and a clear one is not, ties or not.
    let floor = score
        .iter()
        .copied()
        .filter(|s| !kept(*s))
        .fold(0.0f64, f64::max);
    let windows: Vec<((f64, f64), f64, usize)> = looks
        .iter()
        .enumerate()
        .filter(|(i, _)| kept(score[*i]))
        .map(|(i, l)| {
            let (a, b) = stretches[l.stretch];
            let span = ((l.start - PAD).max(a), (l.end + PAD).min(b));
            (span, (score[i] - floor) / (1.0 - floor), i)
        })
        .collect();
    let mut out: Vec<(Judgement, f64)> = Vec::new();
    for ((a, b), above, i) in windows {
        let l = &looks[i];
        let why = why(l, busy[i], fight[i]);
        match out.last_mut() {
            Some((last, best)) if a - last.end <= MERGE && same_stretch(last, l, stretches) => {
                last.end = last.end.max(b);
                if above > *best {
                    *best = above;
                    last.state = why;
                }
            }
            _ => out.push((
                Judgement {
                    start: a,
                    end: b,
                    kind: Kind::Silent,
                    margin: 0.0,
                    state: why,
                    hash: String::new(),
                },
                above,
            )),
        }
    }
    out.into_iter()
        .map(|(mut j, above)| {
            j.margin = (above * TOP_MARGIN * 1000.0).round() / 1000.0;
            j
        })
        .collect()
}

fn same_stretch(last: &Judgement, l: &Look, stretches: &[(f64, f64)]) -> bool {
    let (a, b) = stretches[l.stretch];
    last.start >= a - 1e-9 && last.end <= b + 1e-9
}

/// Why a window is kept, in words, for `keep.json`.
fn why(l: &Look, busy: f64, fight: f64) -> String {
    let mut s = format!("game audio busier than {:.0}% of the silence", busy * 100.0);
    if l.fight > 0.0 {
        s.push_str(&format!(
            "; fight labels {:.0}%, more than {:.0}% of the silence",
            l.fight * 100.0,
            fight * 100.0
        ));
    }
    s
}

/// The window scores (`len` seconds from each start): the speech share, the edit list's base, blended
/// toward a kept span's chance for the share of the window it covers.
pub fn score_windows(windows: &mut [Window], len: f64, j: &Judged) {
    for w in windows {
        let (a, b) = (w.start, w.start + len);
        let from = j.parts.partition_point(|p| p.end <= a);
        let (mut sum, mut cover) = (0.0, 0.0);
        for p in j.parts[from..].iter().take_while(|p| p.start < b) {
            let over = (p.end.min(b) - p.start.max(a)).max(0.0);
            sum += over * super::laya::p_keep(p.margin, 1.0);
            cover += over;
        }
        let score = (sum + (len - cover).max(0.0) * w.speech) / len;
        // Three places, as the file writes it.
        w.score = (score * 1000.0).round() / 1000.0;
    }
}

/// One line for the log.
pub fn summary(j: &Judged) -> String {
    let s = Strictness::read(&j.prompt);
    // From 0.0, not `sum`'s -0.0, so no span reads "0 s".
    let seconds = j.parts.iter().fold(0.0, |s, p| s + p.end - p.start);
    format!(
        "keep: talk as the cut leaves it; silence read as {}: {} busy spans to keep ({seconds:.0} s)",
        s.describe(),
        j.parts.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keep::PRESETS;
    use crate::labels::{Kind as LabelKind, Labelled, Share};

    fn r(start: f64, end: f64) -> Region {
        Region { start, end }
    }

    /// Peaks at `waves::RATE` a second: byte 20 everywhere, `loud` (start, end, byte) louder.
    fn peaks(seconds: f64, loud: &[(f64, f64, u8)]) -> Vec<u8> {
        let rate = crate::waves::RATE as f64;
        let mut v = vec![20u8; (seconds * rate) as usize];
        for &(a, b, byte) in loud {
            v[(a * rate) as usize..(b * rate) as usize].fill(byte);
        }
        v
    }

    fn input<'a>(prompt: &'a str, speech: &'a [Region], d: f64, p: &'a [u8]) -> Input<'a> {
        Input {
            prompt,
            speech,
            duration: d,
            labels: None,
            peaks: Some(p),
        }
    }

    fn spans_of(j: &Judged) -> Vec<(f64, f64)> {
        j.parts.iter().map(|p| (p.start, p.end)).collect()
    }

    #[test]
    fn the_presets_read_as_the_kind_of_video_they_describe() {
        let read: Vec<Strictness> = PRESETS.iter().map(|p| Strictness::read(p.1)).collect();
        assert_eq!(
            read,
            [Strictness::Strict, Strictness::Loose, Strictness::TalkOnly]
        );
    }

    #[test]
    fn the_prompt_is_read_in_english_and_spanish_first_level_first() {
        use Strictness::*;
        for (prompt, level) in [
            ("", Usual),
            ("League with friends", Usual),
            ("Only the fights!", Strict),
            ("Solo las peleas, por favor", Strict),
            ("los MEJORES momentos", Strict),
            ("Todo el directo, sin tiempo muerto", Loose),
            ("Pódcast de la charla", TalkOnly),
            ("podcast highlights", TalkOnly),
            ("highlights of the whole stream", Strict),
            // Whole words: "vodka" is not "vod", "shortcut" not "short video".
            ("a shortcut to vodka", Usual),
        ] {
            assert_eq!(Strictness::read(prompt), level, "{prompt:?}");
        }
        assert_eq!(fold("¡Qué PELEA!  ya"), " que pelea ya ");
    }

    #[test]
    fn only_the_busy_windows_of_a_long_silence_are_kept_padded() {
        // Talk 0-10 and 130-140; 120 s of silence between, loud from 60 to 70 only.
        let speech = [r(0.0, 10.0), r(130.0, 140.0)];
        let p = peaks(140.0, &[(60.0, 70.0, 200)]);
        let j = judge(&input("highlights", &speech, 140.0, &p));
        assert_eq!((j.model.as_str(), j.version), (NAME, VERSION));
        // Strict keeps 10% of the 24 windows of 5 s: the two loud ones, each padded 2 s, joined.
        assert_eq!(spans_of(&j), [(58.0, 72.0)]);
        assert!(j
            .parts
            .iter()
            .all(|p| p.kind == Kind::Silent && p.margin >= 0.0));
        assert!(j.parts[0].state.contains("game audio busier than"));
        // Talk only keeps nothing; loose keeps more around the same place.
        let none = judge(&input("podcast", &speech, 140.0, &p));
        assert!(none.parts.is_empty());
        assert!(
            summary(&none).ends_with("talk only, no silent part kept: 0 busy spans to keep (0 s)")
        );
        assert!(
            summary(&j).ends_with("1 busy spans to keep (14 s)"),
            "{}",
            summary(&j)
        );
        let loose = judge(&input("whole stream", &speech, 140.0, &p));
        assert!(loose.parts.iter().any(|x| x.start <= 58.0 && x.end >= 72.0));
    }

    #[test]
    fn the_line_is_the_recordings_own_whatever_the_volume() {
        // The same shape at two game volumes keeps the same windows.
        let speech = [r(0.0, 5.0)];
        let quiet = peaks(105.0, &[(40.0, 50.0, 60)]);
        let loud: Vec<u8> = quiet.iter().map(|b| b.saturating_add(150)).collect();
        let a = judge(&input("", &speech, 105.0, &quiet));
        let b = judge(&input("", &speech, 105.0, &loud));
        assert!(!a.parts.is_empty());
        assert_eq!(spans_of(&a), spans_of(&b));
    }

    #[test]
    fn a_padded_span_stays_inside_its_stretch_and_stretches_do_not_join() {
        // Two silences split by a sentence at 30-32; loud at both its edges.
        let speech = [r(30.0, 32.0)];
        let p = peaks(62.0, &[(27.0, 30.0, 200), (32.0, 35.0, 200)]);
        let j = judge(&input("", &speech, 62.0, &p));
        assert_eq!(spans_of(&j), [(23.0, 30.0), (32.0, 39.0)]);
    }

    #[test]
    fn fight_labels_keep_a_quiet_window_and_nothing_is_kept_without_any_signal() {
        let speech = [r(0.0, 2.0)];
        let piece = |start: f64, end: f64, fight: f64| Labelled {
            start,
            end,
            frames: 5,
            top: vec![Share {
                label: if fight > 0.0 {
                    "teamfight"
                } else {
                    "walking on the map"
                }
                .into(),
                score: fight.max(0.5),
            }],
            busy: Some(0.1),
            level_db: None,
        };
        let labels = Labels {
            model: "m".into(),
            kind: LabelKind::League,
            labels: vec![],
            prompts: vec![],
            busy_track: None,
            pieces: vec![
                piece(2.0, 32.0, 0.0),
                piece(32.0, 62.0, 0.9),
                piece(62.0, 92.0, 0.0),
                piece(92.0, 122.0, 0.0),
            ],
        };
        let flat = peaks(122.0, &[]);
        let mut i = input("", &speech, 122.0, &flat);
        i.labels = Some(&labels);
        let j = judge(&i);
        // The fight piece's 6 windows are the top quarter of 24 (all even on busy), padded.
        assert_eq!(spans_of(&j), [(30.0, 64.0)]);
        assert!(j.parts[0].state.contains("fight labels 90%"));
        // No peaks, no labels: nothing to tell windows apart, nothing kept.
        let none = Input {
            labels: None,
            peaks: None,
            ..i
        };
        assert!(judge(&none).parts.is_empty());
        // No silence at all: nothing.
        assert!(judge(&input("", &[r(0.0, 50.0)], 50.0, &flat))
            .parts
            .is_empty());
    }

    #[test]
    fn without_peaks_each_piece_busy_stands_for_its_windows() {
        let labels = Labels {
            model: "m".into(),
            kind: LabelKind::Generic,
            labels: vec![],
            prompts: vec![],
            busy_track: None,
            pieces: (0..4)
                .map(|k| Labelled {
                    start: k as f64 * 25.0,
                    end: (k + 1) as f64 * 25.0,
                    frames: 0,
                    top: vec![],
                    busy: Some(if k == 2 { 0.95 } else { 0.2 }),
                    level_db: None,
                })
                .collect(),
        };
        let i = Input {
            prompt: "",
            speech: &[],
            duration: 100.0,
            labels: Some(&labels),
            peaks: None,
        };
        assert_eq!(spans_of(&judge(&i)), [(48.0, 77.0)]);
    }

    #[test]
    fn a_span_well_above_the_best_left_out_is_sure() {
        use crate::keep::Fit;
        let speech = [r(0.0, 5.0)];
        // A ramp: each later window louder; strict keeps the top tenth.
        let loud: Vec<(f64, f64, u8)> = (0..20)
            .map(|k| {
                (
                    5.0 + k as f64 * 5.0,
                    10.0 + k as f64 * 5.0,
                    30 + k as u8 * 10,
                )
            })
            .collect();
        let p = peaks(105.0, &loud);
        let j = judge(&input("highlights", &speech, 105.0, &p));
        assert_eq!(spans_of(&j), [(93.0, 105.0)]);
        let f = Fit::default();
        assert!(f.keeps(j.parts[0].margin) && !f.uncertain(j.parts[0].margin));
        // Its best window ranks 0.95, the best left out 0.85: two thirds of the way to the top.
        assert!(
            (j.parts[0].margin - TOP_MARGIN * 2.0 / 3.0).abs() < 1e-3,
            "{:?}",
            j.parts
        );
    }

    #[test]
    fn windows_keep_their_speech_share_and_lean_toward_a_kept_span() {
        let j = Judged {
            model: NAME.into(),
            prompt: String::new(),
            version: VERSION,
            parts: vec![Judgement {
                start: 10.0,
                end: 15.0,
                kind: Kind::Silent,
                margin: 4.0,
                state: String::new(),
                hash: String::new(),
            }],
        };
        let w = |start: f64, speech: f64| Window {
            start,
            score: 0.33,
            speech,
            words: 0,
        };
        let mut windows = vec![w(0.0, 1.0), w(5.0, 0.0), w(10.0, 0.0), w(12.5, 0.2)];
        score_windows(&mut windows, 5.0, &j);
        let p = crate::keep::laya::p_keep(4.0, 1.0);
        let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
        assert_eq!(windows[0].score, 1.0);
        assert_eq!(windows[1].score, 0.0);
        assert_eq!(windows[2].score, r3(p));
        assert_eq!(windows[3].score, r3((2.5 * p + 2.5 * 0.2) / 5.0));
    }
}
