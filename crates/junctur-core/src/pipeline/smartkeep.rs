//! The `keep` stage: smart keep's rules over the recording with the user's prompt ([`keep_stage`]),
//! at the end of a run or alone on an analysed recording ([`judge`]); and what the app needs around it:
//! the automatic reduction's ranges, which tell a hand edit from an untouched part, and the
//! suggestions as they stand.

use super::{
    not_analysed, speech, steps, tracks_options, Jobs, PipelineOptions, Project, Speech, Stage,
    StageName, StageState,
};
use crate::edit::{self, EditList, Range};
use crate::keep::{self, rules, Choices, Fit, Item, Judged, Summary};
use crate::labels::busy;
use crate::{tracks, Error, Result};

/// The note a `keep` stage left off carries in `jobs.json`.
pub const KEEP_OFF: &str = "off";

/// Smart keep for a run: the config file's settings, and a prompt given for this run (`--keep`), which
/// becomes the recording's.
#[derive(Debug, Clone, Default)]
pub struct KeepRun {
    pub settings: keep::Settings,
    pub prompt: Option<String>,
}

/// The rules over the recording with its prompt ([`rules::judge`]), then the window scores. Off (no
/// prompt for the recording and smart keep off) it is recorded done with the note [`KEEP_OFF`] and
/// nothing is read. No model: the busy track's peaks (the app's `waves.u8`, else that track decoded)
/// and the labels, when there are any.
pub(super) fn keep_stage(
    st: Stage,
    speech: &Speech,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Option<Judged>> {
    let Stage {
        p, jobs, report, ..
    } = st;
    let mut choices = keep::load_choices(&p.dir)?;
    let given = opts.keep.prompt.as_deref().map(str::trim);
    if let Some(given) = given.filter(|g| !g.is_empty()) {
        if choices.prompt.as_deref() != Some(given) {
            choices.prompt = Some(given.to_string());
            keep::save_choices(&p.dir, &choices)?;
        }
    }
    let Some(prompt) = keep::prompt_for(&choices, &opts.keep.settings) else {
        let off = Some(KEEP_OFF.to_string());
        jobs.set(&p.dir, StageName::Keep, StageState::Done, None, off)?;
        return Ok(None);
    };
    let list = p.edit()?;
    let regions = speech.all_regions();
    let labels = super::labels_now(p, jobs, &opts.labels, &regions, report);
    // Talk only keeps no silence: the game track is not decoded for nothing (3e).
    let talk_only = rules::Strictness::read(&prompt) == rules::Strictness::TalkOnly;
    let judged = jobs.stage(&p.dir, StageName::Keep, log, |log| {
        let peaks = match busy::track(report).filter(|_| !talk_only) {
            None if talk_only => None,
            None => {
                log("keep: no game or mix track, so the labels' busy alone");
                None
            }
            Some(t) => match busy::peaks(&p.dir, &p.file, report, t.track) {
                Ok(v) => Some(v),
                Err(e) => {
                    log(&format!(
                        "keep: the game audio was not read ({e}), so the labels' busy alone"
                    ));
                    None
                }
            },
        };
        let judged = rules::judge(&rules::Input {
            prompt: &prompt,
            speech: &regions,
            duration: list.duration,
            labels: labels.as_ref(),
            peaks: peaks.as_deref(),
        });
        keep::save(&p.dir, &judged)?;
        Ok(judged)
    })?;
    let amended = edit::amend(&p.edit_path(), |e| {
        rules::score_windows(&mut e.windows, e.settings.window, &judged);
    });
    if let Ok(edit::Amended::Left) = amended {
        log("edit list: edited outside Junctur, its scores are left as they are");
    }
    log(&rules::summary(&judged));
    Ok(Some(judged))
}

/// The `keep` stage alone, over a recording whose analysis is done (the app's Judge, `junctur
/// judge`): `None` when it is off for it. It waits for the transcript although the rules do not read
/// it: one not done may be a run's still going, whose word counts rewrite the edit list this stage
/// amends, and the jobs file cannot tell it from a run without one (3e kept the gate; such a run
/// judges at its end, `analyze --no-transcript --keep`).
pub fn judge(
    p: &Project,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Option<Judged>> {
    let speech = p.speech().ok_or_else(not_analysed)?;
    let mut jobs: Jobs = p.jobs();
    jobs.file = p.file.clone();
    let done = |s| jobs.get(s).is_some_and(|r| r.state == StageState::Done);
    if !done(StageName::Transcribe) {
        return Err(Error::Edit(
            "the transcript is not finished yet: smart keep runs once it is (without a \
             transcript, it runs at the end of `analyze --no-transcript --keep <prompt>`)"
                .into(),
        ));
    }
    let report = tracks::analyze(&p.file, &tracks_options(p, false))?;
    let stage = Stage {
        p,
        jobs: &mut jobs,
        report: &report,
        steps: steps::none(),
    };
    keep_stage(stage, &speech, opts, log)
}

/// Smart keep's suggested keeps for `p` as it stands now: `keep.json` only when the recording's
/// prompt now (`choices`, `settings`) is the one it was judged with and the last keep stage did not
/// fail. Otherwise the file is an old run's, and its stretches would be suggested for a prompt the
/// user has since changed or turned off (3e).
pub fn judged_now(p: &Project, choices: &Choices, settings: &keep::Settings) -> Option<Judged> {
    let failed = p
        .jobs()
        .get(StageName::Keep)
        .is_some_and(|r| r.state == StageState::Failed);
    let prompt = keep::prompt_for(choices, settings)?;
    keep::load(&p.dir).filter(|j| !failed && j.prompt.trim() == prompt)
}

/// What the automatic reduction keeps of `p` with `list`'s settings: a part the list does otherwise
/// with was edited by hand. `None` before the speech is found.
pub fn automatic_keep(p: &Project, list: &EditList) -> Option<Vec<Range>> {
    let speech = p.speech()?;
    let env = speech::load_envelope(&p.envelope_path()).unwrap_or_default();
    Some(edit::keep_ranges(
        &speech.all_regions(),
        &env,
        list.duration,
        &list.settings,
    ))
}

/// What `p`'s suggestions are now, the one reading of them the app's lane and its answers share, so
/// Accept all applies the parts the lane showed: the automatic cut (the list itself before the speech
/// is found) and the suggestions against it. The fit is the default one: the rules' margin is not
/// refitted (3b's pairs and refit were of Laya's margin; the rules' is a distance above the
/// recording's own line, which a fit across recordings would only blur).
pub struct Standing {
    pub auto: Vec<Range>,
    pub items: Vec<Item>,
    pub summary: Summary,
}

pub fn standing(p: &Project, judged: &Judged, list: &EditList, choices: &Choices) -> Standing {
    let auto = automatic_keep(p, list).unwrap_or_else(|| list.keep.clone());
    let (items, summary) = keep::suggestions(judged, &Fit::default(), &list.keep, &auto, choices);
    Standing {
        auto,
        items,
        summary,
    }
}
