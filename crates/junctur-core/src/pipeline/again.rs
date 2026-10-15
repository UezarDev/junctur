//! Transcribing a recording again: the mark that asks for it, and the user's fixed words and word
//! times carried from the old transcript onto the new one, or set aside where they land on no word.

use super::{PipelineOptions, Project};
use crate::edit::{self, EditList};
use crate::transcribe::{self, Transcript};
use crate::Result;

/// There while a re-transcription asked for (the Correct button) has not finished: a run resumed
/// after the app closed still hears the recording again and carries the fixes onto the new words
/// ([`hear_again`], [`carry_fixes`]). Empty; the old transcript is kept in [`BEFORE_FILE`].
pub(super) const AGAIN_FILE: &str = "transcribe-again";

/// What [`AGAIN_FILE`] holds once the re-transcription it asks for has written its transcript and
/// the carry is still to come: a run that finds it carries rather than hearing the
/// recording again. Empty while the hearing is still asked for.
const HEARD: &[u8] = b"heard";

/// The transcript a re-transcription replaces, copied when it is asked for: the fixes are
/// read against it at the end, and `transcript.json` is the new one from the moment Whisper finishes,
/// so a run that stopped between that write and the carry would otherwise carry against the new words.
pub(super) const BEFORE_FILE: &str = "transcript.before.json";

/// Fixes a re-transcription could not put on any word (Whisper heard something else there, or
/// nothing), as they were, for the user to see and redo: kept until the app is told they were seen
/// ([`forget_fixes_set_aside`]), each later run adding to them.
pub(super) const SET_ASIDE_FILE: &str = "fixes-set-aside.json";

/// A fix a re-transcription set aside ([`SET_ASIDE_FILE`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAside {
    pub track: usize,
    /// Where its words started, in source seconds.
    pub at: f64,
    /// What they were heard as before.
    pub heard: String,
    /// What the user had fixed them to; for a word time, the word as fixed or heard.
    pub text: String,
    /// A word time set by moving a cut's edge that the re-transcription could not put on a
    /// word: the start and end the user had given it. `None` for a fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retimed: Option<[f64; 2]>,
    /// Its track has no words in the new transcript at all: not heard otherwise, not
    /// heard (or not transcribed) this time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub silent: bool,
    /// Not a fix: what the carry could not do, said where the user looks until they have seen it,
    /// since a log line is gone by the run's end. The other fields are empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl SetAside {
    /// A [`SetAside::note`].
    fn note(text: &str) -> Self {
        SetAside {
            track: 0,
            at: 0.0,
            heard: String::new(),
            text: String::new(),
            retimed: None,
            silent: false,
            note: Some(text.to_string()),
        }
    }
}

