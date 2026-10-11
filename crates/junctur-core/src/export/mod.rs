//! Turning an edit list into something another program opens. FCPXML (for Resolve) first; the
//! rendered video is the smart-render phase.

pub mod fcpxml;

use std::path::Path;

use crate::edit::EditList;
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
