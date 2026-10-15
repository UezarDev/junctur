//! Word fixes: one word or a run retyped, a word confirmed, fixes spread or taken back, each
//! refused when the words it was asked on changed meanwhile.

use std::sync::Arc;

use junctur_core::edit::{Fix, Spread};
use junctur_core::wordlist;

use super::Core;
use crate::editor::{Editor, Words};
use crate::lock::lock;

/// What a word fix did besides itself: the entries the word list is to learn (the caller
/// saves them, `settings::learn`), and the fixes it also made on the words heard like it.
#[derive(Debug, Default)]
pub struct FixDone {
    pub learn: Vec<String>,
    pub others: Vec<Fix>,
    /// What each of `others`' words were heard as (what "Undo those" leaves them saying).
    pub heard: Vec<String>,
}

/// The words `words` heard, as a fix's new words are compared with them (the word list learns
/// only words the recording never heard).
fn heard(words: &Words) -> wordlist::Heard {
    wordlist::Heard::new(words.list.iter().map(|w| w.text.as_str()))
}

/// A word edit refused because the words it names changed between reading them and saving (a
/// finished re-transcription would get the fix on the old word's place).
pub const WORDS_CHANGED: &str =
    "the transcript changed meanwhile: nothing was changed, try again on the words shown now";

/// The editor's words were let go while a word edit was asked for (a piece of a running
/// transcription landed): the edit reads them again and retries ([`Core::retrying`]).
const WORDS_LET_GO: &str = "the editor's words were read again meanwhile";

/// Whether `now` has `then`'s words `first..=last` as they were: the same track, heard times, text
/// and place in their millisecond. A piece of a running transcription only adds words
/// after the ones already there, so it never changes them; a finished re-transcription does.
pub(super) fn same_target(now: &Words, then: &Words, first: usize, last: usize) -> bool {
    (first..=last).all(|i| match (now.list.get(i), then.list.get(i)) {
        (Some(a), Some(b)) => {
            a.start == b.start
                && a.end == b.end
                && a.text == b.text
                && now.tracks.get(i) == then.tracks.get(i)
                && now.nth(i) == then.nth(i)
        }
        _ => false,
    })
}

/// `ed`'s words still have words `first..=last` of `words`, the ones a word edit read
/// ([`same_target`]). Not compared by pointer: a piece of a running transcription landing in
/// between would refuse every fix made while it runs.
fn same_words(ed: &Editor, words: &Arc<Words>, first: usize, last: usize) -> Result<(), String> {
    match &ed.words {
        Some(w) if Arc::ptr_eq(w, words) || same_target(w, words, first, last) => Ok(()),
        Some(_) => Err(WORDS_CHANGED.into()),
        None => Err(WORDS_LET_GO.into()),
    }
}

impl Core {
    /// Fixes the spelling of transcript word `index` in the edit list and saves it, the words
    /// heard like it included; returns what the word list is to learn and the fixes made on
    /// those other words. Not an undo step, like a rename.
    pub fn fix_word(
        &self,
        file: Option<&str>,
        index: usize,
        text: &str,
    ) -> Result<FixDone, String> {
        self.retrying(|| self.fix_word_once(file, index, text))
    }

    /// A word edit, read again when the editor let its words go meanwhile ([`WORDS_LET_GO`]):
    /// twice more, then refused as [`WORDS_CHANGED`].
    fn retrying<T>(&self, mut once: impl FnMut() -> Result<T, String>) -> Result<T, String> {
        for _ in 0..2 {
            match once() {
                Err(e) if e == WORDS_LET_GO => continue,
                r => return r,
            }
        }
        once().map_err(|e| {
            if e == WORDS_LET_GO {
                WORDS_CHANGED.into()
            } else {
                e
            }
        })
    }

