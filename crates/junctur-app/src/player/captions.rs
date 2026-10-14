//! Captions of the list in play: the SRT export, the preview's cues and the Resolve send cut from
//! the same words, and the translations they read.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use junctur_core::edit::{Captions, EditList, ExistingClips};
use junctur_core::export::layout::{self, Placed};
use junctur_core::export::srt;
use junctur_core::export::{self, CaptionFiles};
use junctur_core::resolve;
use junctur_core::transcribe;
use junctur_core::translate::Translations;

use super::{Core, Stamp};
use crate::cuts::Fps;
use crate::editor::Words;
use crate::lock::lock;
use crate::translation;

impl Core {
    /// Sets the edit list's caption settings and saves them.
    pub fn set_captions(&self, file: Option<&str>, captions: Captions) -> Result<(), String> {
        self.change(file, |ed, list, _| ed.set_captions(list, captions))
    }

    /// Writes the captions of the list in play to `out` as one SRT on the edited timeline;
    /// translated when set, refused while the translation is not all made.
    pub fn export_captions(&self, out: &Path) -> Result<CaptionFiles, String> {
        let (list, captions, words, fps) = self.caption_source()?;
        let tracked: Vec<_> = words.tracked().collect();
        let tr = self.translations();
        export::write_captions(&list, &captions, &tracked, fps.num, fps.den, &tr, out)
            .map_err(|e| format!("writing {}: {e}", out.display()))
    }

    /// The captions as the SRT export cuts them, for the preview, placed as the Resolve send places
    /// them on a picture `aspect` (height over width); translations so far, the rest as said.
    pub fn caption_cues(&self, aspect: f64) -> Result<Vec<Placed>, String> {
        let (list, captions, cues) = self.cut_captions(false, |_| Ok(()))?;
        Ok(layout::place(&cues, &list, &captions, aspect))
    }

    /// Sends the list in play into DaVinci Resolve as a timeline with Text+ captions; blocks until
    /// Resolve answers. `existing` overrides, this once, the choice on clips in the captions' way.
    pub fn send_to_resolve(
        &self,
        answer: Option<resolve::Answer>,
        existing: Option<ExistingClips>,
    ) -> Result<resolve::Outcome, String> {
        let (list, mut captions, words, fps) = self.edit_source()?;
        if let Some(e) = existing {
            captions.existing_clips = e;
        }
        captions.check()?;
        let cues = match words {
            Some(words) => self.cut(&list, &captions, &words, fps, true)?,
            None => Vec::new(),
        };
        resolve::send(&cues, &list, &captions, answer)
    }

    /// The list in play, its caption settings and its cues: the one cut the preview and the send
    /// share, so the preview shows what Resolve gets. `finished` refuses an unfinished translation;
    /// `adjust` sets or checks the settings before cutting.
    fn cut_captions(
        &self,
        finished: bool,
        adjust: impl FnOnce(&mut Captions) -> Result<(), String>,
    ) -> Result<(Arc<EditList>, Captions, Vec<srt::Cue>), String> {
        let (list, mut captions, words, fps) = self.caption_source()?;
        adjust(&mut captions)?;
        let cues = self.cut(&list, &captions, &words, fps, finished)?;
        Ok((list, captions, cues))
    }

    /// [`Self::cut_captions`]' cut of `words`, apart so a send without a transcript shares it.
    fn cut(
        &self,
        list: &EditList,
        captions: &Captions,
        words: &Words,
        fps: Fps,
        finished: bool,
    ) -> Result<Vec<srt::Cue>, String> {
        let tracked: Vec<_> = words.tracked().collect();
        let tr = self.translations();
        Ok(if finished {
            export::finished_cues(list, captions, &tracked, fps.num, fps.den, &tr)
                .map_err(|e| e.to_string())?
        } else {
            srt::cues(
                list,
                captions,
                tracked.iter().copied(),
                fps.num,
                fps.den,
                Some(&*tr),
            )
        })
    }

    /// The open recording's translations, read again only when the file's size or time moved: the
    /// background run writes them as it goes, and the preview asks on every caption change.
    fn translations(&self) -> Arc<Translations> {
        let Some(dir) = lock(&self.editor).project_dir().map(PathBuf::from) else {
            return Arc::default();
        };
        let stamp: Stamp = std::fs::metadata(junctur_core::translate::path(&dir))
            .ok()
            .and_then(|m| Some((m.len(), m.modified().ok()?)));
        let mut held = lock(&self.translations);
        if let Some((d, s, t)) = &*held {
            if *d == dir && *s == stamp && stamp.is_some() {
                return t.clone();
            }
        }
        let t = Arc::new(Translations::load(&dir));
        *held = Some((dir, stamp, t.clone()));
        t
    }

    /// What translating the captions of the list in play needs: its sentences as said, the
    /// transcript's segments (the language each was said in) and where to keep the result.
    /// While the recording transcribes, the sentences so far.
    pub fn translation_job(&self) -> Result<translation::Job, String> {
        let (list, captions, words, fps) = self.caption_source()?;
        if captions.translate.is_empty() {
            return Err("the captions are not set to be translated (Translate to)".into());
        }
        let project = lock(&self.editor)
            .project_dir()
            .map(PathBuf::from)
            .ok_or("no project for this recording")?;
        let file = self.file().ok_or("no recording open")?;
        let tracked = words.tracked();
        let sentences = srt::sentences(&list, tracked, fps.num, fps.den);
        // Never none in place of the transcript: a piece read mid-write would make every
        // sentence's language unknown.
        let segments = transcribe::load(&project)
            .map(|t| t.segments)
            .or_else(|| transcribe::so_far(&project))
            .filter(|s| !s.is_empty())
            .ok_or("the transcript could not be read just now; try again")?;
        Ok(translation::Job {
            file,
            to: captions.translate,
            project,
            cache_dir: junctur_core::cache::default_dir(),
            sentences,
            segments,
        })
    }

    /// How many of the list in play's caption sentences have no translation yet, of how many;
    /// `(0, 0)` when its captions are not translated.
    pub fn untranslated(&self) -> Result<(usize, usize), String> {
        let (list, captions, words, fps) = self.caption_source()?;
        if captions.translate.is_empty() {
            return Ok((0, 0));
        }
        let tracked = words.tracked();
        let sentences = srt::sentences(&list, tracked, fps.num, fps.den);
        let missing = srt::missing(&captions, &sentences, &self.translations());
        Ok((missing, sentences.len()))
    }

    /// What a caption file is cut from: the list in play, its caption settings (its own, else the
    /// defaults it shows, so export and preview agree with the panel), the words the editor
    /// holds (an `Arc`, not a copy; each lock is held only to take its part) and the frame rate.
    fn caption_source(&self) -> Result<(Arc<EditList>, Captions, Arc<Words>, Fps), String> {
        let (list, captions, words, fps) = self.edit_source()?;
        let words =
            words.ok_or("captions need a transcript: run the pipeline on this recording")?;
        Ok((list, captions, words, fps))
    }

    /// New caption defaults from Settings: a list without its own settings shows them at once.
    pub fn set_caption_defaults(&self, captions: Captions) {
        *lock(&self.caption_defaults) = captions;
        self.emit_edit(&lock(&self.ctl));
    }
}
