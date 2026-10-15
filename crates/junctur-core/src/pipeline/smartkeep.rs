//! The `keep` stage: Laya judges the recording's parts with the user's prompt ([`keep`]), at the end
//! of a run or alone on an analysed recording ([`judge`]); and what the app needs around it: the
//! automatic reduction's ranges, which tell a hand edit from an untouched part, and the pairs.

use std::path::Path;

use super::{
    not_analysed, speech, steps, tracks_options, Jobs, PipelineOptions, Project, Speech, Stage,
    StageName, StageState,
};
use crate::edit::{self, EditList, Range};
use crate::keep::{self, Choices, Fit, Judged};
use crate::labels;
use crate::transcribe::{self, Transcript};
use crate::{tracks, Error, Result};

/// The note a `keep` stage left off carries in `jobs.json` (the labels' word).
pub const KEEP_OFF: &str = super::LABELS_OFF;

/// Smart keep for a run: the config file's settings, and a prompt given for this run (`--keep`), which
/// becomes the recording's.
#[derive(Debug, Clone, Default)]
pub struct KeepRun {
    pub settings: keep::Settings,
    pub prompt: Option<String>,
}

/// Laya over the recording with its prompt, then the window scores and the pairs. Off (no prompt for
/// the recording and smart keep off) it is recorded done with the note [`KEEP_OFF`] and nothing is
/// read.
pub(super) fn keep_stage(
    st: Stage,
    speech: &Speech,
    t: &Transcript,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Option<Judged>> {
    let Stage { p, jobs, .. } = st;
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
    let labels_off = jobs
        .get(StageName::Labels)
        .is_some_and(|r| r.note.as_deref() == Some(super::LABELS_OFF));
    let labels = labels::load(&p.dir).filter(|_| !labels_off);
    let regions = speech.all_regions();
    let job = keep::Job {
        dir: &p.dir,
        cache_dir: &p.cache_dir,
        prompt: &prompt,
        speech: &regions,
        duration: list.duration,
        transcript: t,
        speakers: &list.speakers,
        labels: labels.as_ref(),
        device: opts.device,
        force: opts.force,
    };
    let judged = jobs.stage(&p.dir, StageName::Keep, log, |log| keep::run(&job, log))?;
    let pairs = keep::pairs_path();
    let fit = learn(p, &judged, &list, &choices, pairs.as_deref()).unwrap_or_else(|e| {
        log(&format!("keep: your decisions were not read ({e})"));
        keep::fitted(pairs.as_deref())
    });
    let amended = edit::amend(&p.edit_path(), |e| {
        keep::score_windows(&mut e.windows, e.settings.window, &judged, &fit);
    });
    if let Ok(edit::Amended::Left) = amended {
        log("edit list: edited outside Junctur, its scores are left as they are");
    }
    log(&keep::summary(&judged, &fit));
    Ok(Some(judged))
}

/// The `keep` stage alone, over a recording whose analysis is done (the app's Judge, `junctur
/// judge`): `None` when it is off for it.
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
            "the transcript is not finished yet: Laya judges once it is".into(),
        ));
    }
    let t = transcribe::load(&p.dir).ok_or_else(not_analysed)?;
    let report = tracks::analyze(&p.file, &tracks_options(p, false))?;
    let stage = Stage {
        p,
        jobs: &mut jobs,
        report: &report,
        steps: steps::none(),
    };
    keep_stage(stage, &speech, &t, opts, log)
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

/// Collects `p`'s pairs as `list` and `choices` stand ([`keep::collect`]) into the pairs file at
/// `pairs` (the user's is [`keep::pairs_path`]) and returns the fit of every pair in it. Without a
/// file to keep them in, the defaults; before the speech is found, the fit of what is there.
pub fn learn(
    p: &Project,
    judged: &Judged,
    list: &EditList,
    choices: &Choices,
    pairs: Option<&Path>,
) -> Result<Fit> {
    let Some(path) = pairs else {
        return Ok(Fit::default());
    };
    let Some(auto) = automatic_keep(p, list) else {
        return Ok(keep::fitted(Some(path)));
    };
    let found = keep::collect(&p.file, judged, &list.keep, &auto, choices);
    let all = keep::store_pairs(path, &p.file, found)?;
    let v: Vec<(f64, bool)> = all.iter().map(|x| (x.margin, x.keep)).collect();
    Ok(keep::fit::fit(&v))
}
