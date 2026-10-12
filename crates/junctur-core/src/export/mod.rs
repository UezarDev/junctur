//! Turning an edit list into something another program opens: an FCPXML timeline and an SRT of
//! captions (5a), both for Resolve; the rendered video is the smart-render phase.

pub mod fcpxml;
pub mod srt;

use std::path::Path;

use crate::edit::EditList;
use crate::transcribe::Transcript;
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

/// Write `edit`'s captions as an SRT to `out` (5a): `transcript`'s kept words on the edited
/// timeline, each cue in its speaker's colour, cut by the list's caption settings. The frame rate
/// is the source's, as for the FCPXML, so cues land on the frames the render and the timeline
/// have. Returns the number of cues. Any `out` but the source itself is replaced.
pub fn write_srt(edit: &EditList, transcript: &Transcript, out: &Path) -> Result<usize> {
    output::refuse_source(&edit.source, out)?;
    let (num, den) = ffmpeg::probe_video(&edit.source)?.map_or((30, 1), |v| (v.fps_num, v.fps_den));
    let words = transcript
        .segments
        .iter()
        .flat_map(|s| s.words.iter().map(move |w| (s.track, w)));
    let cues = srt::cues(edit, words, num, den);
    std::fs::write(out, srt::write(&cues, edit))?;
    Ok(cues.len())
}
