//! The detectors a recipe's tools ask, over what a recording's analysis left in its project folder:
//! the speech regions and envelope, the edit list's speakers, the transcript's words, the Like this
//! tab's examples (7b), and the picture ([`super::still`], [`super::look`]).

use crate::edit::{self, EditList};
use crate::labels::busy;
use crate::like::{self, mel};
use crate::pipeline::{self, Project};
use crate::recipe::sets::{self, Spans};
use crate::recipe::{Signals, ANYONE};
use crate::text::folded;
use crate::transcribe::{self, Segment, Word};
use crate::{Error, Result};

use super::{look, still};

/// Words of one remark are at most this far apart: a longer pause ends what someone was saying.
const REMARK_GAP_S: f64 = 0.8;

/// The detectors over project `p` and its edit list, which knows the speakers and the length.
pub struct Detectors<'a> {
    pub p: &'a Project,
    pub list: &'a EditList,
    /// What was read for the first time and how long it took, for the step's notes.
    pub notes: Vec<String>,
    /// The transcript's words, read once a request ([`Self::segments`]).
    pub segments: Option<Vec<Segment>>,
}

fn refuse(why: impl Into<String>) -> Error {
    Error::Recipe(why.into())
}

impl Detectors<'_> {
    fn duration(&self) -> f64 {
        self.list.duration
    }

    fn segments(&mut self) -> Result<&[Segment]> {
        if self.segments.is_none() {
            self.segments = transcribe::load(&self.p.dir)
                .map(|t| t.segments)
                .or_else(|| transcribe::so_far(&self.p.dir))
                .filter(|s| !s.is_empty());
        }
        self.segments
            .as_deref()
            .ok_or_else(|| refuse("there is no transcript yet, so no words to look for"))
    }

    fn timed<T>(&mut self, what: &str, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let t = std::time::Instant::now();
        let r = f(self)?;
        let s = t.elapsed().as_secs_f64();
        if s >= 1.0 {
            self.notes.push(format!("{what}: {s:.1} s"));
        }
        Ok(r)
    }
}

/// The speaker ids `name` names in `list`: one whose folded name is it, else the one whose name
/// holds it; every speaker for [`ANYONE`].
pub fn speaker_ids(list: &EditList, name: &str) -> Result<Vec<u32>> {
    let names = &list.speakers.names;
    let fold = |s: &str| folded(s).trim().to_string();
    let want = fold(name);
    if want == ANYONE {
        return Ok(names.iter().map(|n| n.id).collect());
    }
    if let Some(n) = names.iter().find(|n| fold(&n.name) == want) {
        return Ok(vec![n.id]);
    }
    let holding: Vec<u32> = names
        .iter()
        .filter(|n| {
            !want.is_empty() && (fold(&n.name).contains(&want) || want.contains(&fold(&n.name)))
        })
        .map(|n| n.id)
        .collect();
    if holding.len() == 1 {
        return Ok(holding);
    }
    let all: Vec<&str> = names.iter().map(|n| n.name.as_str()).collect();
    Err(refuse(if all.is_empty() {
        format!("there are no speakers in this recording to find {name} among")
    } else {
        format!(
            "nobody here is called {name}: the speakers are {}",
            all.join(", ")
        )
    }))
}

