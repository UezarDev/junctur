//! Smart keep's rules (phase 3d), which replaced Laya's answer: talk stays as the automatic cut leaves
//! it, and a silent stretch holding a window busier than this recording usually is (its game audio,
//! or 3a's fight labels) is suggested as a keep, whole: a long silence with a fight inside is kept in
//! full (Uezar). How many windows count is read from the prompt's words ([`Strictness`]). A module of its own because it is the whole judgement smart keep
//! now makes, pure (no file, no model) so every edge is a unit test; the stage reads the peaks and
//! writes the file around it.
//!
//! Rejected: Laya over the silent parts (its margin never followed busy or the fight labels in any
//! wording tried, ideas.md); a fixed busy number (busy is a rank in the recording already, and a dB
//! line would follow the game's volume); only the busy windows, padded and merged (built first;
//! Uezar chose the whole stretch, so the fight is kept with what leads to it); every silent part kept or cut in `keep.json` (the automatic cut keeps
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
/// Bumped when the rules change, so a `keep.json` they wrote before is judged again. 2: whole
/// stretches, not padded windows.
pub const VERSION: u32 = 2;
/// A silent stretch is looked at in windows of at most this many seconds (the score windows' grain).
pub const WINDOW: f64 = 5.0;
/// The labels that say "fight" (3a's generic and League lists).
const FIGHT: [&str; 2] = ["fighting", "teamfight"];
/// A stretch's margin at the top of the recording: its chance shows 0.5 at the line to 0.98 at the
/// top.
const TOP_MARGIN: f64 = 4.0;

/// How much of the silence a prompt keeps, read from its words ([`Strictness::read`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// No silent part: the talk is the video.
    TalkOnly,
    /// The stretches holding the busiest tenth of the silent windows.
    Strict,
    /// The busiest quarter: no word says otherwise.
    Usual,
    /// The busiest half: mostly the dead air goes.
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

    /// The share of the recording's silent windows that count: a stretch holding one is kept whole.
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
                    "{name}, every silent stretch holding the busiest {:.0}% of the silent windows",
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
    /// Which stretch it is in.
    stretch: usize,
    /// Busy (a rank among the recording's seconds), `None` when unknown.
    busy: Option<f64>,
    /// The fight labels' share, 0 without labels.
    fight: f64,
}

