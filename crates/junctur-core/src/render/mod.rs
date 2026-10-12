//! Smart rendering: the edit list becomes an MP4 in which only the frames around each cut are
//! re-encoded (NVENC, matched to the source) and everything between keyframes is copied.
//!
//! [`source`] reads the encoding and the real keyframes, [`plan`] splits the kept frames into copy
//! and encode pieces, [`video`] writes each piece, [`audio`] rebuilds every audio track with
//! crossfades, and [`mux`] joins them in one ffmpeg. When the source cannot be matched
//! the same machinery encodes every frame instead (a full re-encode) and says why.
//!
//! Scratch files go in a folder beside the output (same disk; a 30 h render does not fit in a
//! small temp drive) and are deleted when the render ends, failed or not (a killed process leaves
//! them). The output is muxed in there too and renamed into place only once it is complete, so a
//! failed `--force` keeps the previous render. The source is only read.

mod audio;
mod mux;
mod plan;
mod source;
mod splice;
mod video;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use crate::edit::{frame_ranges, EditList};
use crate::{ffmpeg, output, Error, Result};

pub use plan::{pieces, Piece};
pub use source::{inspect, Source};

#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Encode every frame even when the source could be matched.
    pub full: bool,
    /// Replace an existing output file (never the source).
    pub overwrite: bool,
    /// Pieces written at once. Each piece is mostly ffmpeg start-up and opening the source, so
    /// several run side by side; consumer NVENC allows 8 sessions.
    pub jobs: usize,
    /// Where the source's keyframe index is kept between renders (one ffprobe pass over the
    /// packets, ~2 min for 30 h). None reads it every time.
    pub cache_dir: Option<PathBuf>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            full: false,
            overwrite: false,
            jobs: 6,
            cache_dir: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderReport {
    /// Why every frame was encoded, if it was.
    pub full_reason: Option<String>,
    pub frames_copied: u64,
    pub frames_encoded: u64,
    pub pieces: usize,
    pub audio_tracks: usize,
    /// Output length in seconds (whole frames).
    pub duration: f64,
    pub seconds: f64,
}

