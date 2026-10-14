use junctur_core::pipeline::{StageName, Step, Steps};

use super::run::Second;
use super::*;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn a_run_reports_until_it_ends_and_is_not_started_twice() {
    let d = std::env::temp_dir().join(format!("junctur-app-analysis-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // Not a video: the tracks stage fails (ffprobe refuses it, or is not there at all).
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let a = Analysis::with_cache(d.join("cache"));
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let notify: Notify = Arc::new(move |p: &Progress, _| {
        let _ = lock(&tx).send(p.clone());
    });
    // Hold the first lane so the run waits, queued, and a second request finds it.
    let held = lock(&a.first);
    let first = a.start(&file, notify.clone()).unwrap();
    assert_eq!(first.phase, Phase::Queued);
    assert!(a.busy(&file));
    let again = a.start(&file, notify.clone()).unwrap();
    assert_eq!(again.phase, Phase::Queued);
    assert!(
        a.reanalyse(&file, Again::KeepList, notify).is_err(),
        "already running"
    );
    drop(held);
    let mut phases = Vec::new();
    loop {
        let p = rx.recv_timeout(Duration::from_secs(60)).unwrap();
        phases.push(p.phase);
        if !p.phase.busy() {
            assert_eq!(p.phase, Phase::Failed);
            assert!(p.error.is_some());
            assert_eq!(p.file, key(&file).display().to_string());
            let tracks = p.project.unwrap().stages[0].clone();
            assert_eq!(tracks.name, "tracks");
            assert_eq!(tracks.state, pipeline::StageState::Failed);
            break;
        }
    }
    assert_eq!(phases.iter().filter(|p| **p == Phase::Queued).count(), 1);
    assert!(phases.contains(&Phase::Running));
    assert!(!a.busy(&file));
    let _ = std::fs::remove_dir_all(&d);
}

/// Lets the fake transcriptions below end.
static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A pipeline whose first half is instant and whose transcription lands its first piece, then
/// runs chunk after chunk (each a few ms) until [`RELEASE`]; recordings named `fail*` then
/// fail.
fn fake_halves<'a>(
    p: &'a Project,
    _: &'a PipelineOptions,
    _: &mut dyn FnMut(&str),
    _: Steps,
) -> Result<Second<'a>, String> {
    Ok(Box::new(move |_, _, transcribed, between| {
        transcribed(300.0);
        while !RELEASE.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
            between();
        }
        let name = p.file.file_name().unwrap().to_string_lossy();
        if name.starts_with("fail") {
            Err("Whisper failed".into())
        } else {
            Ok(())
        }
    }))
}

