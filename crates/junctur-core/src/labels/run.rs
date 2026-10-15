//! One labelling run: the frames of the silent pieces embedded once (and kept, so a changed label
//! list only scores again), scored against the list, and the busy score read beside them.

use std::io::{Read, Write};
use std::path::Path;

use super::busy::{self, BusyTrack};
use super::model::{self, Vision, DIM, SIZE};
use super::pieces::{self, Piece};
use super::{load, save, Label, Labelled, Labels, Settings, Share, TOP};
use crate::ffmpeg::frames;
use crate::runtime::DevicePref;
use crate::tracks::TrackReport;
use crate::vad::Region;
use crate::{output, Error, Result};

/// The frames' embeddings beside `labels.json`, for the pieces it names.
const FRAMES_FILE: &str = "label_frames.f32";
const MAGIC: &[u8; 8] = b"JLFRAME1";

/// What a run labels.
pub struct Job<'a> {
    pub file: &'a Path,
    /// The project folder.
    pub dir: &'a Path,
    pub cache_dir: &'a Path,
    pub report: &'a TrackReport,
    /// Every speech track's regions together.
    pub speech: &'a [Region],
    pub settings: &'a Settings,
    /// Where the vision tower may run.
    pub device: DevicePref,
    /// Make them again: nothing earlier is reused, frames and busy scores included (`--force`).
    pub force: bool,
}

/// One frame looked at: its piece, its time and its embedding.
#[derive(Debug, Clone, PartialEq)]
struct Look {
    piece: usize,
    t: f64,
    embedding: Vec<f32>,
}

/// The labels in `job`'s project folder, when they answer its settings, speech and busy track as they
/// are now; never when it is forced.
pub fn reused(job: &Job) -> Option<Labels> {
    if job.force {
        return None;
    }
    super::current(job.dir, job.settings, job.speech, job.report)
}

/// Labels `job`'s recording and writes them. Frames are decoded only when the pieces changed or no
/// embeddings are kept; otherwise the kept ones are scored against the list again, and the busy
/// scores of the same pieces on the same track are kept too (the audio is a minute's decode alone).
pub fn run(job: &Job, log: &mut dyn FnMut(&str)) -> Result<Labels> {
    let list = job.settings.labels();
    let pieces = pieces::pieces(job.speech, job.report.duration);
    let before = load(job.dir)
        .filter(|_| !job.force)
        .filter(|l| same_pieces(l, &pieces));
    let kept = before
        .as_ref()
        .filter(|l| l.model == model::NAME)
        .and_then(|_| read_looks(&job.dir.join(FRAMES_FILE), pieces.len()));
    let fresh = kept.is_none();
    let busy_track = busy::track(job.report);
    let busy_kept: Option<Vec<Option<(f64, f64)>>> = before
        .as_ref()
        .filter(|l| l.busy_track == busy_track && busy_track.is_some())
        .map(|l| l.pieces.iter().map(|p| p.busy.zip(p.level_db)).collect());
    let (looks, peaks) = std::thread::scope(|s| {
        // The game audio is read while the frames are: two ffmpegs, one on the card.
        let peaks = busy_track
            .filter(|_| busy_kept.is_none())
            .map(|b| s.spawn(move || busy::peaks(job.dir, job.file, job.report, b.track)));
        let looks = match kept {
            Some(l) => {
                log(&format!(
                    "labels: {} frames looked at before, scored again",
                    l.len()
                ));
                Ok(l)
            }
            None => look(job, &pieces, log),
        };
        let peaks = peaks.map(|h| h.join().unwrap_or_else(|_| Err(panicked())));
        (looks, peaks)
    });
    let looks = looks?;
    if fresh {
        write_looks(&job.dir.join(FRAMES_FILE), &looks)?;
    }
    let (busy, busy_track) = busy_of(busy_kept, peaks, busy_track, &pieces, log);
    // No frame to score (no silent piece): the text tower is not fetched for nothing.
    let texts = if looks.is_empty() {
        Vec::new()
    } else {
        let prompts: Vec<String> = list.iter().map(|l| l.prompt.clone()).collect();
        model::texts(job.cache_dir, &prompts)?
    };
    let labels = score(
        &list,
        &pieces,
        &looks,
        &texts,
        &busy,
        busy_track,
        job.settings,
    );
    save(job.dir, &labels)?;
    Ok(labels)
}

