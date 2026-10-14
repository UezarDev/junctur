//! Every audio track's loudness for the app's track lanes: a byte per track per [`HOP`], the hop's
//! peak on a -60..0 dBFS scale. Not a pipeline stage: the lanes want every track, the CLI none.
//! Kept as [`FILE`]: a 16-byte header, then every track's byte for hop 0, then hop 1, ...

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ffmpeg::PcmReader;
use crate::{output, Error, Result};

/// The file in a project folder.
pub const FILE: &str = "waves.u8";
/// Peaks per second.
pub const RATE: u32 = 50;
/// Seconds per peak.
pub const HOP: f64 = 1.0 / RATE as f64;
/// The rate tracks are decoded at: 160 samples per hop.
const DECODE_RATE: u32 = 8_000;
const PER_HOP: usize = (DECODE_RATE / RATE) as usize;
/// The quietest level drawn, in dBFS: byte 0. Byte 255 is 0 dBFS.
pub const FLOOR_DB: f32 = -60.0;
const MAGIC: &[u8; 8] = b"JWAVES1\n";
/// How much is decoded between two looks at `stop` and two progress calls: 10 s.
const READ_HOPS: usize = RATE as usize * 10;

/// The smallest block of hops the pyramid keeps a max for: smaller ones are read from the bytes.
const BLOCK: usize = 8;

/// Every track's peaks so far.
#[derive(Debug, Clone, Default)]
pub struct Waves {
    tracks: usize,
    /// Hop-major: `data[hop * tracks + track]`. Private, and only appended to ([`Waves::extend`]):
    /// a change anywhere else would leave the pyramid stale.
    data: Vec<u8>,
    /// Each track's max over whole aligned blocks of `BLOCK << level` hops, block-major like
    /// `data`: a zoomed-out lane reads a column from a few blocks instead of every hop in it.
    levels: Vec<Vec<u8>>,
}

/// The peaks are the value: the pyramid is only a faster way to read them.
impl PartialEq for Waves {
    fn eq(&self, other: &Self) -> bool {
        self.tracks == other.tracks && self.data == other.data
    }
}

impl Waves {
    pub fn new(tracks: usize) -> Self {
        Self {
            tracks,
            data: Vec::new(),
            levels: Vec::new(),
        }
    }

    /// Appends whole hops (hop-major) and the pyramid's blocks they complete.
    pub fn extend(&mut self, more: &[u8]) {
        self.data.extend_from_slice(more);
        self.build();
    }

    /// Adds every block the hops now complete, level by level.
    fn build(&mut self) {
        let t = self.tracks;
        if t == 0 {
            return;
        }
        let hops = self.hops();
        let mut level = 0;
        while hops >= BLOCK << level {
            if self.levels.len() == level {
                self.levels.push(Vec::new());
            }
            let want = hops / (BLOCK << level);
            let (below, at) = self.levels.split_at_mut(level);
            let out = &mut at[0];
            for b in out.len() / t..want {
                for k in 0..t {
                    let peak = match below.last() {
                        // Each block the max of the two below it.
                        Some(prev) => prev[2 * b * t + k].max(prev[(2 * b + 1) * t + k]),
                        None => (b * BLOCK..(b + 1) * BLOCK)
                            .map(|h| self.data[h * t + k])
                            .max()
                            .unwrap_or(0),
                    };
                    out.push(peak);
                }
            }
            level += 1;
        }
    }

    /// Track `t`'s loudest byte over hops `a..b` (`a < b <= hops`): the largest aligned blocks of
    /// the pyramid that fit, and single hops where none does.
    fn peak(&self, t: usize, a: usize, b: usize) -> u8 {
        let n = self.tracks;
        let mut peak = 0;
        let mut h = a;
        while h < b {
            let block = (0..self.levels.len()).rev().find(|&l| {
                let size = BLOCK << l;
                h.is_multiple_of(size) && h + size <= b && (h / size) < self.levels[l].len() / n
            });
            match block {
                Some(l) => {
                    let size = BLOCK << l;
                    peak = peak.max(self.levels[l][(h / size) * n + t]);
                    h += size;
                }
                None => {
                    peak = peak.max(self.data[h * n + t]);
                    h += 1;
                }
            }
        }
        peak
    }

    pub fn tracks(&self) -> usize {
        self.tracks
    }

