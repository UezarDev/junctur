//! Turning an edit list into something another program opens: an FCPXML timeline and SRTs of
//! captions (5a; overlapping speakers stacked in one caption since 5e), both for Resolve; the
//! rendered video is the smart-render phase.

pub mod fcpxml;
pub mod srt;

use std::path::{Path, PathBuf};

use crate::edit::EditList;
use crate::transcribe::{Transcript, Word};
use crate::{ffmpeg, output, Result};

/// Write `edit` as an FCPXML timeline to `out`, probing the source for its frame rate. Any `out`
/// but the source itself is replaced.
pub fn write_fcpxml(edit: &EditList, out: &Path) -> Result<()> {
    output::refuse_source(&edit.source, out)?;
    let probe = ffmpeg::probe(&edit.source)?;
    let media = fcpxml::Media {
        video: ffmpeg::probe_video(&edit.source)?,
        audio_tracks: probe.audio.len(),
        audio_rate: probe.audio.first().map_or(48_000, |a| a.sample_rate),
    };
    let name = edit
        .source
        .file_stem()
        .map_or_else(|| "source".into(), |s| s.to_string_lossy().into_owned());
    std::fs::write(out, fcpxml::write(edit, &media, &name))?;
    Ok(())
}

/// What [`write_srt`] wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionFiles {
    /// Cues in the file.
    pub cues: usize,
    /// `<stem> (…).srt` files beside it: speaker files 5b wrote before captions became one file
    /// (5e). Not deleted (they could be the user's own); said, so they are not imported by mistake.
    pub stale: Vec<PathBuf>,
}

/// Write `edit`'s captions as one SRT at `out` (5a, 5e): the kept words on the edited timeline,
/// each speaker's lines in their colour, people talking at once stacked in one cue, cut by the
/// list's caption settings. The frame rate is the source's, as for the FCPXML, so cues land on the
/// frames the render and the timeline have. Any `out` but the source itself is replaced.
pub fn write_srt(edit: &EditList, transcript: &Transcript, out: &Path) -> Result<CaptionFiles> {
    output::refuse_source(&edit.source, out)?;
    let (num, den) = ffmpeg::probe_video(&edit.source)?.map_or((30, 1), |v| (v.fps_num, v.fps_den));
    let words: Vec<(usize, &Word)> = transcript
        .segments
        .iter()
        .flat_map(|s| s.words.iter().map(move |w| (s.track, w)))
        .collect();
    write_captions(edit, &words, num, den, out)
}

/// [`write_srt`] from words already in hand (the app holds them) at a known frame rate.
pub fn write_captions(
    edit: &EditList,
    words: &[(usize, &Word)],
    num: u64,
    den: u64,
    out: &Path,
) -> Result<CaptionFiles> {
    output::refuse_source(&edit.source, out)?;
    let cues = srt::cues(edit, words.iter().copied(), num, den);
    std::fs::write(out, srt::write(&cues, edit))?;
    Ok(CaptionFiles {
        cues: cues.len(),
        stale: stale_files(out),
    })
}

/// `<stem> (…).srt` files beside `out`.
fn stale_files(out: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (out.parent(), out.file_stem()) else {
        return Vec::new();
    };
    let prefix = format!("{} (", stem.to_string_lossy()).to_lowercase();
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            n.starts_with(&prefix) && n.ends_with(").srt")
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_files_left_beside_it_are_named() {
        let d = std::env::temp_dir().join(format!("junctur-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in [
            "rec.srt",
            "rec (Ana).srt",
            "rec (Old).srt",
            "rec (x).txt",
            "other (Ana).srt",
        ] {
            std::fs::write(d.join(f), "").unwrap();
        }
        assert_eq!(
            stale_files(&d.join("rec.srt")),
            vec![d.join("rec (Ana).srt"), d.join("rec (Old).srt")]
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
