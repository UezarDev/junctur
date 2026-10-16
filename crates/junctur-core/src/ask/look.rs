//! "The picture shows ...": the recording's keyframes (OBS writes one every 2 s) embedded once by
//! SigLIP 2's vision tower and kept in the project folder ([`FILE`]), then each read against the
//! asked picture and the built-in generic labels (the labels stage's own list, 3a): a keyframe shows
//! it when the asked picture is its likeliest. Rejected: the labels stage's kept frames, which cover
//! only the silent pieces, and SigLIP's own per-label sigmoid, which puts every label of a game frame
//! near 0 (the labels' notes).

use std::path::{Path, PathBuf};

use crate::ffmpeg::frames;
use crate::labels::{model, Kind};
use crate::runtime::DevicePref;
use crate::{Error, Result};

/// The file in the project folder.
pub const FILE: &str = "look.bin";
const MAGIC: &[u8; 8] = b"JLOOK001";
/// A keyframe stands for this long either side of it: half OBS's keyframe interval.
const HALF: f64 = 1.0;
/// The asked picture's least share of a keyframe, over itself and the generic labels: likeliest
/// alone let the HUD of a game with no such screen pass (on the League sample, "an open
/// inventory" took a quarter of the recording at likeliest; see the 7c record).
const SURE: f32 = 0.5;

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// The keyframes embedded so far: how far the pass reached, then each time and embedding.
#[derive(Debug, Default, PartialEq)]
struct Looks {
    until: f64,
    frames: Vec<(f64, Vec<f32>)>,
}

fn load(dir: &Path) -> Looks {
    let Ok(b) = std::fs::read(path(dir)) else {
        return Looks::default();
    };
    read(&b).unwrap_or_default()
}

fn read(b: &[u8]) -> Option<Looks> {
    let each = 8 + model::DIM * 4;
    if b.len() < 16 || &b[..8] != MAGIC || !(b.len() - 16).is_multiple_of(each) {
        return None;
    }
    let until = f64::from_le_bytes(b[8..16].try_into().ok()?);
    let frames = b[16..]
        .chunks_exact(each)
        .map(|c| {
            let t = f64::from_le_bytes(c[..8].try_into().expect("8 bytes"));
            let e = c[8..]
                .chunks_exact(4)
                .map(|x| f32::from_le_bytes(x.try_into().expect("4 bytes")))
                .collect();
            (t, e)
        })
        .collect();
    Some(Looks { until, frames })
}

fn write(l: &Looks) -> Vec<u8> {
    let mut b = Vec::with_capacity(16 + l.frames.len() * (8 + model::DIM * 4));
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&l.until.to_le_bytes());
    for (t, e) in &l.frames {
        b.extend_from_slice(&t.to_le_bytes());
        for x in e {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    b
}

/// The keyframes of `file` up to `b` seconds embedded, the ones kept read back and the pass going
/// on from where the last one stopped.
fn looks(dir: &Path, file: &Path, cache_dir: &Path, b: f64) -> Result<Looks> {
    let mut l = load(dir);
    if l.until >= b {
        return Ok(l);
    }
    let mut vision = model::Vision::open(cache_dir, DevicePref::Auto)?;
    let from = l.until;
    let mut batch: Vec<(f64, Vec<u8>)> = Vec::new();
    let mut failed: Option<Error> = None;
    let mut stopped = false;
    let mut flush = |batch: &mut Vec<(f64, Vec<u8>)>, l: &mut Looks| -> Result<()> {
        let px: Vec<&[u8]> = batch.iter().map(|(_, p)| p.as_slice()).collect();
        for ((t, _), e) in batch.iter().zip(vision.embed(&px)?) {
            l.frames.push((*t, e));
        }
        batch.clear();
        Ok(())
    };
    frames::keyframes(file, model::SIZE, &mut |t, px| {
        if t <= from && from > 0.0 {
            return true;
        }
        if t > b + HALF {
            stopped = true;
            return false;
        }
        batch.push((t, px.to_vec()));
        if batch.len() == model::BATCH {
            if let Err(e) = flush(&mut batch, &mut l) {
                failed = Some(e);
                return false;
            }
        }
        true
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    flush(&mut batch, &mut l)?;
    // A pass that was not stopped reached the end: the whole recording is read. A stopped one
    // took every keyframe up to b + HALF.
    l.until = if stopped { b + HALF } else { f64::INFINITY };
    crate::output::write_replacing(&path(dir), write(&l))?;
    Ok(l)
}

/// What SigLIP reads for `picture`: in the generic labels' own words.
fn prompt(picture: &str) -> String {
    format!("a video game screen showing {picture}")
}

/// Where the picture shows `picture` in `range`: each keyframe whose likeliest label (of `picture`
/// and the generic ones) it is, by at least [`SURE`], [`HALF`] either side.
pub fn shows(
    dir: &Path,
    file: &Path,
    cache_dir: &Path,
    picture: &str,
    range: (f64, f64),
) -> Result<Vec<(f64, f64)>> {
    let l = looks(dir, file, cache_dir, range.1)?;
    let mut prompts = vec![prompt(picture)];
    prompts.extend(Kind::Generic.builtin().iter().map(|(_, p)| p.to_string()));
    let texts = model::texts(cache_dir, &prompts)?;
    Ok(l.frames
        .iter()
        .filter(|(t, _)| *t + HALF > range.0 && *t - HALF < range.1)
        .filter(|(_, e)| {
            let s = model::shares(e, &texts);
            s[0] >= SURE && s.iter().skip(1).all(|x| *x < s[0])
        })
        .map(|(t, _)| (t - HALF, t + HALF))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_read_back_as_written() {
        let l = Looks {
            until: 12.5,
            frames: vec![(0.0, vec![0.5; model::DIM]), (2.0, vec![-1.0; model::DIM])],
        };
        assert_eq!(read(&write(&l)), Some(l));
        assert_eq!(read(b"JLOOK001"), None);
        assert_eq!(read(b"nonsense, not looks"), None);
    }
}