    /// The peaks, hop-major: `data()[hop * tracks + track]`.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn hops(&self) -> usize {
        self.data.len().checked_div(self.tracks).unwrap_or(0)
    }

    /// How far the peaks reach, in seconds.
    pub fn seconds(&self) -> f64 {
        self.hops() as f64 * HOP
    }

    /// Each track's loudest byte in each of `columns` equal stretches of `[from, to)` seconds, track
    /// by track (`out[track * columns + column]`): what a lane draws, one column per pixel. A
    /// column past the peaks read so far is 0. A column narrower than a hop takes the hop it falls
    /// in, so a zoomed-in lane shows steps rather than gaps.
    pub fn columns(&self, from: f64, to: f64, columns: usize) -> Vec<u8> {
        let mut out = vec![0u8; self.tracks * columns];
        let hops = self.hops();
        if columns == 0 || hops == 0 || !from.is_finite() || !to.is_finite() || to <= from {
            return out;
        }
        let step = (to - from) / columns as f64;
        for c in 0..columns {
            // A hair inside the stretch: 1.0 / HOP is 49.999..., which must not take hop 49.
            let a = ((from + c as f64 * step) / HOP + 1e-6).floor();
            let b = ((from + (c + 1) as f64 * step) / HOP - 1e-6).ceil();
            if b <= 0.0 {
                continue;
            }
            let a = a.max(0.0) as usize;
            let b = (b as usize).min(hops).max(a + 1);
            if a >= hops {
                break;
            }
            for t in 0..self.tracks {
                out[t * columns + c] = self.peak(t, a, b);
            }
        }
        out
    }
}

/// A peak (linear, 0..1 full scale) as a byte on the [`FLOOR_DB`]..0 dBFS scale.
pub fn level(peak: f32) -> u8 {
    if peak.is_nan() || peak <= 0.0 {
        return 0;
    }
    let db = 20.0 * peak.min(1.0).log10();
    (((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The peaks left in project folder `dir` for a file with `tracks` audio tracks; `None` when there
/// are none, they are for another track count, or the file is damaged.
pub fn load(dir: &Path, tracks: usize) -> Option<Waves> {
    let mut f = std::fs::File::open(dir.join(FILE)).ok()?;
    let mut head = [0u8; 16];
    f.read_exact(&mut head).ok()?;
    let count = u32::from_le_bytes(head[8..12].try_into().ok()?) as usize;
    let rate = u32::from_le_bytes(head[12..16].try_into().ok()?);
    if &head[..8] != MAGIC || count != tracks || rate != RATE || count == 0 {
        return None;
    }
    let mut data = Vec::new();
    f.read_to_end(&mut data).ok()?;
    if data.len() % count != 0 {
        return None;
    }
    let mut w = Waves::new(count);
    w.data = data;
    w.build();
    Some(w)
}

/// Writes `w` as [`FILE`] in `dir`, atomically (a reader never sees half of it): to a file of its
/// own beside it, then renamed over it. Streamed, not gathered into one buffer first: 30 h of peaks
/// is tens of megabytes the caller already holds once.
pub fn save(dir: &Path, w: &Waves) -> Result<()> {
    let path = dir.join(FILE);
    let tmp = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        f.write_all(MAGIC)?;
        f.write_all(&(w.tracks as u32).to_le_bytes())?;
        f.write_all(&RATE.to_le_bytes())?;
        f.write_all(&w.data)?;
        f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        output::rename_over(&tmp, &path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(written?)
}

/// Reads the peaks of tracks `0..tracks` of `file` to `duration` seconds (a short track is padded
/// with silence; unknown, all stop with the shortest), handing each new stretch of whole hops to
/// `more`. Returns the hops read, or `Ok(None)` when `stop` was set before the end.
pub fn read(
    file: &Path,
    tracks: usize,
    duration: Option<f64>,
    stop: &AtomicBool,
    more: &mut dyn FnMut(&[u8]),
) -> Result<Option<usize>> {
    if tracks == 0 {
        return Ok(Some(0));
    }
    let all: Vec<usize> = (0..tracks).collect();
    let mut r = PcmReader::merged(file, &all, DECODE_RATE, duration)?;
    let frame = PER_HOP * tracks;
    let mut buf = Vec::with_capacity(READ_HOPS * frame);
    let mut out = Vec::with_capacity(READ_HOPS * tracks);
    let mut hops = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        r.read_into(&mut buf, READ_HOPS * frame)?;
        if buf.is_empty() {
            break;
        }
        out.clear();
        // A read is whole hops of whole frames, but at the end of the stream.
        for hop in buf.chunks(frame) {
            let mut peaks = vec![0f32; tracks];
            for s in hop.chunks_exact(tracks) {
                for (p, x) in peaks.iter_mut().zip(s) {
                    *p = p.max(x.abs());
                }
            }
            out.extend(peaks.into_iter().map(level));
            hops += 1;
        }
        more(&out);
    }
    r.finish()?;
    if hops == 0 {
        return Err(Error::Tool {
            program: "ffmpeg",
            stderr: format!("no audio decoded from {}", file.display()),
        });
    }
    Ok(Some(hops))
}

#[cfg(test)]
mod tests;
