//! Every audio track's loudness over time, for the app's track lanes (2g): a byte per track per
//! [`HOP`] seconds, the hop's peak on a -60..0 dBFS scale.
//!
//! A module of its own, not part of the pipeline: the lanes want every track (the game, a mix next
//! to the mics), while the `speech` stage's envelope ([`crate::pipeline::envelope`]) is the loudest
//! speech track per 10 ms, made for snapping cuts; and nothing in the CLI needs waveforms, so the
//! stages `junctur analyze` runs and `jobs.json` lists stay as they were. One ffmpeg decodes every
//! track at once ([`PcmReader::merged`]) at 8 kHz, plenty for a peak per 20 ms.
//!
//! Kept in the project folder as [`FILE`] (a header and the bytes, hop by hop: every track's byte for
//! hop 0, then hop 1, ...), written whole once the read finishes; 30 h of 4 tracks is 21.6 MB.

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

/// Every track's peaks so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Waves {
    pub tracks: usize,
    /// Hop-major: `data[hop * tracks + track]`.
    pub data: Vec<u8>,
}

impl Waves {
    pub fn new(tracks: usize) -> Self {
        Self {
            tracks,
            data: Vec::new(),
        }
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
                let peak = (a..b)
                    .map(|h| self.data[h * self.tracks + t])
                    .max()
                    .unwrap_or(0);
                out[t * columns + c] = peak;
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
    (data.len() % count == 0).then_some(Waves {
        tracks: count,
        data,
    })
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

/// Reads the peaks of audio tracks `0..tracks` of `file` from start to `duration` seconds when
/// known (the recording's length: a track that ends early is padded with silence, so it never cuts
/// the others short; unknown, every track stops with the shortest), handing each new stretch (whole
/// hops, hop-major) to `more` as it is decoded. Keeps nothing itself: the caller holds the one copy. Returns the hops read, or `Ok(None)` when `stop`
/// was set before the end (the decoder is killed). A final partial hop counts as a hop.
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
mod tests {
    use super::*;

    #[test]
    fn levels_map_decibels_onto_a_byte() {
        assert_eq!(level(0.0), 0);
        assert_eq!(level(-0.5), 0, "a peak is a magnitude");
        assert_eq!(level(1.0), 255);
        assert_eq!(level(2.0), 255);
        assert_eq!(level(0.001), 0, "-60 dBFS is the floor");
        assert_eq!(level(0.0001), 0);
        // -30 dBFS, half way.
        assert!((level(10f32.powf(-1.5)) as i32 - 128).abs() <= 1);
        assert_eq!(level(f32::NAN), 0);
    }

    fn two_tracks() -> Waves {
        // Track 0 rises 0..99, track 1 is 7 throughout; 100 hops = 2 s.
        let mut w = Waves::new(2);
        for h in 0..100u8 {
            w.data.extend([h, 7]);
        }
        w
    }

    #[test]
    fn columns_take_each_tracks_loudest_hop() {
        let w = two_tracks();
        assert_eq!(w.hops(), 100);
        assert!((w.seconds() - 2.0).abs() < 1e-9);
        let c = w.columns(0.0, 2.0, 4);
        assert_eq!(c, vec![24, 49, 74, 99, 7, 7, 7, 7]);
        // Past what was read: zeros, and nothing before 0.
        let c = w.columns(1.5, 3.5, 4);
        assert_eq!(&c[..4], &[99, 0, 0, 0]);
        let c = w.columns(-1.0, 1.0, 2);
        assert_eq!(&c[..2], &[0, 49]);
    }

    #[test]
    fn columns_narrower_than_a_hop_repeat_it() {
        let w = two_tracks();
        // 0.1 s over 10 columns: 5 hops, two columns each.
        let c = w.columns(0.0, 0.1, 10);
        assert_eq!(&c[..10], &[0, 0, 1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn columns_of_nothing_are_empty_or_zero() {
        assert!(Waves::new(2).columns(0.0, 1.0, 0).is_empty());
        assert_eq!(Waves::new(2).columns(0.0, 1.0, 3), vec![0; 6]);
        assert_eq!(two_tracks().columns(1.0, 1.0, 2), vec![0; 4]);
        assert_eq!(Waves::default().hops(), 0);
    }

    #[test]
    fn saved_peaks_load_only_for_the_same_track_count() {
        let dir = std::env::temp_dir().join(format!("junctur-waves-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load(&dir, 2).is_none());
        let w = two_tracks();
        save(&dir, &w).unwrap();
        assert_eq!(load(&dir, 2), Some(w));
        assert!(load(&dir, 3).is_none());
        std::fs::write(dir.join(FILE), b"JWAVES1\n\x02\0\0\0\x32\0\0\0\x01").unwrap();
        assert!(load(&dir, 2).is_none(), "half a hop");
        std::fs::write(dir.join(FILE), b"junk").unwrap();
        assert!(load(&dir, 2).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_decode_reads_every_tracks_peaks() {
        let dir = std::env::temp_dir().join(format!("junctur-waves-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("two.mka");
        // A full-scale tone on track 1 for 3 s, a -20 dB one on track 2 that ends after 1 s (a
        // shorter track: it must not cut track 1's peaks short).
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=3"])
            .args(["-f", "lavfi", "-i", "sine=f=300:d=1"])
            .args(["-filter_complex", "[0:a]volume=8[a];[1:a]volume=0.8[b]"])
            .args(["-map", "[a]", "-map", "[b]", "-c:a", "flac"])
            .arg(&f)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut w = Waves::new(2);
        let hops = read(&f, 2, Some(3.0), &AtomicBool::new(false), &mut |b| {
            w.data.extend_from_slice(b)
        })
        .unwrap()
        .unwrap();
        assert_eq!(hops, w.hops());
        assert_eq!(
            w.hops(),
            150,
            "3 s at 50 a second, past the short track's end"
        );
        let c = w.columns(0.0, 3.0, 3);
        // lavfi's sine is at 1/8 full scale; track 1 is turned up to full, track 2 to 1/10.
        assert!(c[0] > 245 && c[2] > 245, "{c:?}");
        assert!((c[3] as i32 - 170).abs() < 8, "-20 dB: {c:?}");
        // Silent after its first second, but for the resampler ringing out (about -52 dB).
        assert!(
            c[4] < 60 && c[5] == 0,
            "silence after its first second: {c:?}"
        );
        // One track alone is padded the same way.
        let mut one = 0;
        read(&f, 1, Some(3.0), &AtomicBool::new(false), &mut |b| {
            one += b.len()
        })
        .unwrap();
        assert_eq!(one, 150);
        let stopped = read(&f, 2, Some(3.0), &AtomicBool::new(true), &mut |_| {}).unwrap();
        assert!(stopped.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