pub fn render(
    edit: &EditList,
    out: &Path,
    opts: &RenderOptions,
    log: &mut dyn FnMut(&str),
) -> Result<RenderReport> {
    let started = Instant::now();
    check_output(&edit.source, out, opts.overwrite)?;
    if !ffmpeg::has_encoder_cached("h264_nvenc")? {
        return Err(Error::Render(
            "this ffmpeg has no h264_nvenc encoder".into(),
        ));
    }
    let src = match &opts.cache_dir {
        Some(dir) => source::inspect_cached(&edit.source, dir, log)?,
        None => {
            log("reading keyframes");
            source::inspect(&edit.source)?
        }
    };
    let v = &src.video;
    let ranges = frame_ranges(&edit.keep, v.fps_num, v.fps_den, src.frames);
    if ranges.is_empty() {
        return Err(Error::Render("the edit list keeps nothing".into()));
    }
    let full_reason = if opts.full {
        Some("a full re-encode was asked for".into())
    } else {
        src.mismatch()
    };
    if let Some(why) = &full_reason {
        log(&format!("full re-encode: {why}"));
    }
    let keyframes: &[u64] = if full_reason.is_some() {
        &[]
    } else {
        &src.keyframes
    };
    let pieces = plan::pieces(&ranges, keyframes, plan::gop(keyframes));
    let per_second = |s: f64| (s * v.fps_num as f64 / v.fps_den as f64) as u64;
    let batches = plan::batches(&pieces, per_second(BATCH_GAP), per_second(BATCH_SPAN));
    let total: u64 = pieces.iter().map(Piece::frames).sum();
    let copied: u64 = pieces.iter().filter(|p| p.copy).map(Piece::frames).sum();
    log(&format!(
        "{} ranges -> {} pieces in {} ffmpeg runs; copying {:.0}% of {} frames",
        ranges.len(),
        pieces.len(),
        batches.len(),
        copied as f64 / total as f64 * 100.0,
        total
    ));

    let work = scratch_dir(out);
    std::fs::create_dir_all(&work)?;
    let partial = work.join(out.file_name().unwrap_or_default());
    let job = Job {
        edit,
        src: &src,
        ranges: &ranges,
        pieces: &pieces,
        batches: &batches,
        matched: full_reason.is_none(),
        work: &work,
    };
    let built = job.build(&partial, opts.jobs, log);
    let finished = built.is_ok();
    let result = built.and_then(|()| install(&partial, out));
    // A finished render that could be moved nowhere stays in the folder, which the error names.
    // Otherwise the folder goes, a failed render's partial output with it; `out` was never touched.
    if !(finished && partial.exists()) {
        if let Err(e) = std::fs::remove_dir_all(&work) {
            log(&format!("could not remove {}: {e}", work.display()));
        }
    }
    result?;
    Ok(RenderReport {
        full_reason,
        frames_copied: copied,
        frames_encoded: total - copied,
        pieces: pieces.len(),
        audio_tracks: src.audio.len(),
        duration: total as f64 * v.fps_den as f64 / v.fps_num as f64,
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// Refuse to write over the source (by file identity: hard links included), or over anything else
/// unless asked.
fn check_output(source: &Path, out: &Path, overwrite: bool) -> Result<()> {
    output::refuse_source(source, out)?;
    if out.exists() && !overwrite {
        return Err(Error::Render(format!(
            "{} exists (--force replaces it)",
            out.display()
        )));
    }
    Ok(())
}

/// Move the finished render onto `out`. When `out` cannot be replaced (on Windows: open in a
/// player, read-only, held by a virus scan) the render is not thrown away: it goes beside `out` as
/// `<name>.junctur-new.<ext>`, or else stays where it is, and the error says where it is.
fn install(partial: &Path, out: &Path) -> Result<()> {
    let Err(e) = std::fs::rename(partial, out) else {
        return Ok(());
    };
    let beside = beside_name(out);
    let at = if !beside.exists() && std::fs::rename(partial, &beside).is_ok() {
        beside
    } else {
        partial.to_path_buf()
    };
    Err(Error::Render(format!(
        "could not replace {} ({e}); the finished render is at {}",
        out.display(),
        at.display()
    )))
}

fn beside_name(out: &Path) -> PathBuf {
    let mut name = out.file_stem().unwrap_or_default().to_os_string();
    name.push(".junctur-new");
    if let Some(ext) = out.extension() {
        name.push(".");
        name.push(ext);
    }
    out.with_file_name(name)
}

/// Encode pieces at most this far apart in the source (seconds) share one ffmpeg: decoding through
/// the gap on NVDEC costs less than starting another process and NVENC session.
const BATCH_GAP: f64 = 5.0;
/// And one ffmpeg decodes at most this much of the source, so a batch stays a short job.
const BATCH_SPAN: f64 = 30.0;

fn scratch_dir(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".junctur-parts");
    out.with_file_name(name)
}

/// Kept frame ranges as sample-frame ranges at `rate`, rounded the same way at both ends so
/// consecutive ranges add up to the video length to within a sample.
fn sample_ranges(ranges: &[(u64, u64)], rate: u32, fps_num: u64, fps_den: u64) -> Vec<(u64, u64)> {
    let at = |f: u64| {
        ((f as u128 * fps_den as u128 * rate as u128 + fps_num as u128 / 2) / fps_num as u128)
            as u64
    };
    ranges.iter().map(|&(a, b)| (at(a), at(b))).collect()
}

/// Stops the other render threads when this one panics, as a failure would.
struct StopOnPanic<'a>(&'a AtomicBool);

impl Drop for StopOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::Relaxed);
        }
    }
}

enum Progress {
    Piece,
    Track(usize),
}

/// One render's inputs, shared by the threads that write its pieces and tracks.
struct Job<'a> {
    edit: &'a EditList,
    src: &'a Source,
    /// Kept frame ranges.
    ranges: &'a [(u64, u64)],
    pieces: &'a [Piece],
    /// Pieces written by one ffmpeg each, in output order.
    batches: &'a [std::ops::Range<usize>],
    /// Encodes match the source (smart render) rather than a full re-encode.
    matched: bool,
    /// The scratch folder.
    work: &'a Path,
}

