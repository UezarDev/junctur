//! Smart rendering: the edit list becomes an MP4 in which only the frames around each cut are
//! re-encoded (NVENC, matched to the source) and everything between keyframes is copied; when the
//! source cannot be matched, every frame is encoded instead and the reason given. Scratch files go
//! beside the output (a long render may not fit a small temp drive) and the output is renamed into
//! place only once complete, so a failed `--force` keeps the previous render.

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
mod tests;
