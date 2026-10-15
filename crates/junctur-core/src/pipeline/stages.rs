//! The stages between the tracks and the edit list, and Whisper after it: each reused from the
//! project folder unless forced.

use super::speaker_files::{set_aside, write_speakers};
use super::{
    describe, speakers, speech, speech_tracks, steps, Found, Jobs, PipelineOptions, Project,
    Speech, StageName, StageState, Steps,
};
use crate::edit::{self, EditList};
use crate::labels::{self, Labels};
use crate::tracks::{Role, TrackReport};
use crate::transcribe::{self, Source, TranscribeOptions, Transcript};
use crate::Result;

/// What every stage of a run is given: the project, its job record, the tracks report and where
/// each track's progress goes.
pub(super) struct Stage<'a> {
    pub(super) p: &'a Project,
    pub(super) jobs: &'a mut Jobs,
    pub(super) report: &'a TrackReport,
    pub(super) steps: Steps<'a>,
}

/// Puts the transcript's word counts into the score windows of the edit list on disk, changing
/// nothing else, when rewriting it loses nothing of the user's ([`edit::amend`]); the list on disk
/// afterwards, when it reads.
pub(super) fn count_words(
    p: &Project,
    speech: &Speech,
    t: &Transcript,
    log: &mut dyn FnMut(&str),
) -> Result<Option<EditList>> {
    let regions = speech.all_regions();
    let starts: Vec<f64> = t.words().map(|w| w.start).collect();
    let amended = edit::amend(&p.edit_path(), |e| {
        e.windows = edit::windows(&regions, &starts, e.duration, e.settings.window);
    })?;
    Ok(match amended {
        edit::Amended::Written(e) | edit::Amended::Same(e) => Some(e),
        edit::Amended::Left => {
            log("edit list: edited outside Junctur, its word counts are left as they are");
            None
        }
        edit::Amended::Missing => None,
    })
}

/// Speech regions and the cut envelope on every speech track, reused unless `force`.
pub(super) fn speech_stage(st: Stage, force: bool, log: &mut dyn FnMut(&str)) -> Result<Speech> {
    let Stage {
        p,
        jobs,
        report,
        steps,
    } = st;
    let sources = speech_tracks(report);
    if sources.is_empty() {
        log("no track carries speech: the edit list will be empty");
    }
    let speech = match p.speech().filter(|_| !force) {
        Some(s) if p.envelope_path().is_file() => {
            jobs.skip(&p.dir, StageName::Speech, log)?;
            s
        }
        _ => jobs.stage(&p.dir, StageName::Speech, log, |_| {
            let read = |track, at| steps(steps::step(report, StageName::Speech, track, at));
            let (s, env) = speech::detect(&p.file, &sources, &p.cache_dir, &read)?;
            speech::save_envelope(&p.envelope_path(), &env)?;
            std::fs::write(p.speech_path(), serde_json::to_vec_pretty(&s)?)?;
            Ok(s)
        })?,
    };
    for t in &speech.tracks {
        log(&format!(
            "track {}: speech {:.0}% of the time, {} regions",
            t.track + 1,
            t.share * 100.0,
            t.regions.len()
        ));
    }
    Ok(speech)
}

