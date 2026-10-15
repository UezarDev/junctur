//! Word fixes in the edit list: a word or a run retyped, a word confirmed, fixes taken away. Each
//! changes only the words it names. Saved as made, not undo steps.

use junctur_core::edit::{self, EditList, Fix};
use junctur_core::transcribe::Word;

use super::{Edited, Editor, Retyped};

impl Editor {
    /// Fixes the spelling of `word`, heard on `track`, the `nth` of that track's words starting in
    /// its millisecond ([`super::Words::nth`]), and saves; empty text or the heard text again takes
    /// the fix away. Only this word: one heard alike elsewhere is left as heard. Like a rename,
    /// not an undo step.
    pub fn fix_word(
        &mut self,
        list: &EditList,
        track: usize,
        word: &Word,
        nth: usize,
        text: &str,
    ) -> Result<Option<Edited>, String> {
        let mut fixes = list.fixes.clone();
        edit::set_fix(&mut fixes, track, word.start, nth, &word.text, text)?;
        self.set_fixes(list, fixes)
    }

    /// Marks `word` (as for [`Self::fix_word`]) as checked by the user, a fix line with the heard
    /// text so the transcript panel stops flagging it, and saves; a word fixed already is left as
    /// it is. Not an undo step.
    pub fn confirm_word(
        &mut self,
        list: &EditList,
        track: usize,
        word: &Word,
        nth: usize,
    ) -> Result<Option<Edited>, String> {
        self.restyle(list, |l| {
            edit::confirm_fix(&mut l.fixes, track, word.start, nth, &word.text).map(|_| ())
        })
    }

    /// Sets the text of the run `r` names ([`super::Words::retyped`]) as one fix, and saves; the
    /// fixes of words inside it, and a span overlapping it, go. Empty text or the heard text again
    /// takes them all away. Only these words, as [`Self::fix_word`]. Not an undo step.
    pub fn fix_words(
        &mut self,
        list: &EditList,
        r: &Retyped,
        text: &str,
    ) -> Result<Option<Edited>, String> {
        let mut fixes = list.fixes.clone();
        edit::set_span(&mut fixes, r.track, r.first, r.last, &r.heard, text)?;
        self.set_fixes(list, fixes)
    }

    /// Takes the fixes of `gone` out of the list (5ae: the list of fixes' Remove), those still as
    /// listed ([`edit::remove`]): their words read as heard. Saved; not an undo step. How many.
    pub fn remove_fixes(
        &mut self,
        list: &EditList,
        gone: &[Fix],
    ) -> Result<(Option<Edited>, usize), String> {
        let mut fixes = list.fixes.clone();
        let n = edit::remove(&mut fixes, gone);
        Ok((self.set_fixes(list, fixes)?, n))
    }

    /// Saves `fixes` as the list's.
    fn set_fixes(&mut self, list: &EditList, fixes: Vec<Fix>) -> Result<Option<Edited>, String> {
        // Never save a list the parser would refuse.
        if edit::clash(&fixes) {
            return Err("that would give a word two fixes: nothing was changed".into());
        }
        self.restyle(list, |l| {
            l.fixes = fixes;
            Ok(())
        })
    }
}
