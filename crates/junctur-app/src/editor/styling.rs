//! What the list says beside its cuts: speaker names and colours, caption settings and speakers
//! heard again. Saved as made, not undo steps.

use junctur_core::edit::{self, check_name, Captions, EditList};

use super::{Edited, Editor};

impl Editor {
    /// Renames speaker `id` and saves; `None` when the name is already that. Like every styling
    /// change, not an undo step: the history holds kept ranges and retimed words only, so undoing a
    /// cut keeps the new name.
    pub fn rename(
        &mut self,
        list: &EditList,
        id: u32,
        name: &str,
    ) -> Result<Option<Edited>, String> {
        let name = check_name(name)?;
        self.restyle(list, |l| {
            l.speakers
                .rename(id, name)
                .then_some(())
                .ok_or(format!("there is no speaker {id}"))
        })
    }

    /// Sets speaker `id`'s caption colour (`#RRGGBB`) and saves.
    pub fn recolor(
        &mut self,
        list: &EditList,
        id: u32,
        color: &str,
    ) -> Result<Option<Edited>, String> {
        let color = edit::parse_color(color.trim())
            .ok_or_else(|| format!("{color:?} is not a colour like #F2B33D"))?;
        self.restyle(list, |l| {
            l.speakers
                .recolor(id, &color)
                .then_some(())
                .ok_or(format!("there is no speaker {id}"))
        })
    }

    /// Sets the caption settings (switches, style, how cues are cut) and saves.
    pub fn set_captions(
        &mut self,
        list: &EditList,
        captions: Captions,
    ) -> Result<Option<Edited>, String> {
        captions.check()?;
        self.restyle(list, |l| {
            l.captions = Some(captions);
            Ok(())
        })
    }

    /// Puts `speakers` in place of the list's (labelled again after a voice was enrolled or
    /// forgotten) and saves. Like a rename, not an undo step.
    pub fn respeak(
        &mut self,
        list: &EditList,
        speakers: junctur_core::edit::Speakers,
    ) -> Result<Option<Edited>, String> {
        self.restyle(list, |l| {
            l.speakers = speakers;
            Ok(())
        })
    }

    /// Applies `f` to a copy of `list` and saves it; `None` when it changed nothing.
    pub(super) fn restyle(
        &mut self,
        list: &EditList,
        f: impl FnOnce(&mut EditList) -> Result<(), String>,
    ) -> Result<Option<Edited>, String> {
        let mut new = list.clone();
        f(&mut new)?;
        if new == *list {
            return Ok(None);
        }
        let path = self.path.clone().ok_or("no edit list is open")?;
        let saved =
            edit::save(&path, &new).map_err(|e| format!("saving {}: {e}", path.display()))?;
        self.written = Some(saved.text.clone());
        Ok(Some(Edited {
            list: new,
            saved,
            unsnapped: false,
            skipped: 0,
        }))
    }
}