/// Each piece's busy score and the track it was read from: kept, or from the peaks read. A read that
/// failed leaves every piece without one and records no track, so the next run reads it again
/// ([`super::answers`]).
fn busy_of(
    kept: Option<Vec<Option<(f64, f64)>>>,
    peaks: Option<Result<Vec<u8>>>,
    track: Option<BusyTrack>,
    pieces: &[Piece],
    log: &mut dyn FnMut(&str),
) -> (Vec<Option<(f64, f64)>>, Option<BusyTrack>) {
    match (kept, peaks) {
        (Some(b), _) => (b, track),
        (None, Some(Ok(p))) => (busy::scores(&p, pieces), track),
        (None, Some(Err(e))) => {
            log(&format!("labels: the busy score could not be read ({e})"));
            (vec![None; pieces.len()], None)
        }
        (None, None) => (vec![None; pieces.len()], None),
    }
}

fn panicked() -> Error {
    Error::Model("reading the game audio stopped on an internal error".into())
}

fn same_pieces(l: &Labels, pieces: &[Piece]) -> bool {
    l.pieces.len() == pieces.len() && l.pieces.iter().zip(pieces).all(|(a, b)| a.piece() == *b)
}

/// Every piece's frames embedded: the keyframes that stand for its looks, then the looks no
/// keyframe stands for, decoded by seeking.
fn look(job: &Job, pieces: &[Piece], log: &mut dyn FnMut(&str)) -> Result<Vec<Look>> {
    if pieces.is_empty() {
        return Ok(Vec::new());
    }
    let mut vision = Vision::open(job.cache_dir, job.device)?;
    log(&format!("labels: SigLIP 2 on {:?}", vision.device));
    let total = job.report.duration.max(1.0);
    // Keyframes inside a piece, at most one a second (an all-keyframe video would otherwise be
    // embedded thirty times over), embedded a batch at a time as they come.
    let mut found: Vec<(usize, f64, Vec<f32>)> = Vec::new();
    let mut batch: Vec<(usize, f64, Vec<u8>)> = Vec::new();
    let mut last = f64::NEG_INFINITY;
    let mut told = 0;
    let mut failed = None;
    let flush = |batch: &mut Vec<(usize, f64, Vec<u8>)>,
                 found: &mut Vec<(usize, f64, Vec<f32>)>,
                 vision: &mut Vision|
     -> Result<()> {
        let px: Vec<&[u8]> = batch.iter().map(|(_, _, f)| f.as_slice()).collect();
        let e = vision.embed(&px)?;
        found.extend(batch.drain(..).zip(e).map(|((p, t, _), e)| (p, t, e)));
        Ok(())
    };
    let past = pieces.last().map_or(0.0, |p| p.end);
    frames::keyframes(job.file, SIZE, &mut |t, px| {
        // Nothing past the last silent piece is looked at.
        if t >= past {
            return false;
        }
        let tenth = (t / total * 10.0) as usize;
        if tenth > told {
            told = tenth;
            log(&format!("labels: {}% of the video looked at", tenth * 10));
        }
        let at = pieces.partition_point(|p| p.end <= t);
        if at < pieces.len() && pieces[at].start <= t && t - last >= pieces::NEAR {
            last = t;
            batch.push((at, t, px.to_vec()));
            if batch.len() == model::BATCH {
                if let Err(e) = flush(&mut batch, &mut found, &mut vision) {
                    failed = Some(e);
                    return false;
                }
            }
        }
        true
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    flush(&mut batch, &mut found, &mut vision)?;
    let times: Vec<f64> = found.iter().map(|(_, t, _)| *t).collect();
    let plan = pieces::plan(pieces, &times);
    let mut looks: Vec<Look> = Vec::new();
    for (p, keys) in plan.keyframes.iter().enumerate() {
        for &i in keys {
            looks.push(Look {
                piece: p,
                t: found[i].1,
                embedding: found[i].2.clone(),
            });
        }
    }
    let seeks: Vec<(usize, f64)> = plan
        .seek
        .iter()
        .enumerate()
        .flat_map(|(p, ts)| ts.iter().map(move |&t| (p, t)))
        .collect();
    if !seeks.is_empty() {
        log(&format!(
            "labels: {} looks have no keyframe near, decoded one by one",
            seeks.len()
        ));
    }
    let mut unread = 0;
    for chunk in seeks.chunks(model::BATCH) {
        let mut got: Vec<(usize, f64, Vec<u8>)> = Vec::new();
        for &(p, t) in chunk {
            // One frame that will not decode is one look fewer, not the run's frames lost.
            match frames::frame_at(job.file, t, SIZE) {
                Ok(Some(px)) => got.push((p, t, px)),
                Ok(None) => {}
                Err(_) => unread += 1,
            }
        }
        let px: Vec<&[u8]> = got.iter().map(|(_, _, f)| f.as_slice()).collect();
        for ((p, t, _), e) in got.iter().zip(vision.embed(&px)?) {
            looks.push(Look {
                piece: *p,
                t: *t,
                embedding: e,
            });
        }
    }
    if unread > 0 {
        log(&format!(
            "labels: {unread} looks could not be decoded, left out"
        ));
    }
    looks.sort_by(|a, b| a.t.total_cmp(&b.t));
    log(&format!(
        "labels: {} frames looked at in {} pieces",
        looks.len(),
        pieces.len()
    ));
    Ok(looks)
}

/// The labels file from the looks: each piece's shares averaged over its frames, the [`TOP`] kept.
fn score(
    list: &[Label],
    pieces: &[Piece],
    looks: &[Look],
    texts: &[Vec<f32>],
    busy: &[Option<(f64, f64)>],
    busy_track: Option<BusyTrack>,
    settings: &Settings,
) -> Labels {
    let mut sums = vec![vec![0f64; list.len()]; pieces.len()];
    let mut counts = vec![0usize; pieces.len()];
    for l in looks {
        for (s, x) in sums[l.piece]
            .iter_mut()
            .zip(model::shares(&l.embedding, texts))
        {
            *s += x as f64;
        }
        counts[l.piece] += 1;
    }
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    let pieces = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let n = counts[i];
            let mut top: Vec<Share> = if n == 0 {
                Vec::new()
            } else {
                sums[i]
                    .iter()
                    .zip(list)
                    .map(|(s, l)| Share {
                        label: l.text.clone(),
                        score: round(s / n as f64),
                    })
                    .collect()
            };
            top.sort_by(|a, b| b.score.total_cmp(&a.score));
            top.truncate(TOP);
            let b = busy.get(i).copied().flatten();
            Labelled {
                start: p.start,
                end: p.end,
                frames: n,
                top,
                busy: b.map(|(x, _)| round(x)),
                level_db: b.map(|(_, d)| (d * 10.0).round() / 10.0),
            }
        })
        .collect();
    Labels {
        model: model::NAME.into(),
        kind: settings.kind,
        labels: list.iter().map(|l| l.text.clone()).collect(),
        prompts: list.iter().map(|l| l.prompt.clone()).collect(),
        busy_track,
        pieces,
    }
}

