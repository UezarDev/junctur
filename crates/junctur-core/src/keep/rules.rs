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

use super::fit::Fit;
use super::parts::Kind;
use super::{Judged, Judgement};
use crate::edit::Window;
use crate::labels::busy;
use crate::labels::pieces::{self, Piece};
use crate::labels::Labels;
use crate::vad::Region;

/// The judge's name in `keep.json` ([`Judged::model`]).
pub const NAME: &str = "rules";
/// Bumped when the rules change, so a `keep.json` they wrote before is judged again. 2: whole
/// stretches, not padded windows; 3: ties at the line broken, a long silence never split (3e).
pub const VERSION: u32 = 3;
/// What a kept span is, carried by the user's answers ([`super::Answer::spans`]): bumped only when
/// spans stop meaning the same stretch, so an answer to another kind of span (Laya's parts, the padded
/// windows of 3d's first commit, which carry none) is not read as one to a stretch of the same times.
pub const SPANS: u32 = 1;
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

/// The words of each level, accents folded and lowercase ([`fold`]), English then Spanish, singular:
/// a phrase also matches with "s" or "es" after it, so "podcasts" and "entrevistas" read as theirs
/// (3e; whole words had made every plural Usual). The first level any phrase of which is in the
/// prompt wins, in this order, so "podcast highlights" is talk only and "highlights of the whole
/// stream" strict.
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
            "only fights",
            "only the fights",
            "just the fights",
            "only the best",
            "best moment",
            "best part",
            "best play",
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
            .find(|(_, words)| {
                words.iter().any(|w| {
                    ["", "s", "es"]
                        .iter()
                        .any(|plural| text.contains(&format!(" {w}{plural} ")))
                })
            })
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
    // The gaps themselves: their pieces joined back missed a shared end an ulp apart (3e).
    let stretches = pieces::gaps(input.speech, input.duration);
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
/// top `share` of the windows counted, a window lowest on both signals (score 0) never. So the share
/// still counts windows, the grain the recording is ranked at; a stretch is kept when it holds one,
/// and it keeps more time than the share says.
fn kept(looks: &[Look], stretches: &[(f64, f64)], share: f64) -> Vec<Judgement> {
    let keep = (share * looks.len() as f64).round() as usize;
    if keep == 0 {
        return Vec::new();
    }
    let busy = ranks(&looks.iter().map(|l| l.busy).collect::<Vec<_>>());
    let fight = ranks(&looks.iter().map(|l| Some(l.fight)).collect::<Vec<_>>());
    let score: Vec<f64> = busy.iter().zip(&fight).map(|(b, f)| b.max(*f)).collect();
    let low: Vec<f64> = busy.iter().zip(&fight).map(|(b, f)| b.min(*f)).collect();
    // Ties at the line are broken (3e): counting every tied window kept far more than the share
    // where a labels piece's windows share its fight share. High on both signals first, then earlier.
    let mut order: Vec<usize> = (0..looks.len()).filter(|&i| score[i] > 0.0).collect();
    order.sort_by(|&a, &b| {
        (score[b].total_cmp(&score[a]))
            .then(low[b].total_cmp(&low[a]))
            .then(a.cmp(&b))
    });
    order.truncate(keep);
    let mut counts = vec![false; looks.len()];
    let mut held = vec![false; stretches.len()];
    for &i in &order {
        counts[i] = true;
        held[looks[i].stretch] = true;
    }
    // A window tied at the line in a stretch already kept counts too: it keeps no more time, and a
    // stretch is no close call against its own windows.
    if let Some(&last) = order.last() {
        for (i, l) in looks.iter().enumerate() {
            counts[i] |= score[i] == score[last] && held[l.stretch];
        }
    }
    // The best window left out: a stretch's sureness is how far its best window stands above it (0
    // there, 1 at the top), so a stretch kept on a close call at the line is uncertain and a clear
    // one is not.
    let floor = (0..looks.len())
        .filter(|&i| !counts[i])
        .map(|i| score[i])
        .fold(0.0f64, f64::max);
    // Per stretch, its best counted window.
    let mut best: Vec<Option<usize>> = vec![None; stretches.len()];
    for (i, l) in looks.iter().enumerate() {
        if counts[i] && best[l.stretch].is_none_or(|b| score[i] > score[b]) {
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

/// Why a stretch is kept (its best window), in words, for `keep.json`: each signal only when there is
/// one (3e: an unknown busy read "busier than 0%"), the one that ranks it higher first.
fn why(l: &Look, busy: f64, fight: f64) -> String {
    let heard = l
        .busy
        .map(|_| format!("game audio busier than {:.0}% of the silence", busy * 100.0));
    let seen = (l.fight > 0.0).then(|| {
        format!(
            "fight labels {:.0}%, more than {:.0}% of the silence",
            l.fight * 100.0,
            fight * 100.0
        )
    });
    let said: Vec<String> = if fight > busy {
        seen.into_iter().chain(heard).collect()
    } else {
        heard.into_iter().chain(seen).collect()
    };
    said.join("; ")
}

/// The window scores (`len` seconds from each start): the speech share, the edit list's base, blended
/// toward a kept stretch's chance for the share of the window it covers, at the default fit the lane
/// shows them with.
pub fn score_windows(windows: &mut [Window], len: f64, j: &Judged) {
    let fit = Fit::default();
    for w in windows {
        let (mut sum, mut cover) = (0.0, 0.0);
        for (p, over) in super::overlap(&j.parts, w.start, w.start + len) {
            sum += over * fit.p(p.margin);
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
mod tests;