/// Who talks when, reused unless `force` or the speech regions it was found from changed.
pub(super) fn speakers_stage(
    st: Stage,
    speech: &Speech,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Found> {
    let Stage {
        p,
        jobs,
        report,
        steps,
    } = st;
    // An unreadable file holds pins nothing else knows: kept aside under another name and
    // said, not silently replaced by a file without them.
    let previous = match p.speakers_file() {
        Ok(f) => f,
        Err(e) => {
            let kept = set_aside(&p.speakers_path())?;
            log(&format!(
                "speakers.json could not be read ({e}): kept as {}; the seconds pinned to people in it are not used",
                kept.display()
            ));
            None
        }
    };
    let cached = previous
        .clone()
        .filter(|_| !opts.force)
        .filter(|_| !opts.rehear)
        .and_then(|f| speakers::cached(f, speech, &voice_tracks(report), &p.dir));
    let found = match cached {
        // Heard before: only labelled again, with the voices as they are now (cheap).
        Some((f, embeddings)) => {
            jobs.skip(&p.dir, StageName::Speakers, log)?;
            let again = speakers::label(&f, &embeddings, &opts.voices);
            if again != f {
                write_speakers(p, &again)?;
            }
            again
        }
        None => jobs.stage(&p.dir, StageName::Speakers, log, |_| {
            // The user's pins cut the windows and outlive the hearing they were made on.
            let pins = previous
                .as_ref()
                .map(|f| f.pins.clone())
                .unwrap_or_default();
            let mut f = speakers::find(
                &speakers::Job {
                    file: &p.file,
                    speech,
                    voice: &voice_tracks(report),
                    voices: &opts.voices,
                    pins: &pins,
                    dir: &p.dir,
                    cache_dir: &p.cache_dir,
                },
                &mut |track, at| steps(steps::step(report, StageName::Speakers, track, at)),
            )?;
            // Heard again: the voice names the old labels knew still are the voices', not
            // names for whatever cluster now sits there.
            if let Some(old) = &previous {
                speakers::remember_known(&mut f, old);
            }
            write_speakers(p, &f)?;
            Ok(f)
        })?,
    };
    log(&describe(&found));
    Ok(found)
}

/// The note a `labels` stage left off carries in `jobs.json`.
pub const LABELS_OFF: &str = "off";

/// The silent parts labelled ([`labels`]), reused when the list, the speech, the busy track and the
/// model are what they were, unless forced. Off (the default), the stage is recorded done with the
/// note [`LABELS_OFF`] and nothing is read. Not a `StageState` of its own: an older build keeps a
/// stage name it does not know, but cannot parse a state it does not know, and would then read the
/// whole `jobs.json` as every stage pending.
pub(super) fn labels_stage(
    st: Stage,
    speech: &Speech,
    opts: &PipelineOptions,
    log: &mut dyn FnMut(&str),
) -> Result<Option<Labels>> {
    let Stage {
        p, jobs, report, ..
    } = st;
    if !opts.labels.on {
        let off = Some(LABELS_OFF.to_string());
        jobs.set(&p.dir, StageName::Labels, StageState::Done, None, off)?;
        return Ok(None);
    }
    let regions = speech.all_regions();
    let job = labels::Job {
        file: &p.file,
        dir: &p.dir,
        cache_dir: &p.cache_dir,
        report,
        speech: &regions,
        settings: &opts.labels,
        device: opts.device,
        force: opts.force,
    };
    let l = match labels::reused(&job) {
        Some(l) => {
            jobs.skip(&p.dir, StageName::Labels, log)?;
            l
        }
        None => jobs.stage(&p.dir, StageName::Labels, log, |log| labels::run(&job, log))?,
    };
    log(&labels::summary(&l));
    Ok(Some(l))
}

/// Whisper over the speech regions, resumed or reused when it can be.
pub(super) fn transcribe_stage(
    st: Stage,
    speech: &Speech,
    opts: &PipelineOptions,
    again: bool,
    log: &mut dyn FnMut(&str),
    transcribed: &mut dyn FnMut(f64),
    between: &mut dyn FnMut(),
) -> Result<Transcript> {
    let Stage {
        p,
        jobs,
        report,
        steps,
    } = st;
    let t_opts = TranscribeOptions {
        model: opts.model,
        device: opts.device,
        languages: opts.languages.clone(),
        force: opts.force,
        again,
        words: opts.words.clone(),
    };
    let src: Vec<Source> = speech
        .tracks
        .iter()
        .map(|t| Source {
            track: t.track,
            regions: t.regions.clone(),
        })
        .collect();
    jobs.stage(&p.dir, StageName::Transcribe, log, |log| {
        transcribe::transcribe(
            &transcribe::Job {
                file: &p.file,
                sources: &src,
                total: report.duration,
                project: &p.dir,
                cache_dir: &p.cache_dir,
                opts: &t_opts,
            },
            log,
            transcribe::Reports {
                reach: &mut |track, at| {
                    steps(steps::step(report, StageName::Transcribe, track, at))
                },
                transcribed: &mut *transcribed,
                between: &mut *between,
            },
        )
    })
}

/// The speech tracks whose role is a single voice (a mic, a Discord), not a mix.
fn voice_tracks(report: &TrackReport) -> Vec<usize> {
    report
        .logical
        .iter()
        .filter(|l| l.role == Role::Voice)
        .map(|l| l.members[0])
        .collect()
}