/// Writes the looks: [`MAGIC`], the embedding length and the count (u32), then per look its piece
/// (u32), time (f64) and embedding (f32s), little-endian.
fn write_looks(path: &Path, looks: &[Look]) -> Result<()> {
    output::write_replacing_with(path, |file| {
        let mut f = std::io::BufWriter::new(&mut *file);
        f.write_all(MAGIC)?;
        f.write_all(&(DIM as u32).to_le_bytes())?;
        f.write_all(&(looks.len() as u32).to_le_bytes())?;
        for l in looks {
            f.write_all(&(l.piece as u32).to_le_bytes())?;
            f.write_all(&l.t.to_le_bytes())?;
            for x in &l.embedding {
                f.write_all(&x.to_le_bytes())?;
            }
        }
        f.flush()
    })?;
    Ok(())
}

/// The looks [`write_looks`] wrote, when the file reads whole and every look's piece is one of
/// `pieces`; `None` otherwise.
fn read_looks(path: &Path, pieces: usize) -> Option<Vec<Look>> {
    let mut b = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut b).ok()?;
    let u32_at = |at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    if b.get(..8)? != MAGIC || u32_at(8)? as usize != DIM {
        return None;
    }
    let count = u32_at(12)? as usize;
    let each = 4 + 8 + 4 * DIM;
    if b.len() != 16 + count * each {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let at = 16 + k * each;
        let piece = u32_at(at)? as usize;
        if piece >= pieces {
            return None;
        }
        let t = f64::from_le_bytes(b[at + 4..at + 12].try_into().ok()?);
        let embedding = b[at + 12..at + each]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        out.push(Look {
            piece,
            t,
            embedding,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_busy_read_records_no_track_so_the_next_run_reads_it_again() {
        let t = Some(BusyTrack {
            track: 1,
            role: crate::tracks::Role::GamePc,
        });
        let p = [Piece {
            start: 0.0,
            end: 2.0,
        }];
        let mut log = |_: &str| {};
        let (b, track) = busy_of(None, Some(Err(panicked())), t, &p, &mut log);
        assert_eq!((b, track), (vec![None], None));
        let (b, track) = busy_of(None, Some(Ok(vec![100; 200])), t, &p, &mut log);
        assert!(b[0].is_some());
        assert_eq!(track, t);
        let kept = vec![Some((0.5, -20.0))];
        assert_eq!(
            busy_of(Some(kept.clone()), None, t, &p, &mut log),
            (kept, t)
        );
    }

    #[test]
    fn looks_read_back_as_written_and_a_bad_file_is_none() {
        let d = crate::test_support::scratch("labels", "looks");
        let path = d.join(FRAMES_FILE);
        let looks = vec![
            Look {
                piece: 0,
                t: 1.5,
                embedding: vec![0.25; DIM],
            },
            Look {
                piece: 2,
                t: 61.25,
                embedding: (0..DIM).map(|i| i as f32).collect(),
            },
        ];
        write_looks(&path, &looks).unwrap();
        assert_eq!(read_looks(&path, 3), Some(looks.clone()));
        // A piece the labels no longer have.
        assert_eq!(read_looks(&path, 2), None);
        std::fs::write(&path, b"JLFRAME1junk").unwrap();
        assert_eq!(read_looks(&path, 3), None);
        assert_eq!(read_looks(&d.join("none"), 3), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_piece_scores_the_mean_of_its_frames_and_keeps_the_top_three() {
        let list: Vec<Label> = ["a", "b", "c", "d"]
            .iter()
            .map(|t| Label {
                text: t.to_string(),
                prompt: t.to_string(),
            })
            .collect();
        let axis = |i: usize| {
            let mut v = vec![0f32; DIM];
            v[i] = 1.0;
            v
        };
        let texts: Vec<Vec<f32>> = (0..4).map(axis).collect();
        let pieces = [
            Piece {
                start: 0.0,
                end: 4.0,
            },
            Piece {
                start: 10.0,
                end: 12.0,
            },
        ];
        let looks = vec![
            Look {
                piece: 0,
                t: 1.0,
                embedding: axis(1),
            },
            Look {
                piece: 0,
                t: 3.0,
                embedding: axis(1),
            },
        ];
        let l = score(
            &list,
            &pieces,
            &looks,
            &texts,
            &[Some((0.5, -20.04)), None],
            None,
            &Settings::default(),
        );
        let p = &l.pieces[0];
        assert_eq!(p.frames, 2);
        assert_eq!(p.top.len(), TOP);
        assert_eq!(p.top[0].label, "b");
        assert!(p.top[0].score > 0.99);
        assert_eq!((p.busy, p.level_db), (Some(0.5), Some(-20.0)));
        // A piece with no frame has no labels, and is still listed.
        assert_eq!(l.pieces[1].frames, 0);
        assert!(l.pieces[1].top.is_empty());
        assert_eq!(l.prompts, vec!["a", "b", "c", "d"]);
    }
}
