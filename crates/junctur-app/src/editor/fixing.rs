//! Word fixes in the edit list: a word or a run retyped, a word confirmed, fixes spread to the
//! words heard alike or taken back. Saved as made, not undo steps.

use std::sync::{Arc, Weak};

use junctur_core::edit::{self, EditList, Fix, Spread, SpreadIndex, Which, WordAt};
use junctur_core::transcribe::Word;

use super::{Edited, Editor, Retyped};

impl Editor {
    /// Fixes the spelling of `word`, heard on `track`, the `nth` of that track's words starting in
    /// its millisecond ([`super::Words::nth`]), and saves; empty text or the heard text again takes
    /// the fix away. Every word heard like it gets the same fix in the same save
    /// ([`edit::spread`]), [`Edited::others`] naming those. Like a rename, not an undo step.
    pub fn fix_word(
        &mut self,
        list: &EditList,
        track: usize,
        word: &Word,
        nth: usize,
        text: &str,
    ) -> Result<Option<Edited>, String> {
        let at = WordAt {
            start: word.start,
            nth,
        };
        let mut fixes = list.fixes.clone();
        edit::set_fix(&mut fixes, track, word.start, nth, &word.text, text)?;
        let s = self.spread(
            &mut fixes,
            Which::Words {
                track,
                first: at,
                last: at,
            },
        );
        self.set_fixes(list, fixes, s)
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
    /// takes them all away. Spreads as [`Self::fix_word`] does. Not an undo step.
    pub fn fix_words(
        &mut self,
        list: &EditList,
        r: &Retyped,
        text: &str,
    ) -> Result<Option<Edited>, String> {
        let mut fixes = list.fixes.clone();
        edit::set_span(&mut fixes, r.track, r.first, r.last, &r.heard, text)?;
        let s = self.spread(
            &mut fixes,
            Which::Words {
                track: r.track,
                first: r.first,
                last: r.last,
            },
        );
        self.set_fixes(list, fixes, s)
    }

    /// Gives every word heard like a fixed one the same fix (the Correct button's first step),
    /// and saves; what was added and the heard texts two fixes disagree on. Not an undo step.
    pub fn spread_all(&mut self, list: &EditList) -> Result<(Option<Edited>, Spread), String> {
        let mut fixes = list.fixes.clone();
        let s = self.spread(&mut fixes, Which::All);
        let edited = self.set_fixes(list, fixes, s.clone())?;
        Ok((edited, s))
    }

    /// Takes back the fixes a fix also made elsewhere (the note's Undo), those still as they
    /// were: their words keep what they were heard as (`heard`), as fixes of the heard text, so no
    /// later spread gives them the fix again ([`edit::keep_heard`]). Saved; not an undo step.
    pub fn keep_heard(
        &mut self,
        list: &EditList,
        others: &[Fix],
        heard: &[String],
    ) -> Result<(Option<Edited>, usize), String> {
        let mut fixes = list.fixes.clone();
        let n = edit::keep_heard(&mut fixes, others, heard);
        Ok((self.set_fixes(list, fixes, Spread::default())?, n))
    }

    /// [`edit::spread`] over the transcript the editor has (none yet: nothing to spread over).
    fn spread(&mut self, fixes: &mut Vec<Fix>, which: Which) -> Spread {
        let Some(w) = self.words.clone() else {
            return Spread::default();
        };
        let words = w.in_order();
        let index = match &self.spread_index {
            Some((of, index)) if Weak::ptr_eq(of, &Arc::downgrade(&w)) => index.clone(),
            _ => {
                let index = Arc::new(SpreadIndex::new(&words));
                self.spread_index = Some((Arc::downgrade(&w), index.clone()));
                index
            }
        };
        edit::spread_with(fixes, &words, &index, which)
    }

    /// Saves `fixes` as the list's, `s` saying what of them a spread added.
    fn set_fixes(
        &mut self,
        list: &EditList,
        fixes: Vec<Fix>,
        s: Spread,
    ) -> Result<Option<Edited>, String> {
        // Never save a list the parser would refuse; a spread checks this itself, this is the
        // last line.
        if edit::clash(&fixes) {
            return Err("that would give a word two fixes: nothing was changed".into());
        }
        let edited = self.restyle(list, |l| {
            l.fixes = fixes;
            Ok(())
        })?;
        Ok(edited.map(|e| Edited {
            others: s.added,
            heard: s.heard,
            ..e
        }))
    }
}
