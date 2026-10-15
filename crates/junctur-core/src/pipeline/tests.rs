use super::again::{AGAIN_FILE, BEFORE_FILE, SET_ASIDE_FILE, WITHIN_FILE};
use super::editlist::{back_up, is_generated, rewrite, Draft, Rewrite};
use super::speaker_files::{set_aside, write_speakers};
use super::*;
use crate::edit::Speakers;

/// The code of every `.rs` file under `src/<rel>` (a folder or one file), without its
/// `mod tests` or a `tests.rs` beside it, CRLF read as LF.
fn code_under(rel: &str) -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut todo = vec![root.join(rel)];
    let mut out = Vec::new();
    while let Some(at) = todo.pop() {
        if at.is_dir() {
            todo.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
        } else if at.extension().is_some_and(|x| x == "rs")
            && at.file_name().is_some_and(|n| n != "tests.rs")
        {
            let src = std::fs::read_to_string(&at).unwrap().replace("\r\n", "\n");
            let code = match src.find("#[cfg(test)]\nmod tests") {
                Some(k) => src[..k].to_string(),
                None => src,
            };
            let name = at.strip_prefix(&root).unwrap().display().to_string();
            out.push((name, code));
        }
    }
    out
}

/// Tracks, speech and speakers open every model on the CPU (CAM++ on CUDA makes a speaker of
/// every sentence): their code may open a model only through [`runtime::open_on_cpu`] and name
/// no device; this file hands the run's device to transcription only.
#[test]
fn tracks_speech_and_speakers_open_their_models_on_the_cpu_only() {
    let mut stages = Vec::new();
    for rel in [
        "tracks",
        "diarize",
        "vad.rs",
        "pipeline/speech.rs",
        "pipeline/speakers",
    ] {
        stages.extend(code_under(rel));
    }
    assert!(stages.len() >= 9, "{}", stages.len());
    let mut opened = 0;
    for (name, code) in &stages {
        for banned in [
            "DevicePref",
            "Device::",
            "runtime::open(",
            "runtime::open_with_threads",
            "Session::builder",
            "ort::ep",
        ] {
            assert!(!code.contains(banned), "{name} has {banned}");
        }
        opened += code.matches("runtime::open_on_cpu(").count();
    }
    // The VAD's, the segmenter's and the embedder's.
    assert!(opened >= 3, "{opened}");
    // The run's own code: every file of this module but the stages read above, so a file the run
    // gains is checked without being named here.
    let this = code_under("pipeline")
        .into_iter()
        .filter(|(name, _)| {
            let parts: Vec<_> = Path::new(name).components().collect();
            parts.len() == 2 && !name.ends_with("speech.rs")
        })
        .map(|(_, code)| code)
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    for banned in [
        "runtime::open",
        "Session::builder",
        "ort::ep",
        "DevicePref::Auto",
    ] {
        assert!(!this.contains(banned), "the run's code has {banned}");
    }
    // Every `device` field this file writes is transcription's (its own options' declaration
    // aside), and the run's device is only ever written into one.
    let mut fields = 0;
    for (k, _) in this.match_indices("device:") {
        let before = &this[..k];
        let opened = &before[..before.rfind(" {").expect("a struct")];
        let owner = opened.rsplit([' ', '(', '=']).next().unwrap();
        if !opened.ends_with("pub struct PipelineOptions") {
            assert_eq!(owner, "TranscribeOptions", "a device field at byte {k}");
            fields += 1;
        }
    }
    assert!(fields > 0);
    for (k, _) in this.match_indices(".device") {
        let line = this[..k].rsplit('\n').next().unwrap();
        assert!(line.trim_start().starts_with("device: "), "{line}.device");
    }
}

fn list(settings: CutSettings, keep_end: f64) -> String {
    EditList {
        source: PathBuf::from("in.mp4"),
        duration: 10.0,
        settings,
        keep: vec![edit::Range {
            start: 0.0,
            end: keep_end,
        }],
        speakers: Default::default(),
        captions: Default::default(),
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![],
    }
    .to_text()
}

/// What [`cut`] does with the list it replaces.
fn back_up_hand_edit(
    dir: &Path,
    old: &str,
    regenerate: impl Fn(&CutSettings) -> String,
) -> Result<()> {
    if is_generated(old, |s| vec![regenerate(s)]) {
        return Ok(());
    }
    back_up(dir, old)
}