    fn fix_word_once(
        &self,
        file: Option<&str>,
        index: usize,
        text: &str,
    ) -> Result<FixDone, String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        let (Some(word), Some(&track)) = (words.list.get(index), words.tracks.get(index)) else {
            return Err("no such word in the transcript".into());
        };
        let nth = words.nth(index);
        let fixed = junctur_core::edit::check_fix(text)?;
        let mut done = FixDone::default();
        self.change(file, |ed, list, _| {
            same_words(ed, &words, index, index)?;
            let r = ed.fix_word(list, track, word, nth, &fixed)?;
            if let Some(e) = &r {
                (done.others, done.heard) = (e.others.clone(), e.heard.clone());
            }
            Ok(r)
        })?;
        done.learn = wordlist::learned_in(&word.text, &fixed, &heard(&words))
            .into_iter()
            .collect();
        Ok(done)
    }

    /// Marks transcript word `index` as checked in the edit list and saves it, so it is no longer
    /// flagged. Not an undo step, like a fix.
    pub fn confirm_word(&self, file: Option<&str>, index: usize) -> Result<(), String> {
        self.retrying(|| self.confirm_word_once(file, index))
    }

    fn confirm_word_once(&self, file: Option<&str>, index: usize) -> Result<(), String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        let (Some(word), Some(&track)) = (words.list.get(index), words.tracks.get(index)) else {
            return Err("no such word in the transcript".into());
        };
        let nth = words.nth(index);
        self.change(file, |ed, list, _| {
            same_words(ed, &words, index, index)?;
            ed.confirm_word(list, track, word, nth)
        })
    }

    /// Fixes transcript words `first..=last` as one text, the same words heard elsewhere included;
    /// returns what the word list is to learn and the fixes made elsewhere. Refused across two
    /// tracks or two people's words ([`Words::retyped`]). Not an undo step.
    pub fn fix_words(
        &self,
        file: Option<&str>,
        first: usize,
        last: usize,
        text: &str,
    ) -> Result<FixDone, String> {
        self.retrying(|| self.fix_words_once(file, first, last, text))
    }

    fn fix_words_once(
        &self,
        file: Option<&str>,
        first: usize,
        last: usize,
        text: &str,
    ) -> Result<FixDone, String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        let speakers = lock(&self.ctl)
            .edit
            .list
            .as_deref()
            .map(|l| l.speakers.clone())
            .unwrap_or_default();
        let fixed = junctur_core::edit::check_span(text)?;
        let times = lock(&self.ctl)
            .edit
            .list
            .as_ref()
            .map_or_else(Vec::new, |l| l.times.clone());
        let run = words.retyped(first, last, &fixed, &speakers, &times)?;
        let mut done = FixDone::default();
        self.change(file, |ed, list, _| {
            same_words(ed, &words, first.min(last), first.max(last))?;
            let r = ed.fix_words(list, &run, &fixed)?;
            if let Some(e) = &r {
                (done.others, done.heard) = (e.others.clone(), e.heard.clone());
            }
            Ok(r)
        })?;
        let recording = heard(&words);
        done.learn = if first == last {
            wordlist::learned_in(&run.heard, &fixed, &recording)
                .into_iter()
                .collect()
        } else {
            wordlist::learned_all_in(&run.heard, &fixed, &recording)
        };
        Ok(done)
    }

    /// Gives every word heard like a fixed one the same fix and saves (the Correct button's first
    /// step); what was added and the heard texts two fixes disagree on.
    pub fn spread_fixes(&self, file: Option<&str>) -> Result<Spread, String> {
        self.prepare();
        let mut out = Spread::default();
        self.change(file, |ed, list, _| {
            let (r, s) = ed.spread_all(list)?;
            out = s;
            Ok(r)
        })?;
        Ok(out)
    }

    /// Takes back the fixes a fix also made elsewhere, those still as they were (the note's
    /// Undo): their words keep what they were heard as, and no later spread reaches them. Saved.
    pub fn keep_heard(
        &self,
        file: Option<&str>,
        others: &[Fix],
        heard: &[String],
    ) -> Result<usize, String> {
        let mut n = 0;
        self.change(file, |ed, list, _| {
            let (r, k) = ed.keep_heard(list, others, heard)?;
            n = k;
            Ok(r)
        })?;
        Ok(n)
    }
}