/// Where the folded tokens of `phrase` are said in `words` (one track's, in order): each saying, or
/// with `remark` the words around it no pause of [`REMARK_GAP_S`] apart. A one-word phrase of four
/// letters or more also matches words it starts ("food" in "foods").
pub fn sayings(words: &[&Word], phrase: &str, remark: bool) -> Spans {
    let want: Vec<String> = folded(phrase)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if want.is_empty() {
        return Vec::new();
    }
    let toks: Vec<(String, usize)> = words
        .iter()
        .enumerate()
        .flat_map(|(i, w)| {
            folded(&w.text)
                .split_whitespace()
                .map(|t| (t.to_string(), i))
                .collect::<Vec<_>>()
        })
        .collect();
    let same = |t: &str, w: &str| t == w || (want.len() == 1 && w.len() >= 4 && t.starts_with(w));
    let mut out = Vec::new();
    for k in 0..toks.len() {
        let Some(run) = toks.get(k..k + want.len()) else {
            break;
        };
        if !run.iter().zip(&want).all(|((t, _), w)| same(t, w)) {
            continue;
        }
        let (mut a, mut b) = (run[0].1, run[run.len() - 1].1);
        if remark {
            while a > 0 && words[a].start - words[a - 1].end < REMARK_GAP_S {
                a -= 1;
            }
            while b + 1 < words.len() && words[b + 1].start - words[b].end < REMARK_GAP_S {
                b += 1;
            }
        }
        out.push((words[a].start, words[b].end.max(words[a].start)));
    }
    sets::normal(out)
}

impl Signals for Detectors<'_> {
    fn silence(&mut self, range: (f64, f64)) -> Result<Spans> {
        let speech = self
            .p
            .speech()
            .ok_or_else(|| refuse("the speech is not found yet: analyse the recording first"))?;
        let env = pipeline::envelope(&self.p.dir).unwrap_or_default();
        let talk: Spans = edit::keep_ranges(
            &speech.all_regions(),
            &env,
            self.duration(),
            &self.list.settings,
        )
        .iter()
        .map(|r| (r.start, r.end))
        .collect();
        Ok(sets::not(&talk, range))
    }

    fn still(&mut self, range: (f64, f64)) -> Result<Spans> {
        let (dir, file, d) = (self.p.dir.clone(), self.p.file.clone(), self.duration());
        self.timed("picture read", |_| still::still(&dir, &file, d, range))
    }

    fn speaker(&mut self, name: &str, _: (f64, f64)) -> Result<Spans> {
        let ids = speaker_ids(self.list, name)?;
        Ok(sets::normal(
            self.list
                .speakers
                .turns
                .iter()
                .filter(|t| ids.contains(&t.speaker))
                .map(|t| (t.start, t.end))
                .collect(),
        ))
    }

    fn words(&mut self, phrase: &str, remark: bool, _: (f64, f64)) -> Result<Spans> {
        let segs = self.segments()?;
        let mut tracks: Vec<usize> = segs.iter().map(|s| s.track).collect();
        tracks.sort_unstable();
        tracks.dedup();
        let mut out = Spans::new();
        for t in tracks {
            let mut words: Vec<&Word> = segs
                .iter()
                .filter(|s| s.track == t)
                .flat_map(|s| &s.words)
                .collect();
            words.sort_by(|a, b| a.start.total_cmp(&b.start));
            out = sets::or(&out, &sayings(&words, phrase, remark));
        }
        Ok(out)
    }

    fn like_this(&mut self, range: (f64, f64)) -> Result<Spans> {
        let l = like::load(&self.p.dir)?;
        if !l.examples.iter().any(|e| e.positive) {
            return Err(refuse(
                "no sound is marked to look for: mark one in the Like this tab (the selection, or Add at the playhead), then ask again",
            ));
        }
        let report = pipeline::cached_tracks(self.p);
        let track = l
            .track
            .or_else(|| report.as_ref().and_then(busy::track).map(|t| t.track))
            .unwrap_or(0);
        let (dir, file, d) = (self.p.dir.clone(), self.p.file.clone(), self.duration());
        let (spec, _) = self.timed("sound read", |_| mel::of_track(&dir, &file, track, d))?;
        let found = like::find(&spec, &l.examples, Some(range))?;
        Ok(sets::normal(
            found
                .iter()
                .filter(|m| m.score >= l.threshold)
                .map(|m| (m.point, m.end.max(m.point + 0.05)))
                .collect(),
        ))
    }

    fn label(&mut self, picture: &str, range: (f64, f64)) -> Result<Spans> {
        let p = self.p;
        self.timed("keyframes looked at", |_| {
            look::shows(&p.dir, &p.file, &p.cache_dir, picture, range)
        })
    }
}