fn wait_for(what: &str, ok: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !ok() {
        assert!(std::time::Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_first_half_never_waits_behind_a_transcription() {
    let d = std::env::temp_dir().join(format!("junctur-app-lanes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (one, two) = (d.join("one.mp4"), d.join("fail-two.mp4"));
    for f in [&one, &two] {
        std::fs::write(f, b"not a video").unwrap();
    }
    let mut a = Analysis::with_cache(d.join("cache"));
    a.halves = fake_halves;
    let seen = Arc::new(Mutex::new(Vec::<Phase>::new()));
    let noted = seen.clone();
    let file = key(&two).display().to_string();
    let notify: Notify = Arc::new(move |p: &Progress, _| {
        if p.file == file {
            lock(&noted).push(p.phase);
        }
    });
    a.start(&one, notify.clone()).unwrap();
    wait_for("one transcribing", || {
        a.progress(&one)
            .is_some_and(|p| p.transcribed == Some(300.0))
    });
    assert!(a.editable(&one), "edited while it transcribes");
    // The second recording's first half runs at once; its transcription waits, editable.
    a.start(&two, notify).unwrap();
    wait_for("two waiting", || {
        a.progress(&two).is_some_and(|p| p.phase == Phase::Waiting)
    });
    let p = a.progress(&two).unwrap();
    assert!(p.listed && a.editable(&two));
    assert_eq!(a.progress(&one).unwrap().phase, Phase::Running);
    RELEASE.store(true, std::sync::atomic::Ordering::Relaxed);
    wait_for("both ended", || !a.busy(&one) && !a.busy(&two));
    let one = a.progress(&one).unwrap();
    assert_eq!((one.phase, one.transcribed), (Phase::Done, None));
    // Failed part way: the piece it finished stays the editor's.
    let two = a.progress(&two).unwrap();
    assert_eq!((two.phase, two.transcribed), (Phase::Failed, Some(300.0)));
    let phases = lock(&seen).clone();
    let at = |x| phases.iter().position(|p| *p == x).unwrap();
    assert!(at(Phase::Queued) < at(Phase::Waiting));
    assert!(phases[at(Phase::Waiting)..].contains(&Phase::Running));
    let _ = std::fs::remove_dir_all(&d);
}

/// Lets the transcriptions of [`counted_halves`] end; how many chunks `one*` has done.
static END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CHUNKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// [`fake_halves`] with its own end, counting the chunks of recordings named `one*`.
fn counted_halves<'a>(
    p: &'a Project,
    _: &'a PipelineOptions,
    _: &mut dyn FnMut(&str),
    _: Steps,
) -> Result<Second<'a>, String> {
    let one = p
        .file
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("one");
    Ok(Box::new(move |_, _, transcribed, between| {
        transcribed(300.0);
        while !END.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
            if one {
                CHUNKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            between();
        }
        Ok(())
    }))
}

#[test]
fn a_pause_holds_the_transcribing_recording_and_the_ones_waiting() {
    use std::sync::atomic::Ordering::SeqCst;
    let d = std::env::temp_dir().join(format!("junctur-app-pause-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (one, two) = (d.join("one.mp4"), d.join("two.mp4"));
    for f in [&one, &two] {
        std::fs::write(f, b"not a video").unwrap();
    }
    let mut a = Analysis::with_cache(d.join("cache"));
    a.halves = counted_halves;
    let notify: Notify = Arc::new(|_: &Progress, _| {});
    let quiet = |_: &Transcription| {};
    assert!(
        !a.pause(true, &quiet),
        "nothing transcribing: nothing to pause"
    );
    a.start(&one, notify.clone()).unwrap();
    wait_for("one transcribing", || CHUNKS.load(SeqCst) > 0);
    a.start(&two, notify).unwrap();
    wait_for("two waiting", || {
        a.progress(&two).is_some_and(|p| p.phase == Phase::Waiting)
    });
    let files = |t: &Transcription| t.jobs.iter().map(|p| p.file.clone()).collect::<Vec<_>>();
    let (k1, k2) = (
        key(&one).display().to_string(),
        key(&two).display().to_string(),
    );
    assert_eq!(files(&a.transcription()), [k1.clone(), k2.clone()]);

    assert!(a.pause(true, &quiet));
    wait_for("one stopped", || a.transcription().stopped);
    let at = CHUNKS.load(SeqCst);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(CHUNKS.load(SeqCst), at, "no chunk while paused");
    let t = a.transcription();
    assert!(t.paused);
    assert_eq!(files(&t), [k1, k2]);
    // Still a busy run, editable, its pieces kept; the one behind it still waits.
    let p = a.progress(&one).unwrap();
    assert_eq!((p.phase, p.transcribed), (Phase::Running, Some(300.0)));
    assert!(a.editable(&one));
    assert_eq!(a.progress(&two).unwrap().phase, Phase::Waiting);

    assert!(a.pause(false, &quiet));
    wait_for("one going again", || CHUNKS.load(SeqCst) > at);
    assert!(!a.transcription().stopped);
    END.store(true, SeqCst);
    wait_for("both ended", || !a.busy(&one) && !a.busy(&two));
    let t = a.transcription();
    assert!(t.jobs.is_empty() && !t.paused);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_failed_open_never_marks_a_running_recording_failed() {
    // No such folder: opening the project would fail, and must not be needed.
    let a = Analysis::with_cache(std::env::temp_dir().join("junctur-app-failed-unused"));
    let k = key(Path::new("Z:/nowhere/rec.mp4"));
    let running = Progress {
        file: k.display().to_string(),
        phase: Phase::Running,
        line: None,
        error: None,
        project: None,
        steps: Vec::new(),
        listed: true,
        transcribed: Some(300.0),
    };
    lock(&a.runs).insert(k.clone(), running);
    let p = a.failed(&k, "could not open".into());
    assert_eq!((p.phase, p.error), (Phase::Running, None));
    // Asked to start again, the run going on is the answer, without opening anything.
    let p = a.start(&k, Arc::new(|_: &Progress, _| {})).unwrap();
    assert_eq!(p.phase, Phase::Running);
    assert!(a.busy(&k));
}

#[test]
fn edits_wait_only_until_the_runs_list_is_written() {
    let a = Analysis::with_cache(std::env::temp_dir().join("junctur-app-editable-unused"));
    let k = PathBuf::from("rec.mp4");
    assert!(a.editable(&k), "no run");
    let run = |phase, listed| Progress {
        file: "rec.mp4".into(),
        phase,
        line: None,
        error: None,
        project: None,
        steps: Vec::new(),
        listed,
        transcribed: None,
    };
    for (phase, listed, editable) in [
        (Phase::Queued, false, false),
        (Phase::Running, false, false),
        (Phase::Waiting, true, true),
        (Phase::Running, true, true),
        (Phase::Done, true, true),
        (Phase::Failed, false, true),
    ] {
        lock(&a.runs).insert(key(&k), run(phase, listed));
        assert_eq!(a.editable(&k), editable, "{phase:?}, listed {listed}");
    }
}

/// A pipeline whose transcription panics for recordings named `panic*` and ends at once for
/// any other.
fn panicking_halves<'a>(
    p: &'a Project,
    _: &'a PipelineOptions,
    _: &mut dyn FnMut(&str),
    _: Steps,
) -> Result<Second<'a>, String> {
    Ok(Box::new(move |_, _, transcribed, _| {
        transcribed(300.0);
        if p.file
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("panic")
        {
            panic!("Whisper fell over");
        }
        Ok(())
    }))
}

#[test]
fn a_run_that_panics_ends_failed_and_frees_both_lanes() {
    let d = std::env::temp_dir().join(format!("junctur-app-panic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (bad, good) = (d.join("panic.mp4"), d.join("good.mp4"));
    for f in [&bad, &good] {
        std::fs::write(f, b"not a video").unwrap();
    }
    let mut a = Analysis::with_cache(d.join("cache"));
    a.halves = panicking_halves;
    let notify: Notify = Arc::new(|_: &Progress, _| {});
    a.start(&bad, notify.clone()).unwrap();
    wait_for("the panicked run ended", || !a.busy(&bad));
    let p = a.progress(&bad).unwrap();
    assert_eq!(p.phase, Phase::Failed);
    assert!(
        p.error.as_deref().unwrap().contains("Whisper fell over"),
        "{:?}",
        p.error
    );
    // Its pieces kept, as any run failing part way; editable and analysable again.
    assert_eq!(p.transcribed, Some(300.0));
    assert!(a.editable(&bad) && a.edit(&bad).is_some());
    assert!(a.transcription().jobs.is_empty());
    // Both lanes were handed on: another recording runs through to the end.
    a.start(&good, notify).unwrap();
    wait_for("the next run ended", || !a.busy(&good));
    assert_eq!(a.progress(&good).unwrap().phase, Phase::Done);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_run_waits_for_an_edit_of_its_recording_and_refuses_new_ones() {
    let d = std::env::temp_dir().join(format!("junctur-app-editing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let mut a = Analysis::with_cache(d.join("cache"));
    a.halves = panicking_halves;
    let notify: Notify = Arc::new(|_: &Progress, _| {});
    // An enrollment passed the check and is working when a re-analysis is asked for.
    let enrolling = a.edit(&file).expect("no run: editable");
    a.start(&file, notify).unwrap();
    assert!(a.edit(&file).is_none(), "queued: no new edit");
    std::thread::sleep(Duration::from_millis(50));
    let p = a.progress(&file).unwrap();
    assert_eq!(p.phase, Phase::Queued, "the run waits for the edit");
    drop(enrolling);
    wait_for("the run ended", || !a.busy(&file));
    assert_eq!(a.progress(&file).unwrap().phase, Phase::Done);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn steps_are_sent_when_a_track_moves_a_whole_percent_or_a_stage() {
    let a = Analysis::with_cache(std::env::temp_dir().join("junctur-app-steps-unused"));
    let k = PathBuf::from("rec.mp4");
    lock(&a.runs).insert(
        k.clone(),
        Progress {
            file: "rec.mp4".into(),
            phase: Phase::Running,
            line: None,
            error: None,
            project: None,
            steps: Vec::new(),
            listed: false,
            transcribed: None,
        },
    );
    let sent = Mutex::new(Vec::<Vec<Step>>::new());
    let notify = |p: &Progress, c| {
        assert_eq!(c, Change::Steps);
        lock(&sent).push(p.steps.clone());
    };
    let step = |stage, track, at| Step {
        stage,
        track,
        at,
        of: 1000.0,
    };
    a.step(&k, step(StageName::Speech, 1, 0.0), &notify);
    a.step(&k, step(StageName::Speech, 0, 0.0), &notify);
    // Under a whole percent: kept, not sent.
    a.step(&k, step(StageName::Speech, 0, 9.0), &notify);
    a.step(&k, step(StageName::Speech, 0, 10.0), &notify);
    // A new stage at the same percentage is sent.
    a.step(&k, step(StageName::Transcribe, 0, 10.0), &notify);
    let sent = lock(&sent).clone();
    assert_eq!(sent.len(), 4);
    let last = sent.last().unwrap();
    assert_eq!(last.iter().map(|s| s.track).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(last[0], step(StageName::Transcribe, 0, 10.0));
    // 1 % (10 s) is the third event.
    assert_eq!(sent[2][0].at, 10.0);
    // A file with no run takes nothing.
    a.step(
        &PathBuf::from("other.mp4"),
        step(StageName::Speech, 0, 500.0),
        &|_, _| panic!("no run"),
    );
}

#[test]
fn steps_from_two_threads_reach_notify_in_the_order_taken() {
    let a = Analysis::with_cache(std::env::temp_dir().join("junctur-app-order-unused"));
    let k = PathBuf::from("rec.mp4");
    lock(&a.runs).insert(
        k.clone(),
        Progress {
            file: "rec.mp4".into(),
            phase: Phase::Running,
            line: None,
            error: None,
            project: None,
            steps: Vec::new(),
            listed: false,
            transcribed: None,
        },
    );
    let sent = Mutex::new(Vec::<Vec<Step>>::new());
    let notify = |p: &Progress, _| {
        // Widen the gap between snapshot and send, where an unordered send would overtake.
        std::thread::yield_now();
        lock(&sent).push(p.steps.clone());
    };
    std::thread::scope(|scope| {
        for track in 0..2 {
            let (a, k, notify) = (&a, &k, &notify);
            scope.spawn(move || {
                for at in 0..=1000 {
                    let s = Step {
                        stage: StageName::Speech,
                        track,
                        at: at as f64,
                        of: 1000.0,
                    };
                    a.step(k, s, notify);
                }
            });
        }
    });
    let sent = lock(&sent).clone();
    for track in 0..2 {
        let ats: Vec<f64> = sent
            .iter()
            .filter_map(|v| v.iter().find(|s| s.track == track).map(|s| s.at))
            .collect();
        assert!(
            ats.windows(2).all(|w| w[0] <= w[1]),
            "track {track} went back"
        );
        assert_eq!(ats.last(), Some(&1000.0));
    }
}

/// What each run of the test below was told about the user's list.
static KEPT: Mutex<Vec<KeepList>> = Mutex::new(Vec::new());

/// A pipeline that only notes whether the run keeps the user's list.
fn noting_halves<'a>(
    _: &'a Project,
    o: &'a PipelineOptions,
    _: &mut dyn FnMut(&str),
    _: Steps,
) -> Result<Second<'a>, String> {
    lock(&KEPT).push(o.keep_list);
    Ok(Box::new(|_, _, _, _| Ok(())))
}

#[test]
fn reanalyse_keeps_the_users_list_and_a_fresh_cut_asks_first() {
    let d = std::env::temp_dir().join(format!("junctur-app-reanalyse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let mut a = Analysis::with_cache(d.join("cache"));
    a.halves = noting_halves;
    let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
    let ended = |a: &Analysis| wait_for("the run ends", || !a.busy(&file));
    let quiet = || -> Notify { Arc::new(|_: &Progress, _| {}) };
    std::fs::write(p.edit_path(), "junctur-edit 1\nkeep x\n").unwrap();
    // Re-analyse on a list the user edited starts at once and keeps it.
    let r = a.reanalyse(&file, Again::KeepList, quiet()).unwrap();
    assert!(!r.hand_edited && r.progress.is_some());
    ended(&a);
    assert_eq!(*lock(&KEPT), [KeepList::Whole]);
    // A fresh cut over it is asked first: nothing started.
    let r = a.reanalyse(&file, Again::Recut, quiet()).unwrap();
    assert!(r.hand_edited && r.progress.is_none());
    assert!(!a.busy(&file));
    // Told to replace it: re-cut.
    let r = a.reanalyse(&file, Again::RecutOverEdits, quiet()).unwrap();
    assert!(!r.hand_edited && r.progress.is_some());
    ended(&a);
    assert_eq!(*lock(&KEPT), [KeepList::Whole, KeepList::No]);
    // Nothing of the user's to lose: a fresh cut starts without asking.
    std::fs::remove_file(p.edit_path()).unwrap();
    let r = a.reanalyse(&file, Again::Recut, quiet()).unwrap();
    assert!(!r.hand_edited && r.progress.is_some());
    ended(&a);
    assert_eq!(*lock(&KEPT), [KeepList::Whole, KeepList::No, KeepList::No]);
    let _ = std::fs::remove_dir_all(&d);
}

static ASKED: Mutex<Vec<(KeepList, bool, bool)>> = Mutex::new(Vec::new());
static HOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A first half noting what the run was asked to do, held while `HOLD` is set.
fn asked_halves<'a>(
    _: &'a Project,
    o: &'a PipelineOptions,
    _: &mut dyn FnMut(&str),
    _: Steps,
) -> Result<Second<'a>, String> {
    lock(&ASKED).push((o.keep_list, o.rehear, o.retranscribe));
    while HOLD.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(Box::new(|_, _, _, _| Ok(())))
}

#[test]
fn correct_hears_the_speakers_and_the_words_again_keeping_the_list() {
    // Correct is Re-analyse with speakers and words heard again; a second one is refused.
    let d = std::env::temp_dir().join(format!("junctur-app-correct-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let mut a = Analysis::with_cache(d.join("cache"));
    a.halves = asked_halves;
    let quiet = || -> Notify { Arc::new(|_: &Progress, _| {}) };
    HOLD.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = a.correct(&file, quiet()).unwrap();
    assert!(r.phase.busy());
    wait_for("the run asks", || !lock(&ASKED).is_empty());
    assert!(a.correct(&file, quiet()).is_err(), "already being analysed");
    HOLD.store(false, std::sync::atomic::Ordering::SeqCst);
    wait_for("the run ends", || !a.busy(&file));
    // A plain Re-analyse hears nothing again that answers the settings.
    a.reanalyse(&file, Again::KeepList, quiet()).unwrap();
    wait_for("the run ends", || !a.busy(&file));
    assert_eq!(
        *lock(&ASKED),
        [
            (KeepList::Whole, true, true),
            (KeepList::Whole, false, false)
        ]
    );
    let _ = std::fs::remove_dir_all(&d);
}
