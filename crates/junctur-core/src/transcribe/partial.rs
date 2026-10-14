//! The partial transcript a run appends to: its header, resuming an interrupted run from it, and
//! reading it while the run goes on.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{partial_path, ModelPref, Segment, TranscribeOptions};
use crate::runtime::Device;
use crate::Result;

/// First line of the partial file. A partial made with other languages or another model is not
/// resumed, so one transcript never mixes models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Header {
    pub(super) version: u32,
    pub(super) languages: String,
    pub(super) model: String,
    /// Where the run that started the partial ran; reported when a resume finds nothing left to do.
    pub(super) device: Device,
}

impl Header {
    pub(super) fn same_run(&self, other: &Header) -> bool {
        (self.version, &self.languages, &self.model)
            == (other.version, &other.languages, &other.model)
    }
}

/// A version-1 partial names no model, so it is not resumed.
pub(super) const VERSION: u32 = 2;

/// The chunks a run in progress (or an interrupted one) has finished, from its partial file, sorted
/// as the finished transcript sorts them, so a word keeps its place when the whole one replaces
/// this; `None` when there is no readable partial. A torn last line is dropped, as a resume drops
/// it. For the editor while the rest is transcribed.
pub fn so_far(project: &Path) -> Option<Vec<Segment>> {
    let (_, mut segs) = read_partial(&partial_path(project))?;
    sort(&mut segs);
    Some(segs)
}

/// [`so_far`] for a reader that asks again as the run appends: only the lines added since its last
/// read are parsed. What it parsed is checked against the file's start each time (length and hash),
/// so a partial written afresh (a new run, [`open_partial`]) is read again whole.
#[derive(Debug, Default)]
pub struct SoFar {
    path: std::path::PathBuf,
    /// Bytes parsed: whole lines, through the last newline read.
    parsed: usize,
    hash: u64,
    /// The parsed chunks in [`sort`]'s order.
    segs: Vec<Segment>,
}

impl SoFar {
    /// What [`so_far`] gives for `project` now.
    pub fn read(&mut self, project: &Path) -> Option<std::borrow::Cow<'_, [Segment]>> {
        use std::borrow::Cow;
        let path = partial_path(project);
        let Ok(body) = std::fs::read_to_string(&path) else {
            *self = Self::default();
            return None;
        };
        let same = self.path == path
            && self.parsed > 0
            && body.len() >= self.parsed
            && hash(&body.as_bytes()[..self.parsed]) == self.hash;
        if !same {
            *self = Self {
                path,
                ..Self::default()
            };
            // The header is the first line, and there is no partial without one that reads.
            let end = body.find('\n').map_or(body.len(), |i| i + 1);
            serde_json::from_str::<Header>(body[..end].lines().next()?).ok()?;
            if body[..end].ends_with('\n') {
                self.parsed = end;
            }
        }
        // Whole lines are kept; a last line without its newline is parsed but read again next time,
        // as it may still be being written.
        let rest = &body[self.parsed..];
        let whole = rest.rfind('\n').map_or(0, |i| i + 1);
        let new: Vec<Segment> = rest[..whole]
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        if !new.is_empty() {
            // Stable: the parsed ones, then the new, is the file's order among equal keys.
            self.segs.extend(new);
            sort(&mut self.segs);
        }
        self.parsed += whole;
        self.hash = hash(&body.as_bytes()[..self.parsed]);
        let torn = rest[whole..]
            .lines()
            .next()
            .and_then(|l| serde_json::from_str::<Segment>(l).ok());
        Some(match torn {
            Some(seg) => {
                let mut all = self.segs.clone();
                all.push(seg);
                sort(&mut all);
                Cow::Owned(all)
            }
            None => Cow::Borrowed(&self.segs),
        })
    }
}

pub(super) fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// The finished transcript's order: by start, then track.
pub(super) fn sort(segs: &mut [Segment]) {
    segs.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.track.cmp(&b.track)));
}

/// Where a run starts: the model to use, the header it writes, and the chunks an interrupted run
/// already finished (`started` is that run's header, when it is resumed).
pub(super) struct Resume {
    pub(super) pref: ModelPref,
    pub(super) want: Header,
    pub(super) started: Option<Header>,
    pub(super) done: Vec<Segment>,
}

pub(super) fn resume_point(partial: &Path, opts: &TranscribeOptions) -> Resume {
    let prior = read_partial(partial);
    // `Auto` resumes an interrupted run with the model it was started with, rather than restarting.
    let pref = match &prior {
        Some((h, _))
            if opts.model == ModelPref::Auto
                && h.version == VERSION
                && h.languages == opts.languages.key() =>
        {
            h.model.parse().unwrap_or(ModelPref::Auto)
        }
        _ => opts.model,
    };
    let want = Header {
        version: VERSION,
        languages: opts.languages.key(),
        model: pref.name(),
        device: Device::Cpu,
    };
    let (started, done) = match prior {
        Some((h, segs)) if h.same_run(&want) => (Some(h), segs),
        _ => (None, Vec::new()),
    };
    Resume {
        pref,
        want,
        started,
        done,
    }
}

/// An earlier run's header and the chunks it finished, or None when there is no readable partial
/// (missing, or a header this version cannot read). A torn last line (killed mid-write) is dropped.
pub(super) fn read_partial(path: &Path) -> Option<(Header, Vec<Segment>)> {
    let body = std::fs::read_to_string(path).ok()?;
    let mut lines = body.lines();
    let h = serde_json::from_str::<Header>(lines.next()?).ok()?;
    Some((
        h,
        lines.filter_map(|l| serde_json::from_str(l).ok()).collect(),
    ))
}

/// Write the partial file afresh (`header`, then the chunks already done) and open it for
/// appending. Rewritten rather than appended to: a killed run leaves a torn last line, which the
/// next chunk would share and be lost with.
pub(super) fn open_partial(
    path: &Path,
    header: &Header,
    done: &[Segment],
) -> Result<std::fs::File> {
    let mut body = serde_json::to_string(header)? + "\n";
    for seg in done {
        body += &serde_json::to_string(seg)?;
        body.push('\n');
    }
    // Through a temp file, so a kill now still leaves the old partial to resume from.
    crate::output::write_replacing(path, body)?;
    Ok(std::fs::OpenOptions::new().append(true).open(path)?)
}
