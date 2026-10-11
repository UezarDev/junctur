//! Smart rendering: the edit list becomes an MP4 in which only the frames around each cut are
//! re-encoded (NVENC, matched to the source) and everything between keyframes is copied.
//!
//! [`source`] reads the encoding and the real keyframes, [`plan`] splits the kept frames into copy
//! and encode pieces, [`video`] writes each piece, [`audio`] rebuilds every audio track with
//! crossfades, and the pieces are joined by one muxing ffmpeg. When the source cannot be matched
//! the same machinery encodes every frame instead (a full re-encode) and says why.
//!
//! Scratch files go in a folder beside the output (same disk; a 30 h render does not fit in a
//! small temp drive) and are deleted when the render ends, failed or not (a killed process leaves
//! them). The output is muxed in there too and renamed into place only once it is complete, so a
//! failed `--force` keeps the previous render. The source is only read.

mod audio;
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
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            full: false,
            overwrite: false,
            jobs: 6,
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
    if !ffmpeg::has_encoder("h264_nvenc")? {
        return Err(Error::Render(
            "this ffmpeg has no h264_nvenc encoder".into(),
        ));
    }
    log("reading keyframes");
    let src = source::inspect(&edit.source)?;
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
    let pieces = plan::pieces(&ranges, keyframes);
    let total: u64 = pieces.iter().map(Piece::frames).sum();
    let copied: u64 = pieces.iter().filter(|p| p.copy).map(Piece::frames).sum();
    log(&format!(
        "{} ranges -> {} pieces; copying {:.0}% of {} frames",
        ranges.len(),
        pieces.len(),
        copied as f64 / total as f64 * 100.0,
        total
    ));

    let work = scratch_dir(out);
    std::fs::create_dir_all(&work)?;
    let partial = work.join(out.file_name().unwrap_or_default());
    let built = build(
        edit,
        &src,
        &ranges,
        &pieces,
        full_reason.is_none(),
        &work,
        &partial,
        opts,
        log,
    );
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

#[allow(clippy::too_many_arguments)]
fn build(
    edit: &EditList,
    src: &Source,
    ranges: &[(u64, u64)],
    pieces: &[Piece],
    matched: bool,
    work: &Path,
    partial: &Path,
    opts: &RenderOptions,
    log: &mut dyn FnMut(&str),
) -> Result<()> {
    let piece_path = |i: usize| work.join(format!("{i:06}.ts"));
    let track_path = |i: usize| work.join(format!("audio{i}.m4a"));
    let (tx, rx) = mpsc::channel();
    let next = AtomicUsize::new(0);
    // Set by the first thread that fails; every other thread stops at its next step.
    let failed = AtomicBool::new(false);
    let v = &src.video;
    // Only the first failure is reported: the rest are threads stopping because of it.
    let first = |r: Result<()>| match r {
        Err(e) if !failed.swap(true, Ordering::Relaxed) => Err(e),
        _ => Ok(()),
    };

    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for (i, track) in src.audio.iter().enumerate() {
            let (tx, failed, first) = (tx.clone(), &failed, &first);
            let segs = sample_ranges(ranges, track.rate, v.fps_num, v.fps_den);
            let fade = (edit.settings.crossfade.max(0.0) / 2.0 * track.rate as f64).round() as u64;
            let path = track_path(i);
            handles.push(scope.spawn(move || {
                let _stop = StopOnPanic(failed);
                let r = audio::render_track(&edit.source, i, track, &segs, fade, &path, failed);
                if r.is_ok() {
                    let _ = tx.send(Progress::Track(i));
                }
                first(r)
            }));
        }
        for _ in 0..opts.jobs.max(1) {
            let (tx, next, failed, first) = (tx.clone(), &next, &failed, &first);
            handles.push(scope.spawn(move || {
                let _stop = StopOnPanic(failed);
                while !failed.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(p) = pieces.get(i) else { break };
                    let args = video::args(&edit.source, p, v, matched, &piece_path(i));
                    first(ffmpeg::ffmpeg(&args))?;
                    let _ = tx.send(Progress::Piece);
                }
                Ok(())
            }));
        }
        drop(tx);
        let (mut done, step) = (0, pieces.len().div_ceil(10).max(1));
        for msg in rx {
            match msg {
                Progress::Piece => {
                    done += 1;
                    if done % step == 0 || done == pieces.len() {
                        log(&format!("video pieces {done}/{}", pieces.len()));
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
    mux(edit, src, pieces.len(), &piece_path, &track_path, partial)
}

/// Join the pieces (fed in order through stdin) with the audio tracks into the output MP4, by copy.
fn mux(
    edit: &EditList,
    src: &Source,
    pieces: usize,
    piece_path: &dyn Fn(usize) -> PathBuf,
    track_path: &dyn Fn(usize) -> PathBuf,
    out: &Path,
) -> Result<()> {
    let n = src.audio.len();
    let mut a: Vec<String> = ["-v", "error", "-y", "-f", "mpegts", "-i", "-"]
        .map(String::from)
        .to_vec();
    for i in 0..n {
        a.push("-i".into());
        a.push(track_path(i).to_string_lossy().into_owned());
    }
    // The source again, unread, only for its metadata (track titles, languages).
    a.push("-i".into());
    a.push(edit.source.to_string_lossy().into_owned());
    a.extend(["-map".into(), "0:v:0".into()]);
    for i in 0..n {
        a.extend(["-map".into(), format!("{}:a:0", i + 1)]);
        a.extend([
            format!("-map_metadata:s:a:{i}"),
            format!("{}:s:a:{i}", n + 1),
        ]);
    }
    a.extend([
        "-map_metadata".into(),
        (n + 1).to_string(),
        "-c".into(),
        "copy".into(),
    ]);
    a.push(out.to_string_lossy().into_owned());
    let mut feed = ffmpeg::Feed::spawn(&a)?;
    for i in 0..pieces {
        let path = piece_path(i);
        // Streamed: a copy piece can be a whole kept range, gigabytes long.
        feed.copy_from(&mut std::fs::File::open(&path)?)?;
        // Free the scratch space as the mux goes.
        let _ = std::fs::remove_file(path);
    }
    feed.finish()
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
}