/// The fixes re-transcriptions of `p` set aside and the user has not yet seen.
pub fn fixes_set_aside(p: &Project) -> Vec<SetAside> {
    std::fs::read(p.dir.join(SET_ASIDE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Forgets the fixes set aside for `p` that the user has seen (`seen`, as [`fixes_set_aside`] gave
/// them): only those, so any a run added since the panel listed them stay. Under the edit list's
/// writer lock, as a run's additions are ([`keep_set_aside`]), so neither lands inside the other.
pub fn forget_fixes_set_aside(p: &Project, seen: &[SetAside]) -> Result<()> {
    let _writing = edit::hold();
    let path = p.dir.join(SET_ASIDE_FILE);
    let left: Vec<SetAside> = fixes_set_aside(p)
        .into_iter()
        .filter(|s| !seen.contains(s))
        .collect();
    if !left.is_empty() {
        crate::output::write_replacing(&path, serde_json::to_vec_pretty(&left)?)?;
        return Ok(());
    }
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Whether this run's transcription hears the recording again: asked for now (`opts.retranscribe`,
/// which leaves [`AGAIN_FILE`]), or by an earlier run that stopped before its first chunk. An earlier
/// ask whose carry never ran keeps its copy of the old transcript, which the fixes are on; one whose
/// transcription finished is carried, not heard again.
pub(super) fn hear_again(
    p: &Project,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<bool> {
    let mark = p.dir.join(AGAIN_FILE);
    if opts.retranscribe {
        if mark.is_file() {
            // Asked for again: heard again, whatever the last ask got to.
            crate::output::write_replacing(&mark, b"")?;
            log("transcribing again with the word list as it is now (the fixed words still on the transcript before the last ask)");
            return Ok(true);
        }
        // The old transcript first, then the mark: a mark is never there without its copy.
        let before = p.dir.join(BEFORE_FILE);
        match std::fs::read(transcribe::path(&p.dir)) {
            Ok(b) => crate::output::write_replacing(&before, b)?,
            Err(_) => {
                let _ = std::fs::remove_file(&before);
            }
        }
        if let Err(e) = crate::output::write_replacing(&mark, b"") {
            // Nor a copy without its mark: nothing would ever remove it.
            let _ = std::fs::remove_file(&before);
            return Err(e.into());
        }
        log("transcribing again with the word list as it is now");
        return Ok(true);
    }
    if !mark.is_file() || transcribe::interrupted(&p.dir) {
        return Ok(false);
    }
    if heard_again_already(p) {
        log("the transcription asked for again had finished: its fixed words are carried now");
        return Ok(false);
    }
    Ok(true)
}

/// A re-transcription asked for has written its transcript: the mark says so ([`HEARD`], written by
/// [`heard_again`] once the transcription stage ends) and the transcript is there. Not told by the
/// transcript differing from the copy, which a hearing that came out the same never would.
fn heard_again_already(p: &Project) -> bool {
    std::fs::read(p.dir.join(AGAIN_FILE)).is_ok_and(|m| m == HEARD)
        && transcribe::path(&p.dir).is_file()
}

/// The transcription stage of a run with [`AGAIN_FILE`] there has ended with the new transcript
/// the mark says so, so a run stopped before the carry carries on the next open
/// instead of hearing the recording again.
pub(super) fn heard_again(p: &Project, log: &mut dyn FnMut(&str)) {
    let mark = p.dir.join(AGAIN_FILE);
    if mark.is_file() {
        if let Err(e) = crate::output::write_replacing(&mark, HEARD) {
            log(&format!(
                "could not mark the transcription as heard again ({e})"
            ));
        }
    }
}

/// A transcription that failed: a re-transcription asked for is forgotten when nothing of
/// it is left to resume, or every later run of the recording would hear it again from the start. One
/// with a partial keeps its mark: the next run resumes it, and its end must still carry the fixes.
pub(super) fn not_again(p: &Project, log: &mut dyn FnMut(&str)) {
    if transcribe::interrupted(&p.dir) || !p.dir.join(AGAIN_FILE).is_file() {
        return;
    }
    for f in [AGAIN_FILE, BEFORE_FILE] {
        let path = p.dir.join(f);
        if let Err(e) = crate::output::remove_retrying(&path) {
            if path.exists() {
                log(&format!("could not remove {}: {e}", path.display()));
            }
        }
    }
    log("the transcription again stopped before its first chunk: asked for no longer");
}

/// After a re-transcription ([`AGAIN_FILE`] there): the fixes and retimes, read against the old
/// transcript ([`BEFORE_FILE`]) at the end so edits made during the run count, move onto the new
/// words ([`edit::carry`], [`edit::carry_times`]) and fixes spread ([`edit::spread`]); what lands on
/// no word is set aside ([`SET_ASIDE_FILE`]). Nothing is carried with no old copy or no new words.
/// The mark and copy always go, or they would ask for another re-transcription.
pub(super) fn carry_fixes(p: &Project, t: &Transcript, log: &mut dyn FnMut(&str)) {
    let mark = p.dir.join(AGAIN_FILE);
    if !mark.is_file() {
        return;
    }
    // The set-aside file is written under the same lock as the list, so the app's "seen"
    // cannot land between its read and its write.
    let _writing = edit::hold();
    let old = std::fs::read(p.dir.join(BEFORE_FILE))
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Transcript>(&b).map_err(|e| e.to_string()));
    let words = t.in_order();
    // Said where the user looks too, when there was anything to carry.
    let left = |why: &str, log: &mut dyn FnMut(&str)| {
        log(&format!(
            "{why}: fixed words and word times are left where they were"
        ));
        let any = p
            .edit()
            .is_ok_and(|e| !e.fixes.is_empty() || !e.times.is_empty());
        let note = format!("Correct could not move your fixed words and word times onto the new transcript ({why}); they are where they were, and some may now show on other words.");
        if any {
            if let Err(e) = keep_set_aside(p, vec![SetAside::note(&note)]) {
                log(&format!("{SET_ASIDE_FILE} not written ({e})"));
            }
        }
    };
    match old {
        Err(e) => left(&format!("the transcript before was not kept: {e}"), log),
        Ok(_) if words.is_empty() => left("the new transcript has no words", log),
        Ok(old) => carry_onto(p, &old.in_order(), &words, log),
    }
    for f in [AGAIN_FILE, BEFORE_FILE] {
        let path = p.dir.join(f);
        if let Err(e) = crate::output::remove_retrying(&path) {
            if path.exists() {
                log(&format!("could not remove {}: {e}", path.display()));
            }
        }
    }
}

/// What [`carry_all`] did to a list.
#[derive(Default)]
struct Carry {
    fixes: edit::CarryReport,
    times: edit::TimesReport,
    spread: edit::Spread,
}

/// How much of a carry the list could take ([`carry_onto`]).
#[derive(PartialEq)]
enum Took {
    /// Carried and spread.
    All,
    /// Carried; the spread would not have read back.
    Carried,
    /// Nothing: left as it was.
    Nothing,
}

/// The fixes and retimes of `e` moved from `before`'s words onto `words`, every fix spread after
/// when `spread`.
fn carry_all(
    e: &mut EditList,
    before: &[(usize, &transcribe::Word)],
    words: &[(usize, &transcribe::Word)],
    spread: bool,
) -> Carry {
    // The retimes are read with the fixes as they are on the old words (a word heard now as its fix
    // is the same word).
    let timed = edit::snapshot_times(&e.times, &e.fixes, before);
    let carried = edit::snapshot(&e.fixes, before);
    let mut done = Carry {
        fixes: edit::carry(&mut e.fixes, &carried, words),
        times: edit::carry_times(&mut e.times, &timed, words),
        ..Carry::default()
    };
    if spread {
        done.spread = edit::spread(&mut e.fixes, words, edit::Which::All);
    }
    done
}

/// [`carry_fixes`]' work on the list, `before` and `words` the old and new transcripts. A result
/// the parser would refuse ([`EditList::checked_text`]) is retried without the spread, keeping its
/// set-aside; if that fails too, the list stays as it was and nothing is set aside.
fn carry_onto(
    p: &Project,
    before: &[(usize, &transcribe::Word)],
    words: &[(usize, &transcribe::Word)],
    log: &mut dyn FnMut(&str),
) {
    let mut done = Carry::default();
    let mut took = Took::Nothing;
    let amended = edit::amend(&p.edit_path(), |e| {
        let was = (e.fixes.clone(), e.times.clone());
        for (spread, how) in [(true, Took::All), (false, Took::Carried)] {
            let d = carry_all(e, before, words, spread);
            if e.checked_text().is_ok() {
                (done, took) = (d, how);
                return;
            }
            (e.fixes, e.times) = was.clone();
        }
    });
    // A track the new transcript has no words on: its fixes are set aside as not heard, not as
    // heard otherwise.
    let heard: std::collections::HashSet<usize> = words.iter().map(|&(t, _)| t).collect();
    let why = |track: usize| {
        if heard.contains(&track) {
            "is heard otherwise now"
        } else {
            "is on a track with no words this time"
        }
    };
    let mut aside: Vec<SetAside> = Vec::new();
    match amended {
        Ok(edit::Amended::Written(_) | edit::Amended::Same(_)) if took == Took::Nothing => {
            log("edit list: carrying the fixed words would give a list that does not read back; they are left as they were");
            aside.push(SetAside::note("Correct could not move your fixed words and word times onto the new transcript (the edit list would not have read back); they are where they were, and some may now show on other words."));
        }
        Ok(edit::Amended::Written(_) | edit::Amended::Same(_)) => {
            if took == Took::Carried {
                log("edit list: spreading the fixed words would give a list that does not read back; carried without spreading");
                aside.push(SetAside::note("Correct moved your fixed words onto the new transcript but did not give them to the other words heard the same way (the edit list would not have read back)."));
            }
            log(&carried_line(&done));
            for g in &done.fixes.gone {
                log(&format!(
                    "fixed word set aside: \"{}\" (heard \"{}\") at {} {}",
                    g.fix.text,
                    g.heard.join(" "),
                    clock(g.from),
                    why(g.fix.track)
                ));
                aside.push(SetAside {
                    track: g.fix.track,
                    at: g.from,
                    heard: g.heard.join(" "),
                    text: g.fix.text.clone(),
                    retimed: None,
                    silent: !heard.contains(&g.fix.track),
                    note: None,
                });
            }
            for g in &done.times.gone {
                log(&format!(
                    "word time set aside: \"{}\" at {} {}",
                    g.heard,
                    clock(g.start),
                    why(g.time.track)
                ));
                aside.push(SetAside {
                    track: g.time.track,
                    at: g.start,
                    heard: g.heard.clone(),
                    text: g.fixed.clone().unwrap_or_else(|| g.heard.clone()),
                    retimed: Some([g.time.from, g.time.to]),
                    silent: !heard.contains(&g.time.track),
                    note: None,
                });
            }
        }
        Ok(edit::Amended::Left) => {
            log("edit list: edited outside Junctur, its fixed words are left as they are")
        }
        Ok(edit::Amended::Missing) => {}
        Err(e) => log(&format!("edit list: fixed words not carried ({e})")),
    }
    if let Err(e) = keep_set_aside(p, aside) {
        log(&format!("{SET_ASIDE_FILE} not written ({e})"));
    }
}

/// Adds `new` to the fixes set aside for `p` ([`SET_ASIDE_FILE`]). Called under the edit list's
/// writer lock ([`carry_fixes`]), as [`forget_fixes_set_aside`] is.
fn keep_set_aside(p: &Project, new: Vec<SetAside>) -> Result<()> {
    if new.is_empty() {
        return Ok(());
    }
    let mut all = fixes_set_aside(p);
    all.extend(new);
    crate::output::write_replacing(
        &p.dir.join(SET_ASIDE_FILE),
        serde_json::to_vec_pretty(&all)?,
    )?;
    Ok(())
}

/// The log line on fixes carried onto a new transcript.
fn carried_line(c: &Carry) -> String {
    let (r, s, t) = (&c.fixes, &c.spread, &c.times);
    let mut line = format!(
        "fixed words: {} kept on their words, {} no longer needed, {} set aside; {} more place(s) fixed",
        r.kept,
        r.right,
        r.gone.len(),
        s.added.len()
    );
    if !s.unclear.is_empty() {
        line.push_str(&format!(
            "; fixed two ways, left as heard: {}",
            s.unclear.join(", ")
        ));
    }
    if t.kept + t.right + t.gone.len() > 0 {
        line.push_str(&format!(
            "; word times: {} kept on their words, {} no longer needed, {} set aside",
            t.kept,
            t.right,
            t.gone.len()
        ));
    }
    line
}

/// `h:mm:ss` of source seconds, for the log.
fn clock(s: f64) -> String {
    let t = s.max(0.0) as u64;
    format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}
