//! Word fixes: one word or a run retyped, a word confirmed, fixes taken away, each refused when
//! the words it was asked on changed meanwhile.

use std::sync::Arc;

use junctur_core::edit::Fix;
use junctur_core::wordlist;
use serde::Deserialize;

use super::Core;
use crate::editor::{Editor, Words};
use crate::lock::lock;

/// What a word fix did besides itself: the entries the word list is to learn (the caller
/// saves them, `settings::learn`).
#[derive(Debug, Default)]
pub struct FixDone {
    pub learn: Vec<String>,
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

/// A word as the page named it when it opened a fix field (5ae): its heard start in whole
/// milliseconds (`words.heard`), its track and its place among that track's words starting in that
/// millisecond. A save by bare index wrote onto whatever word had the index by then: a field left
/// open while its words changed put its text on other words, out of sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct WordRef {
    pub at: u64,
    pub track: usize,
    pub nth: usize,
}

/// A fix field's words are not where the page had them when it opened (5ae).
pub const FIELD_MOVED: &str = "these words are no longer where the fix field was opened (the transcript changed meanwhile): nothing was saved, open the field again on the words shown now";

/// Whether word `i` of the editor's `words` is the one `r` names. A run is checked by its two ends:
/// a word put in or taken out between them moves the last one's index onto another word.
pub(super) fn named(words: &Words, i: usize, r: WordRef) -> Result<(), String> {
    let here = words.list.get(i).map(|w| {
        (
            (w.start.max(0.0) * 1000.0).round() as u64,
            words.tracks.get(i).copied(),
            words.nth(i),
        )
    });
    if here == Some((r.at, Some(r.track), r.nth)) {
        Ok(())
    } else {
        Err(FIELD_MOVED.into())
    }
}

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
    /// Fixes the spelling of transcript word `index` in the edit list and saves it; returns what
    /// the word list is to learn. Not an undo step, like a rename.
    pub fn fix_word(
        &self,
        file: Option<&str>,
        index: usize,
        word: WordRef,
        text: &str,
    ) -> Result<FixDone, String> {
        self.retrying(|| self.fix_word_once(file, index, word, text))
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
        named_as: WordRef,
        text: &str,
    ) -> Result<FixDone, String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        named(&words, index, named_as)?;
        let (Some(word), Some(&track)) = (words.list.get(index), words.tracks.get(index)) else {
            return Err("no such word in the transcript".into());
        };
        let nth = words.nth(index);
        let fixed = junctur_core::edit::check_fix(text)?;
        self.change(file, |ed, list, _| {
            same_words(ed, &words, index, index)?;
            ed.fix_word(list, track, word, nth, &fixed)
        })?;
        Ok(FixDone {
            learn: wordlist::learned_in(&word.text, &fixed, &heard(&words))
                .into_iter()
                .collect(),
        })
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

    /// Fixes transcript words `first..=last` as one text; returns what the word list is to learn.
    /// Refused across two tracks or two people's words ([`Words::retyped`]). Not an undo step.
    pub fn fix_words(
        &self,
        file: Option<&str>,
        first: usize,
        last: usize,
        ends: [WordRef; 2],
        text: &str,
    ) -> Result<FixDone, String> {
        self.retrying(|| self.fix_words_once(file, first, last, ends, text))
    }

    fn fix_words_once(
        &self,
        file: Option<&str>,
        first: usize,
        last: usize,
        ends: [WordRef; 2],
        text: &str,
    ) -> Result<FixDone, String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        named(&words, first, ends[0])?;
        named(&words, last, ends[1])?;
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
        self.change(file, |ed, list, _| {
            same_words(ed, &words, first.min(last), first.max(last))?;
            ed.fix_words(list, &run, &fixed)
        })?;
        let recording = heard(&words);
        let learn = if first == last {
            wordlist::learned_in(&run.heard, &fixed, &recording)
                .into_iter()
                .collect()
        } else {
            wordlist::learned_all_in(&run.heard, &fixed, &recording)
        };
        Ok(FixDone { learn })
    }

    /// The list's fixes on the words of the transcript in the editor (5ae: the list of fixes the
    /// user takes wrong ones out of), in the list's order.
    pub fn fixes_list(&self) -> Result<Vec<junctur_core::edit::Placed>, String> {
        self.prepare();
        let words = lock(&self.editor)
            .words
            .clone()
            .ok_or("this recording has no transcript")?;
        let fixes = lock(&self.ctl)
            .edit
            .list
            .as_deref()
            .map(|l| l.fixes.clone())
            .ok_or("this recording has no edit list")?;
        Ok(junctur_core::edit::placed(&fixes, &words.in_order()))
    }

    /// Takes the fixes of `gone`, as [`Self::fixes_list`] gave them, out of the list (5ae), those
    /// still as listed: their words read as heard. Saved, not an undo step. How many.
    pub fn remove_fixes(&self, file: Option<&str>, gone: &[Fix]) -> Result<usize, String> {
        self.prepare();
        let mut n = 0;
        self.change(file, |ed, list, _| {
            let (r, k) = ed.remove_fixes(list, gone)?;
            n = k;
            Ok(r)
        })?;
        Ok(n)
    }
}