impl Job<'_> {
    fn piece_path(&self, i: usize) -> PathBuf {
        self.work.join(format!("{i:06}.ts"))
    }

    fn track_path(&self, i: usize) -> PathBuf {
        self.work.join(format!("audio{i}.m4a"))
    }

    /// Write every batch of pieces (`jobs` at once) and audio track, then mux them into `partial`.
    fn build(&self, partial: &Path, jobs: usize, log: &mut dyn FnMut(&str)) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let next = AtomicUsize::new(0);
        // Set by the first thread that fails; every other thread stops at its next step.
        let failed = AtomicBool::new(false);
        let (edit, v, batches) = (self.edit, &self.src.video, self.batches);
        // Only the first failure is reported: the rest are threads stopping because of it.
        let first = |r: Result<()>| match r {
            Err(e) if !failed.swap(true, Ordering::Relaxed) => Err(e),
            _ => Ok(()),
        };

        std::thread::scope(|scope| -> Result<()> {
            let mut handles = Vec::new();
            for (i, track) in self.src.audio.iter().enumerate() {
                let (tx, failed, first) = (tx.clone(), &failed, &first);
                let segs = sample_ranges(self.ranges, track.rate, v.fps_num, v.fps_den);
                let fade =
                    (edit.settings.crossfade.max(0.0) / 2.0 * track.rate as f64).round() as u64;
                let path = self.track_path(i);
                handles.push(scope.spawn(move || {
                    let _stop = StopOnPanic(failed);
                    let r = audio::render_track(&edit.source, i, track, &segs, fade, &path, failed);
                    if r.is_ok() {
                        let _ = tx.send(Progress::Track(i));
                    }
                    first(r)
                }));
            }
            for _ in 0..jobs.max(1) {
                let (tx, next, failed, first) = (tx.clone(), &next, &failed, &first);
                handles.push(scope.spawn(move || {
                    let _stop = StopOnPanic(failed);
                    while !failed.load(Ordering::Relaxed) {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(b) = batches.get(i) else { break };
                        let batch = &self.pieces[b.clone()];
                        let args =
                            video::args(&edit.source, batch, v, self.matched, &self.piece_path(i));
                        first(ffmpeg::ffmpeg(&args))?;
                        let _ = tx.send(Progress::Piece);
                    }
                    Ok(())
                }));
            }
            drop(tx);
            let (mut done, step) = (0, batches.len().div_ceil(10).max(1));
            for msg in rx {
                match msg {
                    Progress::Piece => {
                        done += 1;
                        if done % step == 0 || done == batches.len() {
                            log(&format!("video pieces {done}/{}", batches.len()));
                        }
                    }
                    Progress::Track(i) => log(&format!("audio track {} done", i + 1)),
                }
            }
            // Join every thread, so a panic is an error (and cleanup runs) rather than an unwind
            // out of `render` that leaves the parts folder behind.
            let mut result = Ok(());
            for h in handles {
                let r = h
                    .join()
                    .unwrap_or_else(|_| Err(Error::Render("a render thread panicked".into())));
                if result.is_ok() {
                    result = r;
                }
            }
            result
        })?;

        log("muxing");
        self.mux(partial)
    }

    /// Join the pieces (fed in order through stdin) with the audio tracks into `out`, by copy.
    fn mux(&self, out: &Path) -> Result<()> {
        let tracks: Vec<PathBuf> = (0..self.src.audio.len())
            .map(|i| self.track_path(i))
            .collect();
        let mut feed = ffmpeg::Feed::spawn(&mux::args(&self.edit.source, &tracks, out))?;
        for i in 0..self.batches.len() {
            let path = self.piece_path(i);
            // Streamed: a copy piece can be a whole kept range, gigabytes long.
            feed.copy_from(&mut std::fs::File::open(&path)?)?;
            // Free the scratch space as the mux goes.
            let _ = std::fs::remove_file(path);
        }
        feed.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_ranges_follow_the_frames() {
        // 30 fps at 48 kHz: 1600 samples a frame.
        assert_eq!(
            sample_ranges(&[(3, 33)], 48_000, 30, 1),
            vec![(4800, 52_800)]
        );
        // 29.97 fps at 44.1 kHz: frame 1 is 1471.47 samples -> 1471.
        assert_eq!(
            sample_ranges(&[(1, 2)], 44_100, 30_000, 1001),
            vec![(1471, 2943)]
        );
    }

    #[test]
    fn output_guard() {
        let dir = std::env::temp_dir().join(format!("junctur-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        std::fs::write(&src, b"x").unwrap();
        assert!(check_output(&src, &src, true).is_err(), "never the source");
        assert!(check_output(&src, &dir.join("./in.mp4"), true).is_err());
        let link = dir.join("link.mp4");
        std::fs::hard_link(&src, &link).unwrap();
        assert!(
            check_output(&src, &link, true).is_err(),
            "a hard link is the source"
        );
        assert!(check_output(&src, &src.with_file_name("out.mp4"), false).is_ok());
        std::fs::write(dir.join("out.mp4"), b"y").unwrap();
        assert!(check_output(&src, &dir.join("out.mp4"), false).is_err());
        assert!(check_output(&src, &dir.join("out.mp4"), true).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            scratch_dir(Path::new("x/out.mp4")),
            PathBuf::from("x/out.mp4.junctur-parts")
        );
    }

    #[test]
    fn a_finished_render_that_cannot_replace_the_output_is_kept() {
        let dir = std::env::temp_dir().join(format!("junctur-install-{}", std::process::id()));
        let work = dir.join("out.mp4.junctur-parts");
        std::fs::create_dir_all(&work).unwrap();
        let partial = work.join("out.mp4");
        // A folder where the output goes: no rename replaces it, on any system.
        let out = dir.join("out.mp4");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(&partial, b"render").unwrap();
        let e = install(&partial, &out).unwrap_err().to_string();
        let beside = dir.join("out.junctur-new.mp4");
        assert!(e.contains("out.junctur-new.mp4"), "{e}");
        assert_eq!(std::fs::read(&beside).unwrap(), b"render");
        // The name beside is taken too: the render stays in the parts folder.
        std::fs::write(&partial, b"render 2").unwrap();
        let e = install(&partial, &out).unwrap_err().to_string();
        assert!(e.contains("junctur-parts"), "{e}");
        assert_eq!(std::fs::read(&partial).unwrap(), b"render 2");
        // The usual case replaces the output.
        std::fs::remove_dir_all(&out).unwrap();
        std::fs::write(&out, b"old").unwrap();
        install(&partial, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"render 2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Every frame of `file`'s video, decoded small (32x24 luma).
    fn small_frames(file: &Path) -> Vec<Vec<u8>> {
        let out = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(file)
            .args([
                "-vf",
                "scale=32:24",
                "-pix_fmt",
                "gray",
                "-fps_mode",
                "passthrough",
            ])
            .args(["-f", "rawvideo", "-"])
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
            .chunks_exact(32 * 24)
            .map(<[u8]>::to_vec)
            .collect()
    }

    fn mse(a: &[u8], b: &[u8]) -> f64 {
        let d = a
            .iter()
            .zip(b)
            .map(|(&x, &y)| (x as f64 - y as f64).powi(2));
        d.sum::<f64>() / a.len() as f64
    }

    #[test]
    #[ignore = "needs NVENC"]
    fn a_batch_lands_on_the_frames_its_pieces_did() {
        let dir = std::env::temp_dir().join(format!("junctur-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // testsrc2 moves every frame, so a frame is told from its neighbours.
        let src = dir.join("in.mp4");
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=320x240:r=30:d=10",
            ])
            .args([
                "-c:v", "libx264", "-g", "30", "-bf", "2", "-pix_fmt", "yuv420p",
            ])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let info = source::inspect(&src).unwrap();
        let kf = &info.keyframes;
        let pieces = plan::pieces(&[(10, 50), (70, 130), (140, 200)], kf, plan::gop(kf));
        let batches = plan::batches(&pieces, 150, 900);
        let mut joined = 0;
        for b in batches.iter().filter(|b| b.len() > 1) {
            let batch = &pieces[b.clone()];
            let one = dir.join("batch.ts");
            ffmpeg::ffmpeg(&video::args(&src, batch, &info.video, true, &one)).unwrap();
            let mut apart = Vec::new();
            for p in batch {
                let each = dir.join("piece.ts");
                ffmpeg::ffmpeg(&video::args(&src, &[*p], &info.video, true, &each)).unwrap();
                apart.extend(small_frames(&each));
            }
            // Compared with the pieces encoded one by one, not the source: on these clips both
            // land one frame late (1e open issue), and batching must not change where.
            let together = small_frames(&one);
            assert_eq!(together.len(), apart.len());
            for (k, f) in together.iter().enumerate() {
                let here = mse(f, &apart[k]);
                for n in [k.wrapping_sub(1), k + 1] {
                    if let Some(other) = apart.get(n) {
                        assert!(here < mse(f, other), "batch frame {k} is piece frame {n}");
                    }
                }
            }
            joined += 1;
        }
        assert!(joined > 0, "no batch in {batches:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