/// `input`'s busy silent stretches, as `keep.json` holds them: only keeps, in time order.
pub fn judge(input: &Input) -> Judged {
    let strictness = Strictness::read(input.prompt);
    let stretches: Vec<(f64, f64)> = joined(
        pieces::pieces(input.speech, input.duration)
            .into_iter()
            .map(|p| (p.start, p.end)),
        Join::Touching,
    );
    let looks = looks(&stretches, input);
    let parts = kept(&looks, &stretches, strictness.share());
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

/// The stretches holding a kept window, whole: each window's score the higher of its two ranks, the
/// line the score that leaves the top `share` of the windows at or above it, a window lowest on both
/// signals (score 0) never kept. So the share still counts windows, the grain the recording is ranked
/// at; a stretch is kept when it holds one, and it keeps more time than the share says.
fn kept(looks: &[Look], stretches: &[(f64, f64)], share: f64) -> Vec<Judgement> {
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
    let counts = |s: f64| s >= line && s > 0.0;
    // The best window left out: a stretch's sureness is how far its best window stands above it (0
    // there, 1 at the top), so a stretch kept on a close call at the line is uncertain and a clear
    // one is not, ties or not.
    let floor = score
        .iter()
        .copied()
        .filter(|s| !counts(*s))
        .fold(0.0f64, f64::max);
    // Per stretch, its best counted window.
    let mut best: Vec<Option<usize>> = vec![None; stretches.len()];
    for (i, l) in looks.iter().enumerate() {
        if counts(score[i]) && best[l.stretch].is_none_or(|b| score[i] > score[b]) {
            best[l.stretch] = Some(i);
        }
    }
    stretches
        .iter()
        .zip(best)
        .filter_map(|(&(start, end), b)| {
            let i = b?;
            let above = (score[i] - floor) / (1.0 - floor);
            Some(Judgement {
                start,
                end,
                kind: Kind::Silent,
                margin: (above * TOP_MARGIN * 1000.0).round() / 1000.0,
                state: why(&looks[i], busy[i], fight[i]),
                hash: String::new(),
            })
        })
        .collect()
}

/// Why a stretch is kept (its best window), in words, for `keep.json`.
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
/// toward a kept stretch's chance for the share of the window it covers.
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
    // From 0.0, not `sum`'s -0.0, so nothing kept reads "-0 s".
    let seconds = j.parts.iter().fold(0.0, |s, p| s + p.end - p.start);
    format!(
        "keep: talk as the cut leaves it; silence read as {}: {} busy silent stretches to keep, whole ({seconds:.0} s)",
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
    fn a_long_silence_with_a_busy_window_is_kept_whole_and_a_quiet_one_is_not() {
        // Talk at 0-10, 130-140 and 200-210; the 120 s silence between the first two is loud from 60
        // to 70 only, the other two silences are quiet.
        let speech = [r(0.0, 10.0), r(130.0, 140.0), r(200.0, 210.0)];
        let p = peaks(260.0, &[(60.0, 70.0, 200)]);
        let j = judge(&input("highlights", &speech, 260.0, &p));
        assert_eq!((j.model.as_str(), j.version), (NAME, VERSION));
        // The two loud windows count (strict: 10% of 46 windows, the rest tied at 0), and the
        // stretch that holds them is kept in full, not padded windows.
        assert_eq!(spans_of(&j), [(10.0, 130.0)]);
        assert!(j
            .parts
            .iter()
            .all(|p| p.kind == Kind::Silent && p.margin >= 0.0));
        assert!(j.parts[0].state.contains("game audio busier than"));
        assert_eq!(
            summary(&j),
            "keep: talk as the cut leaves it; silence read as strict, every silent stretch holding the busiest 10% of the silent windows: 1 busy silent stretches to keep, whole (120 s)"
        );
        // Talk only keeps nothing.
        let none = judge(&input("podcast", &speech, 260.0, &p));
        assert!(none.parts.is_empty());
        assert!(summary(&none).ends_with(
            "talk only, no silent part kept: 0 busy silent stretches to keep, whole (0 s)"
        ));
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
    fn stretches_either_side_of_a_sentence_are_kept_apart() {
        // Two silences split by a sentence at 30-32; loud at both its edges.
        let speech = [r(30.0, 32.0)];
        let p = peaks(62.0, &[(27.0, 30.0, 200), (32.0, 35.0, 200)]);
        let j = judge(&input("", &speech, 62.0, &p));
        assert_eq!(spans_of(&j), [(0.0, 30.0), (32.0, 62.0)]);
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
        // The fight piece's 6 windows are the top quarter of 24 (all even on busy): their stretch.
        assert_eq!(spans_of(&j), [(2.0, 122.0)]);
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
        assert_eq!(spans_of(&judge(&i)), [(0.0, 100.0)]);
    }

    #[test]
    fn a_stretch_well_above_the_best_window_left_out_is_sure_and_a_close_call_is_not() {
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
        assert_eq!(spans_of(&j), [(5.0, 105.0)]);
        let f = Fit::default();
        assert!(f.keeps(j.parts[0].margin) && !f.uncertain(j.parts[0].margin));
        // Its best window ranks 0.95, the best left out 0.85: two thirds of the way to the top.
        assert!(
            (j.parts[0].margin - TOP_MARGIN * 2.0 / 3.0).abs() < 1e-3,
            "{:?}",
            j.parts
        );
        // 41 silences of 4 s, one window each, each louder than the one before: the usual quarter
        // keeps the top 10; the lowest of them is one step above the best left out, a close call.
        let speech: Vec<Region> = (0..40)
            .map(|k| r(4.0 + 6.0 * k as f64, 6.0 + 6.0 * k as f64))
            .collect();
        let loud: Vec<(f64, f64, u8)> = (0..41)
            .map(|k| (6.0 * k as f64, 6.0 * k as f64 + 4.0, 20 + 5 * k as u8))
            .collect();
        let p = peaks(244.0, &loud);
        let j = judge(&input("", &speech, 244.0, &p));
        assert_eq!(j.parts.len(), 10);
        assert_eq!(spans_of(&j)[0], (186.0, 190.0));
        assert!(f.uncertain(j.parts[0].margin), "{:?}", j.parts[0]);
        assert!(!f.uncertain(j.parts[9].margin), "{:?}", j.parts[9]);
    }

    #[test]
    fn windows_keep_their_speech_share_and_lean_toward_a_kept_stretch() {
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