#[test]
fn only_hand_edits_are_backed_up_and_never_over_each_other() {
    let dir = std::env::temp_dir().join(format!("junctur-bak-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // What the analysis makes: the kept end follows the padding.
    let regenerate = |s: &CutSettings| list(*s, 5.0 + s.pad_after);
    let wide = CutSettings {
        pad_after: 1.0,
        ..Default::default()
    };
    let bak = |name: &str| std::fs::read_to_string(dir.join(name)).ok();

    // A generated list replaced by a re-cut: nothing to keep.
    back_up_hand_edit(&dir, &regenerate(&CutSettings::default()), regenerate).unwrap();
    assert_eq!(bak("edit.junctur.bak"), None);
    // Two hand edits, each replaced by a re-cut: both kept.
    let hand1 = list(CutSettings::default(), 2.0);
    back_up_hand_edit(&dir, &hand1, regenerate).unwrap();
    back_up_hand_edit(&dir, &regenerate(&wide), regenerate).unwrap();
    let hand2 = list(wide, 3.0);
    back_up_hand_edit(&dir, &hand2, regenerate).unwrap();
    assert_eq!(bak("edit.junctur.bak"), Some(hand1));
    assert_eq!(bak("edit.junctur.bak2"), Some(hand2));
    assert_eq!(bak("edit.junctur.bak3"), None);
    // An untouched app save the re-cut reproduces (a speaker renamed in the app, the stamp
    // aside) is Junctur's own; an app save with other ranges is the user's.
    let renamed = dir.join("renamed.junctur");
    let wide_list = edit::parse(&regenerate(&wide)).unwrap();
    edit::save(&renamed, &wide_list).unwrap();
    let stamped = std::fs::read_to_string(&renamed).unwrap();
    assert_ne!(stamped, regenerate(&wide));
    back_up_hand_edit(&dir, &stamped, regenerate).unwrap();
    assert_eq!(bak("edit.junctur.bak3"), None);
    edit::save(&renamed, &edit::parse(&list(wide, 4.0)).unwrap()).unwrap();
    let app_cut = std::fs::read_to_string(&renamed).unwrap();
    back_up_hand_edit(&dir, &app_cut, regenerate).unwrap();
    assert_eq!(bak("edit.junctur.bak3"), Some(app_cut));
    std::fs::remove_file(dir.join("edit.junctur.bak3")).unwrap();
    // A list that does not parse is the user's too.
    back_up_hand_edit(&dir, "junctur-edit 1\nkeep x\n", regenerate).unwrap();
    assert!(bak("edit.junctur.bak3").is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unreadable_speakers_file_is_set_aside_and_writes_leave_no_temporary() {
    let dir = std::env::temp_dir().join(format!("junctur-spk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = Project {
        file: PathBuf::from("in.mp4"),
        dir: dir.clone(),
        cache_dir: dir.clone(),
    };
    assert!(p.speakers_file().unwrap().is_none());
    std::fs::write(p.speakers_path(), "{ nope").unwrap();
    assert!(p.speakers_file().is_err());
    assert!(p.speakers().is_none());
    let kept = set_aside(&p.speakers_path()).unwrap();
    assert_eq!(kept, dir.join("speakers.json.unreadable"));
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "{ nope");
    assert!(!p.speakers_path().exists());
    let mut found = Found {
        people: vec![],
        heard: None,
        known: vec![],
        method: Method::None,
        tracks: vec![],
        turns: vec![],
        speech: String::new(),
        pins: vec![],
    };
    found.pin("Ana", &[(1.0, 2.0)]);
    write_speakers(&p, &found).unwrap();
    write_speakers(&p, &found).unwrap();
    assert_eq!(p.speakers(), Some(found));
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
    // A second unreadable file does not replace the first one set aside.
    std::fs::write(p.speakers_path(), "x").unwrap();
    assert_eq!(
        set_aside(&p.speakers_path()).unwrap(),
        dir.join("speakers.json.unreadable2")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_recut_without_found_speakers_keeps_the_ones_the_list_had() {
    let turn = |speaker| edit::Turn {
        start: speaker as f64,
        end: speaker as f64 + 1.0,
        speaker,
    };
    let mut had = Speakers::new(vec![turn(1), turn(2)], &[None, None], None);
    had.rename(2, "Ana");
    // `--no-speakers`, or a failed stage: nothing found, nothing erased.
    assert_eq!(speakers_for(None, Some(&had)), had);
    assert!(speakers_for(None, None).is_empty());
    // Found again: the rename carries over.
    let found = Found {
        people: vec![],
        heard: None,
        known: vec![],
        method: Method::Voices,
        tracks: vec![None, None],
        turns: vec![turn(2)],
        speech: String::new(),
        pins: vec![],
    };
    assert_eq!(speakers_for(Some(&found), Some(&had)).name(2), Some("Ana"));
    // Ana's stretch is speaker 1 now (an enrolled voice took the old numbering): the name
    // goes with what she says, not with the number.
    let renumbered = Found {
        turns: vec![edit::Turn {
            speaker: 1,
            ..turn(2)
        }],
        ..found.clone()
    };
    let s = speakers_for(Some(&renumbered), Some(&had));
    assert_eq!((s.name(1), s.name(2)), (Some("Ana"), Some("Speaker 2")));
    // A name an enrolled voice had (here: since forgotten) is never carried onto a cluster.
    let forgotten = Found {
        known: vec!["ANA".into()],
        ..found
    };
    assert_eq!(
        speakers_for(Some(&forgotten), Some(&had)).name(2),
        Some("Speaker 2")
    );
}

fn scratch_project(name: &str) -> (PathBuf, Project) {
    let d = std::env::temp_dir().join(format!("junctur-2d-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let file = d.join("rec.mp4");
    std::fs::write(&file, b"not a video").unwrap();
    let p = Project::open(&file, Some(d.join("cache")), None).unwrap();
    (d, p)
}

#[test]
fn a_rename_moves_the_pins_of_speakers_an_older_junctur_heard() {
    // Heard by an older Junctur, so not labelled again; the renamed person's pins must still
    // follow the name, or the next hearing would drop them as a name no voice has.
    let (d, p) = scratch_project("rename-stale");
    let mut found = Found {
        method: Method::Voices,
        tracks: vec![],
        people: vec![],
        turns: vec![],
        speech: String::new(),
        heard: Some(Heard {
            track: 0,
            windows: vec![],
            overlaps: vec![],
            key: "before 5r".into(),
            dim: 0,
            embeddings: String::new(),
        }),
        known: vec![],
        pins: vec![],
    };
    found.pin("Ana", &[(1.0, 2.0)]);
    found.pin("Bea", &[(3.0, 4.0)]);
    write_speakers(&p, &found).unwrap();
    assert!(labelled(&p, &[]).is_err(), "not labelled from old windows");
    let renamed = labelled_renamed(&p, &[], "ana", "Anna").unwrap();
    let names: Vec<&str> = renamed.pins.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Anna", "Bea"]);
    assert_eq!((renamed.turns, renamed.heard), (found.turns, found.heard));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn outdated_names_what_a_rerun_would_redo() {
    let (d, p) = scratch_project("outdated");
    let opts = |model| PipelineOptions {
        model,
        ..Default::default()
    };
    assert_eq!(
        outdated(&p, &opts(ModelPref::Auto)),
        vec!["There is no transcript yet."]
    );
    // Without a transcript asked for, nothing is missing.
    let none = PipelineOptions {
        no_transcript: true,
        ..Default::default()
    };
    assert!(outdated(&p, &none).is_empty());
    let made = |langs: &str| {
        format!(
            r#"{{"choice":{{"model":"large-v3-turbo","device":"cuda","note":null}},
                "languages":{langs},"segments":[],"audio_s":0,"elapsed_s":0}}"#
        )
    };
    std::fs::write(transcribe::path(&p.dir), made(r#"["es","en"]"#)).unwrap();
    // `auto` takes whatever model made it; the same model explicitly too.
    assert!(outdated(&p, &opts(ModelPref::Auto)).is_empty());
    assert!(outdated(&p, &opts(ModelPref::LargeV3Turbo)).is_empty());
    let why = outdated(&p, &opts(ModelPref::LargeV3));
    assert_eq!(
        why,
        vec!["Transcribed with large-v3-turbo; the settings ask for large-v3."]
    );
    std::fs::write(transcribe::path(&p.dir), made(r#"["en"]"#)).unwrap();
    let why = outdated(&p, &opts(ModelPref::Auto));
    assert_eq!(why, vec!["Transcribed in en; the settings ask for es+en."]);
    // Unreadable is not missing: a run would make it again, and the user hears why.
    std::fs::write(transcribe::path(&p.dir), "{ torn").unwrap();
    let why = outdated(&p, &opts(ModelPref::Auto));
    assert!(
        why[0].starts_with("The transcript could not be read"),
        "{why:?}"
    );
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_resumed_run_keeps_the_users_list_but_not_a_broken_one() {
    let (d, p) = scratch_project("keeplist");
    let fresh = || edit::parse(&list(CutSettings::default(), 5.0)).unwrap();
    let draft_over = |old: &str| Draft {
        list: fresh(),
        old: Some(old.to_string()),
        users: true,
    };
    let mut log = |_: &str| {};
    // The user cut while it transcribed, then the app closed: resumed, their list stays.
    let mine = list(CutSettings::default(), 2.0);
    std::fs::write(p.edit_path(), &mine).unwrap();
    let kept = keep_or_write(&p, draft_over(&mine), None, KeepList::Edits, &mut log).unwrap();
    assert_eq!(kept.keep[0].end, 2.0);
    assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), mine);
    assert!(!p.dir.join("edit.junctur.bak").exists());
    // A re-analysis (not kept): backed up and re-cut.
    let recut = keep_or_write(&p, draft_over(&mine), None, KeepList::No, &mut log).unwrap();
    assert_eq!(recut.keep[0].end, 5.0);
    assert_eq!(
        std::fs::read_to_string(p.dir.join("edit.junctur.bak")).unwrap(),
        mine
    );
    // A list that does not parse is never kept, or the recording would never get one.
    std::fs::write(p.edit_path(), "junctur-edit 1\nkeep x\n").unwrap();
    let recut = keep_or_write(
        &p,
        draft_over("junctur-edit 1\nkeep x\n"),
        None,
        KeepList::Edits,
        &mut log,
    );
    assert_eq!(recut.unwrap().keep[0].end, 5.0);
    assert!(p.dir.join("edit.junctur.bak2").exists());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_reanalysis_keeps_the_users_cut_names_and_fixes_and_takes_the_new_speakers() {
    // Re-analyse keeps the list the user edited and puts on top of it only the speakers
    // the run matched against the remembered voices.
    let (d, p) = scratch_project("reanalyse-keeps");
    let turn = |start: f64, speaker| edit::Turn {
        start,
        end: start + 1.0,
        speaker,
    };
    // The user's list, saved by the app: their own cut, speaker 2 named Ana in a colour of
    // their own, a word fixed and caption settings of their own.
    let mut mine = edit::parse(&list(CutSettings::default(), 2.0)).unwrap();
    mine.keep.push(edit::Range {
        start: 3.5,
        end: 4.5,
    });
    mine.speakers = Speakers::new(vec![turn(1.0, 1), turn(4.0, 2)], &[None, None], None);
    mine.speakers.rename(2, "Ana");
    mine.speakers.recolor(2, "#123456");
    edit::set_fix(&mut mine.fixes, 0, 4.25, 0, "anna", "Ana").unwrap();
    mine.captions = Some(edit::Captions {
        chars: 21,
        ..Default::default()
    });
    edit::save(&p.edit_path(), &mine).unwrap();
    let saved = std::fs::read_to_string(p.edit_path()).unwrap();
    // The run's own cut, which a fresh cut would write.
    let fresh = || edit::parse(&list(CutSettings::default(), 5.0)).unwrap();
    let draft_over = |old: &str| Draft {
        list: fresh(),
        old: Some(old.to_string()),
        users: true,
    };
    // Matched again: Ana's stretch is speaker 1 now (a remembered voice took the old
    // numbering), and a third stretch was found.
    let found = Found {
        people: vec![],
        heard: None,
        known: vec![],
        method: Method::Voices,
        tracks: vec![None, None],
        turns: vec![turn(4.0, 1), turn(1.0, 2), turn(7.0, 2)],
        speech: String::new(),
        pins: vec![],
    };
    let mut log = |_: &str| {};
    let kept = keep_or_write(
        &p,
        draft_over(&saved),
        Some(&found),
        KeepList::Whole,
        &mut log,
    )
    .unwrap();
    assert_eq!(kept.keep, mine.keep, "the user's cut, not the fresh one");
    assert_eq!(kept.fixes, mine.fixes);
    assert_eq!(kept.captions, mine.captions);
    assert_eq!(kept.settings, mine.settings);
    assert_eq!(
        kept.speakers.name(1),
        Some("Ana"),
        "the name follows her stretch"
    );
    assert_eq!(kept.speakers.color(1), Some("#123456"));
    assert_eq!(kept.speakers.name(2), Some("Speaker 2"));
    let starts: Vec<(f64, u32)> = kept
        .speakers
        .turns
        .iter()
        .map(|t| (t.start, t.speaker))
        .collect();
    assert_eq!(starts, [(1.0, 2), (4.0, 1), (7.0, 2)]);
    // Written as the app saves, and no backup: nothing of the user's was replaced.
    let text = std::fs::read_to_string(p.edit_path()).unwrap();
    assert_ne!(edit::unstamped(&text), text, "stamped");
    assert_eq!(edit::parse(edit::unstamped(&text)).unwrap(), kept);
    assert!(!p.dir.join("edit.junctur.bak").exists());
    // Run again with nothing new: the same list, untouched.
    let again = keep_or_write(
        &p,
        draft_over(&text),
        Some(&found),
        KeepList::Whole,
        &mut log,
    )
    .unwrap();
    assert_eq!(again, kept);
    assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), text);
    // No speakers this time (the stage failed): the list keeps the ones it has.
    let none = keep_or_write(&p, draft_over(&text), None, KeepList::Whole, &mut log).unwrap();
    assert_eq!(none, kept);
    // A list edited outside Junctur (a comment of the user's) is left whole, speakers and all,
    // rather than rewritten without the comment.
    let by_hand = format!("# my notes\n{}", mine.to_text());
    std::fs::write(p.edit_path(), &by_hand).unwrap();
    let left = keep_or_write(
        &p,
        draft_over(&by_hand),
        Some(&found),
        KeepList::Whole,
        &mut log,
    )
    .unwrap();
    assert_eq!(left.speakers, mine.speakers);
    assert_eq!(left.keep, mine.keep);
    assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), by_hand);
    assert!(!p.dir.join("edit.junctur.bak").exists());
    // A list that does not parse is left as it is too, and the stage says so: re-cutting is
    // only "Start over with a fresh cut", which asks first.
    let broken = "junctur-edit 1\nkeep x\n";
    std::fs::write(p.edit_path(), broken).unwrap();
    let err = keep_or_write(
        &p,
        draft_over(broken),
        Some(&found),
        KeepList::Whole,
        &mut log,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("left as it is"), "{err}");
    assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), broken);
    assert!(!p.dir.join("edit.junctur.bak").exists());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_list_saved_after_the_draft_is_backed_up_not_lost() {
    // The draft reads the list outside the writer lock; a save landing before the
    // write is decided on again under it.
    let (d, p) = scratch_project("recheck");
    // Junctur's own list, as an untouched app save (so the user's save below is no backup).
    let generated = list(CutSettings::default(), 5.0);
    edit::save(&p.edit_path(), &edit::parse(&generated).unwrap()).unwrap();
    let drafted_from = std::fs::read_to_string(p.edit_path()).unwrap();
    let drafted = || Draft {
        list: edit::parse(&generated).unwrap(),
        old: Some(drafted_from.clone()),
        users: false,
    };
    // The user cut meanwhile: a re-cut keeps their save as the backup, not the stale text.
    let saved = edit::parse(&list(CutSettings::default(), 2.0)).unwrap();
    edit::save(&p.edit_path(), &saved).unwrap();
    let theirs = std::fs::read_to_string(p.edit_path()).unwrap();
    let mut log = |_: &str| {};
    keep_or_write(&p, drafted(), None, KeepList::No, &mut log).unwrap();
    assert_eq!(
        std::fs::read_to_string(p.dir.join("edit.junctur.bak")).unwrap(),
        theirs
    );
    // Kept instead, when the run keeps the user's list (the editor's own backup of the plain
    // re-cut it saves over aside).
    edit::save(&p.edit_path(), &saved).unwrap();
    let kept = keep_or_write(&p, drafted(), None, KeepList::Whole, &mut log).unwrap();
    assert_eq!(kept.keep, saved.keep);
    assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), theirs);
    // A save of the very list the draft would write is nothing to back up.
    edit::save(&p.edit_path(), &edit::parse(&generated).unwrap()).unwrap();
    let backups = || {
        std::fs::read_dir(&p.dir)
            .unwrap()
            .filter(|e| {
                let name = e.as_ref().unwrap().file_name();
                name.to_string_lossy().contains(".bak")
            })
            .count()
    };
    let before = backups();
    keep_or_write(&p, drafted(), None, KeepList::No, &mut log).unwrap();
    assert_eq!(backups(), before);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_reanalysis_run_keeps_the_users_cut_and_names_and_takes_the_speakers_found() {
    // The whole first half a Re-analyse runs (`prepare` with `KeepList::Whole`),
    // every stage's result in the cache as an earlier analysis left it, so no media or model
    // is needed: tracks and speech reused, the speakers found again from the cache (two voice
    // tracks, one speaker each), the edit list kept.
    let (d, p) = scratch_project("prepare-keeps");
    let voice = |track: usize| tracks::LogicalTrack {
        members: vec![track],
        sameness: tracks::Sameness::Single,
        activity: crate::signal::Activity {
            mean_db: -30.0,
            peak_db: -10.0,
            near_silent_share: 0.0,
            silent: false,
        },
        speech: None,
        role: Role::Voice,
        confidence: 1.0,
        reasons: vec![],
    };
    let report = TrackReport {
        file: p.file.clone(),
        duration: 10.0,
        windows: vec![],
        window_seconds: 1.0,
        tracks: vec![],
        logical: vec![voice(0), voice(1)],
        speech_error: None,
        from_cache: false,
    };
    let entry = tracks::entry(&p.file, &tracks_options(&p, false)).unwrap();
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(&entry, serde_json::to_vec(&report).unwrap()).unwrap();
    let region = |start: f64, end: f64| crate::vad::Region { start, end };
    let speech = Speech {
        tracks: vec![
            speech::SpeechTrack {
                track: 0,
                regions: vec![region(1.0, 3.0)],
                share: 0.2,
            },
            speech::SpeechTrack {
                track: 1,
                regions: vec![region(5.0, 8.0)],
                share: 0.3,
            },
        ],
        seconds: 5.0,
    };
    std::fs::write(p.speech_path(), serde_json::to_vec(&speech).unwrap()).unwrap();
    std::fs::write(p.envelope_path(), b"").unwrap();
    let turn = |start: f64, end: f64, speaker| edit::Turn {
        start,
        end,
        speaker,
    };
    // The speakers as the stage finds them now: one per voice track, in track order.
    let found = Found {
        people: vec![None, None],
        heard: None,
        known: vec![],
        method: Method::Tracks,
        tracks: vec![Some(0), Some(1)],
        turns: vec![turn(1.0, 3.0, 1), turn(5.0, 8.0, 2)],
        speech: speakers::speech_key(&speech),
        pins: vec![],
    };
    write_speakers(&p, &found).unwrap();
    // The user's list, from when track 0's talk was speaker 2 (numbered otherwise then),
    // named Ana, with a cut of their own that the analysis would never make.
    let mut mine = edit::parse(&list(CutSettings::default(), 10.0)).unwrap();
    mine.source = p.file.clone();
    mine.keep = vec![edit::Range {
        start: 1.5,
        end: 2.5,
    }];
    mine.speakers = Speakers::new(
        vec![turn(1.0, 3.0, 2), turn(5.0, 8.0, 1)],
        &[Some(1), Some(0)],
        None,
    );
    mine.speakers.rename(2, "Ana");
    edit::set_fix(&mut mine.fixes, 0, 1.5, 0, "anna", "Ana").unwrap();
    edit::save(&p.edit_path(), &mine).unwrap();
    let opts = PipelineOptions {
        keep_list: KeepList::Whole,
        ..Default::default()
    };
    let mut log = |_: &str| {};
    let prepared = prepare(&p, &opts, &mut log, steps::none()).unwrap();
    let kept = p.edit().unwrap();
    assert_eq!(prepared.edit, kept);
    assert_eq!(kept.keep, mine.keep, "the user's cut");
    assert_eq!(kept.fixes, mine.fixes);
    assert_eq!(
        kept.speakers.name(1),
        Some("Ana"),
        "the name follows her talk"
    );
    assert_eq!(kept.speakers.name(2), Some("Speaker 2"));
    let turns: Vec<(f64, u32)> = kept
        .speakers
        .turns
        .iter()
        .map(|t| (t.start, t.speaker))
        .collect();
    assert_eq!(turns, [(1.0, 1), (5.0, 2)]);
    assert!(!p.dir.join("edit.junctur.bak").exists());
    // Not kept (a fresh cut): the analysis's own cut, the user's kept as a backup.
    let opts = PipelineOptions::default();
    let recut = prepare(&p, &opts, &mut log, steps::none()).unwrap().edit;
    assert_ne!(recut.keep, mine.keep);
    assert_eq!(recut.speakers.name(1), Some("Ana"));
    assert!(p.dir.join("edit.junctur.bak").exists());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn hand_edited_without_a_list_or_its_analysis() {
    let (d, p) = scratch_project("handedited");
    assert!(!hand_edited(&p).unwrap(), "no list: nothing to lose");
    std::fs::write(
        p.edit_path(),
        "junctur-edit 1
keep x
",
    )
    .unwrap();
    assert!(
        hand_edited(&p).unwrap(),
        "a list that does not parse is the user's"
    );
    std::fs::write(p.edit_path(), list(CutSettings::default(), 5.0)).unwrap();
    assert!(
        hand_edited(&p).unwrap(),
        "a list whose analysis is gone cannot be remade"
    );
    std::fs::remove_dir_all(&d).unwrap();
}

fn transcript_of(words: &[(f64, &str)]) -> Transcript {
    Transcript {
        choice: transcribe::Choice {
            model: "large-v3-turbo".into(),
            device: crate::runtime::Device::Cpu,
            note: None,
        },
        languages: vec![],
        segments: vec![transcribe::Segment {
            track: 0,
            start: 0.0,
            end: 10.0,
            lang: "en".into(),
            model: "large-v3-turbo".into(),
            words: words
                .iter()
                .map(|&(start, text)| transcribe::Word {
                    start,
                    end: start + 0.3,
                    text: text.into(),
                    p: 1.0,
                    p_min: None,
                })
                .collect(),
        }],
        audio_s: 10.0,
        elapsed_s: 1.0,
        resumed_chunks: 0,
    }
}

#[test]
fn a_retranscription_carries_the_fixes_onto_the_new_words_and_spreads_them() {
    // The Correct button's re-transcription. At its end the fixes, read against the
    // transcript it replaces, are moved onto the new words and spread to the words heard
    // alike, and the mark that asked for it goes.
    let (d, p) = scratch_project("carry");
    let fix = |start: f64, text: &str| edit::Fix {
        track: 0,
        start,
        nth: 0,
        through: None,
        text: text.into(),
    };
    let mut e = EditList {
        source: p.file.clone(),
        duration: 10.0,
        settings: CutSettings::default(),
        keep: vec![edit::Range {
            start: 0.0,
            end: 10.0,
        }],
        speakers: Default::default(),
        captions: Default::default(),
        fixes: vec![fix(1.0, "Vayne,"), fix(5.0, "Mell")],
        times: Vec::new(),
        windows: vec![],
    };
    std::fs::write(p.edit_path(), e.to_text()).unwrap();
    let old = transcript_of(&[(1.0, " Bane,"), (3.0, " Bane."), (5.0, " Mel")]);
    std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&old).unwrap()).unwrap();
    let lines = std::cell::RefCell::new(Vec::<String>::new());
    let mut log = |l: &str| lines.borrow_mut().push(l.to_string());
    let asked = PipelineOptions {
        retranscribe: true,
        ..Default::default()
    };
    assert!(hear_again(&p, &asked, &mut log).unwrap());
    let mark = p.dir.join(AGAIN_FILE);
    assert!(mark.is_file());
    // Asked and not started (the app closed before the first chunk): heard again, not settled
    // for the old transcript; once a partial is there, it is resumed as any run is.
    let plain = PipelineOptions::default();
    assert!(hear_again(&p, &plain, &mut log).unwrap());
    let partial = p.dir.join("transcript.partial.jsonl");
    std::fs::write(&partial, "{}").unwrap();
    assert!(!hear_again(&p, &plain, &mut log).unwrap());
    std::fs::remove_file(&partial).unwrap();
    // While it ran, the user kept the "Bane." at 3 as heard ("Undo those" on a spread): it
    // stays so, carried like any fix, and no spread reaches it.
    e.fixes.push(fix(3.0, "Bane."));
    std::fs::write(p.edit_path(), e.to_text()).unwrap();
    // The new transcript: "Bane," a little later, "Mel" now "and", a third "Bane" at 7. It is
    // in `transcript.json` already, as when the app stopped between Whisper's end and the carry:
    // the copy of the old one is what the fixes are read against.
    let new = transcript_of(&[
        (1.1, " Bane,"),
        (3.0, " Bane."),
        (5.0, " and"),
        (7.0, " bane"),
    ]);
    std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&new).unwrap()).unwrap();
    carry_fixes(&p, &new, &mut log);
    assert!(!mark.exists(), "the mark goes");
    assert!(!p.dir.join(BEFORE_FILE).exists(), "and the copy");
    // "Mel" is heard as "and" now: set aside, not lost, until the user has seen it.
    assert_eq!(
        fixes_set_aside(&p),
        [SetAside {
            track: 0,
            at: 5.0,
            heard: "Mel".into(),
            text: "Mell".into(),
            retimed: None,
            silent: false,
            note: None,
        }]
    );
    forget_fixes_set_aside(&p, &fixes_set_aside(&p)).unwrap();
    assert!(fixes_set_aside(&p).is_empty());
    forget_fixes_set_aside(&p, &[]).unwrap();
    e = p.edit().unwrap();
    let shown: Vec<(f64, &str)> = e.fixes.iter().map(|f| (f.start, f.text.as_str())).collect();
    assert_eq!(shown, [(1.1, "Vayne,"), (3.0, "Bane."), (7.0, "Vayne")]);
    let said = lines.borrow().clone();
    assert!(
        said.iter().any(|l| l
            == "fixed words: 2 kept on their words, 0 no longer needed, 1 set aside; 1 more place(s) fixed"),
        "{said:?}"
    );
    assert!(said
        .iter()
        .any(|l| l.contains("\"Mell\" (heard \"Mel\") at 0:00:05 is heard otherwise now")));
    // Nothing asked: nothing carried, the list as it is.
    carry_fixes(&p, &new, &mut log);
    assert_eq!(p.edit().unwrap(), e);
    assert!(!hear_again(&p, &plain, &mut log).unwrap());
    // A transcription that fails before its first chunk forgets the ask (else every later
    // run would hear the recording again); one with a partial keeps it for its resume.
    assert!(hear_again(&p, &asked, &mut log).unwrap());
    std::fs::write(&partial, "{}").unwrap();
    not_again(&p, &mut log);
    assert!(mark.is_file());
    std::fs::remove_file(&partial).unwrap();
    not_again(&p, &mut log);
    assert!(!mark.exists() && !p.dir.join(BEFORE_FILE).exists());
    assert!(!hear_again(&p, &plain, &mut log).unwrap());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_retranscription_carries_word_times_and_survives_being_stopped_or_asked_again() {
    // A word retimed by moving a cut's edge is carried like a fix; Correct asked again keeps the
    // earlier copy of the old transcript; a finished re-transcription is carried, not heard
    // again; no copy leaves the fixes, said; a failed mark takes the copy; "seen" forgets only
    // what was seen.
    let (d, p) = scratch_project("carry-times");
    let e = EditList {
        source: p.file.clone(),
        duration: 10.0,
        settings: CutSettings::default(),
        keep: vec![edit::Range {
            start: 0.0,
            end: 10.0,
        }],
        speakers: Default::default(),
        captions: Default::default(),
        fixes: vec![
            edit::Fix {
                track: 0,
                start: 1.0,
                nth: 0,
                through: None,
                text: "Vayne".into(),
            },
            edit::Fix {
                track: 1,
                start: 6.0,
                nth: 0,
                through: None,
                text: "Hola,".into(),
            },
        ],
        // "Bane" heard 1.0 to 1.3, its end moved out to 1.6; "Mel" 5.0 to 5.3, to 5.5.
        times: vec![
            edit::Retime {
                track: 0,
                start: 1.0,
                nth: 0,
                from: 1.0,
                to: 1.6,
            },
            edit::Retime {
                track: 0,
                start: 5.0,
                nth: 0,
                from: 5.0,
                to: 5.5,
            },
        ],
        windows: vec![],
    };
    std::fs::write(p.edit_path(), e.to_text()).unwrap();
    let mut old = transcript_of(&[(1.0, " Bane"), (5.0, " Mel")]);
    // A second track, which the new transcript has no words on.
    let mut other = transcript_of(&[(6.0, " hola")]).segments.remove(0);
    other.track = 1;
    old.segments.push(other);
    let old_bytes = serde_json::to_vec(&old).unwrap();
    std::fs::write(transcribe::path(&p.dir), &old_bytes).unwrap();
    let lines = std::cell::RefCell::new(Vec::<String>::new());
    let mut log = |l: &str| lines.borrow_mut().push(l.to_string());
    let asked = PipelineOptions {
        retranscribe: true,
        ..Default::default()
    };
    let plain = PipelineOptions::default();
    let (mark, before) = (p.dir.join(AGAIN_FILE), p.dir.join(BEFORE_FILE));
    assert!(hear_again(&p, &asked, &mut log).unwrap());
    // Whisper wrote the new transcript, then the app stopped before the carry.
    let new = transcript_of(&[(1.2, " Bane"), (5.0, " and")]);
    std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&new).unwrap()).unwrap();
    heard_again(&p, &mut log);
    // Opened again: that transcription had finished, so it is carried, not heard again.
    assert!(!hear_again(&p, &plain, &mut log).unwrap());
    // Correct asked again instead: the copy stays the transcript the fixes are on, and the
    // recording is heard again (also on the next open, until that hearing ends).
    assert!(hear_again(&p, &asked, &mut log).unwrap());
    assert_eq!(std::fs::read(&before).unwrap(), old_bytes);
    assert!(hear_again(&p, &plain, &mut log).unwrap());
    heard_again(&p, &mut log);
    carry_fixes(&p, &new, &mut log);
    assert!(!mark.exists() && !before.exists());
    let now = p.edit().unwrap();
    // The fix and the time moved onto "Bane" at 1.2, its end still the user's 1.6.
    assert_eq!(
        (now.fixes[0].start, now.fixes[0].text.as_str()),
        (1.2, "Vayne")
    );
    assert_eq!(
        now.times,
        [edit::Retime {
            track: 0,
            start: 1.2,
            nth: 0,
            from: 1.2,
            to: 1.6,
        }]
    );
    // "Mel" is heard as "and" now: its time is set aside and listed, with what it was. The fix
    // on track 1, which has no words this time, is set aside as such, not as heard otherwise.
    let aside = fixes_set_aside(&p);
    assert_eq!(
        aside,
        [
            SetAside {
                track: 1,
                at: 6.0,
                heard: "hola".into(),
                text: "Hola,".into(),
                retimed: None,
                silent: true,
                note: None,
            },
            SetAside {
                track: 0,
                at: 5.0,
                heard: "Mel".into(),
                text: "Mel".into(),
                retimed: Some([5.0, 5.5]),
                silent: false,
                note: None,
            }
        ]
    );
    assert!(lines.borrow().iter().any(|l| l
        == "fixed word set aside: \"Hola,\" (heard \"hola\") at 0:00:06 is on a track with no words this time"));
    assert!(lines.borrow().iter().any(|l| l
        == "fixed words: 1 kept on their words, 0 no longer needed, 1 set aside; 0 more place(s) fixed; word times: 1 kept on their words, 0 no longer needed, 1 set aside"),
        "{:?}", lines.borrow());
    // A run adds one more after the panel listed the first: "seen" forgets only that first.
    let more = SetAside {
        track: 1,
        at: 7.0,
        heard: "hom".into(),
        text: "home".into(),
        retimed: None,
        silent: true,
        note: None,
    };
    std::fs::write(
        p.dir.join(SET_ASIDE_FILE),
        serde_json::to_vec(&[aside[0].clone(), more.clone()]).unwrap(),
    )
    .unwrap();
    forget_fixes_set_aside(&p, &aside).unwrap();
    assert_eq!(fixes_set_aside(&p), std::slice::from_ref(&more));
    forget_fixes_set_aside(&p, &[more]).unwrap();
    assert!(!p.dir.join(SET_ASIDE_FILE).exists());
    // No copy of the old transcript (removed, or unreadable): the fixes are left, and said.
    let listed = p.edit().unwrap();
    assert!(hear_again(&p, &asked, &mut log).unwrap());
    std::fs::write(&before, "not json").unwrap();
    let newer = transcript_of(&[(2.0, " Bane")]);
    carry_fixes(&p, &newer, &mut log);
    assert_eq!(p.edit().unwrap(), listed);
    assert!(!mark.exists() && !before.exists());
    assert!(lines
        .borrow()
        .iter()
        .any(|l| l.starts_with("the transcript before was not kept: ")));
    // Said above the transcript too, not only in a progress line the run's end replaces.
    let aside = fixes_set_aside(&p);
    assert_eq!(aside.len(), 1);
    assert!(aside[0]
        .note
        .as_deref()
        .is_some_and(|n| n.starts_with("Correct could not move your fixed words")));
    forget_fixes_set_aside(&p, &aside).unwrap();
    // A hearing that came out exactly as the transcript it replaces is still finished: carried
    // on the next open, not heard again (told by the mark, not by the bytes).
    assert!(hear_again(&p, &asked, &mut log).unwrap());
    heard_again(&p, &mut log);
    assert_eq!(
        std::fs::read(&before).unwrap(),
        std::fs::read(transcribe::path(&p.dir)).unwrap()
    );
    assert!(!hear_again(&p, &plain, &mut log).unwrap());
    carry_fixes(&p, &new, &mut log);
    assert!(!mark.exists());
    // A mark that cannot be written leaves no copy behind.
    std::fs::create_dir(&mark).unwrap();
    assert!(hear_again(&p, &asked, &mut log).is_err());
    assert!(!before.exists());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_stretch_is_cut_again_and_fixes_and_names_forgotten_only_as_asked() {
    // The Correct pop-up's write: a selection re-cut leaves the cut outside it, fixes dropped in
    // the stretch only, and names forgotten afresh; a list of the user's that loses anything is
    // kept as a backup first.
    let (d, p) = scratch_project("rewrite");
    let turn = |start: f64, speaker| edit::Turn {
        start,
        end: start + 1.0,
        speaker,
    };
    let mut mine = edit::parse(&list(CutSettings::default(), 2.0)).unwrap();
    mine.keep.push(edit::Range {
        start: 3.5,
        end: 4.5,
    });
    mine.keep.push(edit::Range {
        start: 7.0,
        end: 8.0,
    });
    mine.speakers = Speakers::new(vec![turn(1.0, 1), turn(4.0, 2)], &[None, None], None);
    mine.speakers.rename(2, "Ana");
    edit::set_fix(&mut mine.fixes, 0, 1.25, 0, "uno", "one").unwrap();
    edit::set_fix(&mut mine.fixes, 0, 4.25, 0, "anna", "Ana").unwrap();
    edit::set_time(&mut mine.times, 0, 4.0, 0, (4.0, 4.3), (3.9, 4.3)).unwrap();
    edit::save(&p.edit_path(), &mine).unwrap();
    // The run's own cut: all of 0..5 and 6..9.
    let fresh = || {
        let mut e = edit::parse(&list(CutSettings::default(), 5.0)).unwrap();
        e.keep.push(edit::Range {
            start: 6.0,
            end: 9.0,
        });
        e
    };
    let draft_now = || Draft {
        list: fresh(),
        old: std::fs::read_to_string(p.edit_path()).ok(),
        users: true,
    };
    let found = Found {
        people: vec![],
        heard: None,
        known: vec![],
        method: Method::Voices,
        tracks: vec![None, None],
        turns: vec![turn(1.0, 1), turn(4.0, 2)],
        speech: String::new(),
        pins: vec![],
    };
    let mut log = |_: &str| {};
    let r = |a: f64, b: f64| edit::Range { start: a, end: b };
    let bak = |n: &str| p.dir.join(format!("edit.junctur.{n}"));

    // Words only in the stretch, cut and fixes kept: the list is the user's, untouched but for
    // the speakers, and nothing is backed up.
    let kept = rewrite(
        &p,
        draft_now(),
        Some(&found),
        &Rewrite {
            keep: KeepList::Whole,
            within: Some((3.0, 5.0)),
            forget: Forget::default(),
        },
        &mut log,
    )
    .unwrap();
    assert_eq!(kept.keep, mine.keep);
    assert_eq!(kept.fixes, mine.fixes);
    assert!(!bak("bak").exists());

    // The cut made again in 3..5 and the fixes dropped there: the fresh cut inside it, the
    // user's ranges outside it, the fix at 1.25 kept, the one at 4.25 and the word time gone.
    let recut = rewrite(
        &p,
        draft_now(),
        Some(&found),
        &Rewrite {
            keep: KeepList::No,
            within: Some((3.0, 5.0)),
            forget: Forget {
                names: false,
                fixes: true,
            },
        },
        &mut log,
    )
    .unwrap();
    assert_eq!(recut.keep, [r(0.0, 2.0), r(3.0, 5.0), r(7.0, 8.0)]);
    assert_eq!(recut.fixes.len(), 1);
    assert_eq!(recut.fixes[0].start, 1.25);
    assert!(recut.times.is_empty());
    assert_eq!(recut.speakers.name(2), Some("Ana"), "names kept");
    assert_eq!(
        edit::parse(edit::unstamped(
            &std::fs::read_to_string(bak("bak")).unwrap()
        ))
        .unwrap(),
        mine
    );

    // Names afresh over the whole list: Ana is "Speaker 2" again, the cut and the fix left;
    // backed up again.
    let afresh = rewrite(
        &p,
        draft_now(),
        Some(&found),
        &Rewrite {
            keep: KeepList::Whole,
            within: None,
            forget: Forget {
                names: true,
                fixes: false,
            },
        },
        &mut log,
    )
    .unwrap();
    assert_eq!(afresh.speakers.name(2), Some("Speaker 2"));
    assert_eq!(afresh.keep, recut.keep);
    assert_eq!(afresh.fixes, recut.fixes);
    assert!(bak("bak2").exists());

    // The fixes dropped everywhere with a fresh cut of the whole timeline: none left.
    let all = rewrite(
        &p,
        draft_now(),
        Some(&found),
        &Rewrite {
            keep: KeepList::No,
            within: None,
            forget: Forget {
                names: false,
                fixes: true,
            },
        },
        &mut log,
    )
    .unwrap();
    assert_eq!(all.keep, fresh().keep);
    assert!(all.fixes.is_empty() && all.times.is_empty());

    // A list edited outside Junctur cannot have a stretch cut again: refused, left as it is.
    let by_hand = format!("# my notes\n{}", mine.to_text());
    std::fs::write(p.edit_path(), &by_hand).unwrap();
    let err = rewrite(
        &p,
        draft_now(),
        Some(&found),
        &Rewrite {
            keep: KeepList::No,
            within: Some((3.0, 5.0)),
            forget: Forget::default(),
        },
        &mut log,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("outside Junctur"), "{err}");
    assert_eq!(std::fs::read_to_string(p.edit_path()).unwrap(), by_hand);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_stretch_is_refused_with_the_speakers_or_nothing_to_splice_into() {
    let (d, p) = scratch_project("within-check");
    let within = |o: PipelineOptions| PipelineOptions {
        within: Some((1.0, 2.0)),
        ..o
    };
    let err = |o: &PipelineOptions| check(&p, o).unwrap_err().to_string();
    assert!(err(&within(PipelineOptions {
        rehear: true,
        ..Default::default()
    }))
    .contains("whole recording"));
    assert!(err(&within(PipelineOptions {
        forget: Forget {
            names: true,
            fixes: false
        },
        ..Default::default()
    }))
    .contains("whole recording"));
    assert!(err(&within(PipelineOptions {
        retranscribe: true,
        ..Default::default()
    }))
    .contains("no transcript"));
    // A transcript made with other languages than the settings ask would be mixed.
    let mut t = transcript_of(&[(1.5, "hola")]);
    t.languages = vec!["es".into()];
    std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&t).unwrap()).unwrap();
    assert!(err(&within(PipelineOptions {
        retranscribe: true,
        ..Default::default()
    }))
    .contains("another model or languages"));
    // A stretch with only the cut or the fixes asks nothing of the transcript.
    assert!(check(&p, &within(PipelineOptions::default())).is_ok());
    assert!(err(&PipelineOptions {
        within: Some((2.0, 2.0)),
        ..Default::default()
    })
    .contains("empty"));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_selection_transcribed_again_spreads_fixes_only_inside_it() {
    // Re-analyse selection: the carry's spread gives a fix to the new words heard alike inside
    // the stretch, never to words outside it, which stay as they were; the stretch is kept beside
    // the mark for a resumed run and goes with it.
    let (d, p) = scratch_project("carry-within");
    let fix = |start: f64, text: &str| edit::Fix {
        track: 0,
        start,
        nth: 0,
        through: None,
        text: text.into(),
    };
    let e = EditList {
        source: p.file.clone(),
        duration: 10.0,
        settings: CutSettings::default(),
        keep: vec![edit::Range {
            start: 0.0,
            end: 10.0,
        }],
        speakers: Default::default(),
        captions: Default::default(),
        fixes: vec![fix(1.0, "Vayne")],
        times: Vec::new(),
        windows: vec![],
    };
    std::fs::write(p.edit_path(), e.to_text()).unwrap();
    let old = transcript_of(&[(1.0, " Bane"), (3.0, " Bane"), (7.0, " Ben")]);
    std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&old).unwrap()).unwrap();
    let mut log = |_: &str| {};
    let whole = PipelineOptions {
        retranscribe: true,
        ..Default::default()
    };
    let stretch = PipelineOptions {
        within: Some((6.0, 9.0)),
        ..whole.clone()
    };
    let within = p.dir.join(WITHIN_FILE);
    // Asked for the whole recording after a stretch: the carry covers everything.
    assert!(hear_again(&p, &stretch, &mut log).unwrap());
    assert!(within.is_file());
    assert!(hear_again(&p, &whole, &mut log).unwrap());
    assert!(!within.exists());
    not_again(&p, &mut log);
    // A stretch alone: the new words at 7 are heard "Bane" now and take the fix; the "Bane" at
    // 3, outside the stretch, stays as heard.
    assert!(hear_again(&p, &stretch, &mut log).unwrap());
    let new = transcript_of(&[(1.0, " Bane"), (3.0, " Bane"), (7.0, " Bane")]);
    std::fs::write(transcribe::path(&p.dir), serde_json::to_vec(&new).unwrap()).unwrap();
    carry_fixes(&p, &new, &mut log);
    let shown: Vec<(f64, String)> = p
        .edit()
        .unwrap()
        .fixes
        .iter()
        .map(|f| (f.start, f.text.clone()))
        .collect();
    assert_eq!(shown, [(1.0, "Vayne".into()), (7.0, "Vayne".into())]);
    assert!(!within.exists() && !p.dir.join(AGAIN_FILE).exists());
    std::fs::remove_dir_all(&d).unwrap();
}
