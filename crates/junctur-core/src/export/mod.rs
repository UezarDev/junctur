//! Turning an edit list into something another program opens: an FCPXML timeline and SRTs of
//! captions (5a; one per speaker since 5b), both for Resolve; the rendered video is the smart-render phase.

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
    /// Cues in the combined file (the path asked for).
    pub cues: usize,
    /// Each speaker's own file (5b): the path, the speaker's name and its cue count.
    pub speakers: Vec<(PathBuf, String, usize)>,
}

/// Write `edit`'s captions as SRT (5a, 5b): the combined file at `out`, everyone one cue at a time,
/// and beside it one file per speaker with any words, `<stem> (<name>).srt`, holding every word
/// that speaker says, so overlapping talk shows on separate Resolve subtitle tracks. The kept words
/// go on the edited timeline, each cue in its speaker's colour, cut by the list's caption settings.
/// The frame rate is the source's, as for the FCPXML, so cues land on the frames the render and the
/// timeline have. Any of these paths but the source itself is replaced.
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
    let all = srt::cues(edit, words.iter().copied(), num, den);
    let mut files = Vec::new();
    let mut taken = Vec::new();
    for s in &edit.speakers.names {
        let cues = srt::cues_for(
            edit,
            words.iter().copied(),
            num,
            den,
            srt::Pick::Speaker(s.id),
        );
        if cues.is_empty() {
            continue;
        }
        let path = srt::speaker_path(out, &s.name, s.id, &mut taken);
        output::refuse_source(&edit.source, &path)?;
        files.push((path, s.name.clone(), cues));
    }
    std::fs::write(out, srt::write(&all, edit))?;
    let mut speakers = Vec::new();
    for (path, name, cues) in files {
        std::fs::write(&path, srt::write(&cues, edit))?;
        speakers.push((path, name, cues.len()));
    }
    Ok(CaptionFiles {
        cues: all.len(),
        speakers,
    })
}
