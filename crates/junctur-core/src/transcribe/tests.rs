use super::partial::{read_partial, VERSION};
use super::*;

fn header(languages: &str, model: &str) -> Header {
    Header {
        version: VERSION,
        languages: languages.into(),
        model: model.into(),
        device: Device::Cuda,
    }
}

#[test]
fn the_log_names_the_word_list_and_what_was_left_out() {
    use crate::wordlist::Prompt;
    let p = |kept, left: &[&str]| Prompt {
        text: String::new(),
        kept,
        left_out: left.iter().map(|s| s.to_string()).collect(),
    };
    assert_eq!(prompt_line(&p(0, &[])), None, "no list, no line");
    assert_eq!(
        prompt_line(&p(1, &[])).unwrap(),
        "word list: 1 entry in Whisper's prompt"
    );
    let cut = prompt_line(&p(40, &["Junctur", "Resolve"])).unwrap();
    assert!(
        cut.starts_with("word list: 40 entries in Whisper's prompt, 2 entries left out"),
        "{cut}"
    );
    assert!(cut.ends_with(": Junctur, Resolve"), "{cut}");
}

#[test]
fn a_run_stops_before_whisper_and_between_chunks_never_after_the_last() {
    let mut calls = 0;
    let mut between = || calls += 1;
    let mut stops = Stops::new(3, &mut between);
    stops.chunk_done();
    stops.chunk_done();
    stops.chunk_done();
    // Before the first chunk, after the first and after the second.
    assert_eq!(calls, 3);
}

#[test]
fn partial_resume_drops_torn_line_and_foreign_header() {
    let dir = std::env::temp_dir().join(format!("junctur-partial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("p.jsonl");
    let h = header("es+en", "large-v3-turbo");
    let seg = Segment {
        track: 0,
        start: 1.0,
        end: 2.0,
        lang: "es".into(),
        model: "large-v3-turbo".into(),
        words: vec![],
    };
    let body = format!(
        "{}\n{}\n{{\"track\":0,\"sta",
        serde_json::to_string(&h).unwrap(),
        serde_json::to_string(&seg).unwrap()
    );
    std::fs::write(&p, body).unwrap();
    let (got, segs) = read_partial(&p).unwrap();
    assert_eq!((got.clone(), segs), (h.clone(), vec![seg.clone()]));
    assert!(got.same_run(&Header {
        device: Device::Cpu,
        ..h.clone()
    }));
    assert!(!got.same_run(&header("en", "large-v3-turbo")));
    assert!(!got.same_run(&header("es+en", "large-v3")));
    // Resuming rewrites the torn partial, so the next chunk gets a line of its own; so does a
    // last line that is whole but lost its newline.
    let next = Segment {
        start: 3.0,
        end: 4.0,
        ..seg.clone()
    };
    let append = |segs: &[Segment]| {
        let mut out = open_partial(&p, &h, segs).unwrap();
        out.write_all(format!("{}\n", serde_json::to_string(&next).unwrap()).as_bytes())
            .unwrap();
    };
    append(std::slice::from_ref(&seg));
    let (_, segs) = read_partial(&p).unwrap();
    assert_eq!(segs, vec![seg.clone(), next.clone()]);
    let unterminated = format!(
        "{}\n{}",
        serde_json::to_string(&h).unwrap(),
        serde_json::to_string(&seg).unwrap()
    );
    std::fs::write(&p, unterminated).unwrap();
    let (_, segs) = read_partial(&p).unwrap();
    append(&segs);
    assert_eq!(read_partial(&p).unwrap().1, vec![seg.clone(), next]);
    // A version-1 partial (no model) is not resumed.
    std::fs::write(&p, "{\"version\":1,\"languages\":\"es+en\"}\n").unwrap();
    assert!(read_partial(&p).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_track_reaches_its_first_chunk_not_done_and_its_end_when_all_are() {
    let chunk = |start: f64| Chunk {
        start,
        end: start + 10.0,
    };
    let source = |track| Source {
        track,
        regions: vec![],
    };
    // Track 0: chunks at 0, 50 and 200; the one at 50 done by an interrupted run. Track 1: one
    // chunk. Track 2: no speech, so nothing to wait for.
    let work = [
        (0, chunk(0.0)),
        (0, chunk(50.0)),
        (0, chunk(200.0)),
        (1, chunk(30.0)),
    ];
    let groups = audio::groups(&work);
    let todo: Vec<Vec<bool>> = groups
        .iter()
        .map(|g| g.chunks.iter().map(|c| c.start != 50.0).collect())
        .collect();
    let mut r = Reached::new(&[source(0), source(1), source(2)], &groups, &todo);
    let end = f64::INFINITY;
    assert_eq!(r.all(), vec![(0, 0.0), (1, 30.0), (2, end)]);
    // The first chunk done: the silence after it and the resumed chunk are behind us too.
    assert_eq!(r.finish(0, 0.0), Some(200.0));
    // The last one: the whole track, though its speech ended at 210 s.
    assert_eq!(r.finish(0, 200.0), Some(end));
    assert_eq!(r.finish(1, 30.0), Some(end));
    // A chunk of a track not asked for, or done twice, moves nothing.
    assert_eq!(r.finish(5, 0.0), None);
    assert_eq!(r.finish(1, 30.0), None);
    assert_eq!(r.whole(), end);
}

#[test]
fn the_transcript_is_whole_up_to_the_track_furthest_behind() {
    let chunk = |start: f64| Chunk {
        start,
        end: start + 10.0,
    };
    let source = |track| Source {
        track,
        regions: vec![],
    };
    let work = [(0, chunk(0.0)), (0, chunk(320.0)), (1, chunk(100.0))];
    let groups = audio::groups(&work);
    let todo: Vec<Vec<bool>> = groups.iter().map(|g| vec![true; g.chunks.len()]).collect();
    let mut r = Reached::new(&[source(0), source(1)], &groups, &todo);
    assert_eq!(r.whole(), 0.0);
    r.finish(0, 0.0);
    // Track 0 is at 320 s, track 1 still at 100 s: words before 100 s are all in.
    assert_eq!(r.whole(), 100.0);
    r.finish(1, 100.0);
    assert_eq!(r.whole(), 320.0);
    assert_eq!(piece(r.whole()), 1);
    // No tracks: nothing to wait for.
    assert_eq!(Reached::new(&[], &[], &[]).whole(), f64::INFINITY);
}

#[test]
fn the_work_goes_piece_by_piece_and_no_language_group_spans_two() {
    use crate::vad::Region;
    let speech = |spans: &[(f64, f64)]| {
        spans
            .iter()
            .map(|&(start, end)| Region { start, end })
            .collect::<Vec<_>>()
    };
    let sources = [
        Source {
            track: 2,
            regions: speech(&[(10.0, 20.0), (400.0, 410.0), (900.0, 905.0)]),
        },
        Source {
            track: 0,
            regions: speech(&[(50.0, 60.0), (299.0, 299.5), (310.0, 320.0)]),
        },
    ];
    let got: Vec<(usize, u64)> = order(&sources, 1000.0)
        .iter()
        .map(|(t, c)| (*t, c.start as u64))
        .collect();
    // Piece 0 (0-5 min): track 2 then track 0, each in time; the chunk at 4:58.8 is piece 0's
    // though it ends after 5:00. Then piece 1, then piece 3 (piece 2 has no speech).
    assert_eq!(
        got,
        vec![(2, 9), (0, 49), (0, 298), (2, 399), (0, 309), (2, 899)]
    );
    // A piece's last group ends the piece: languages are never detected across a boundary.
    let groups = audio::groups(&order(&sources, 1000.0));
    assert!(groups.iter().all(|g| g
        .chunks
        .iter()
        .all(|c| piece(c.start) == piece(g.chunks[0].start))));
}

#[test]
fn one_transcription_of_a_recording_at_a_time() {
    let dir = std::env::temp_dir().join(format!("junctur-tlock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(!transcribing_elsewhere(&dir));
    let held = Transcribing::claim(&dir).unwrap();
    assert!(transcribing_elsewhere(&dir), "the lock is seen");
    assert!(
        Transcribing::claim(&dir).is_err(),
        "a second run is refused"
    );
    drop(held);
    assert!(!transcribing_elsewhere(&dir), "let go when done");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn so_far_is_the_partial_in_the_finished_order() {
    let dir = std::env::temp_dir().join(format!("junctur-sofar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(so_far(&dir).is_none(), "no partial: nothing");
    let seg = |track, start: f64| Segment {
        track,
        start,
        end: start + 1.0,
        lang: "es".into(),
        model: "large-v3-turbo".into(),
        words: vec![],
    };
    // Written in Whisper's order (piece, then track); a run killed mid-line.
    let mut body = serde_json::to_string(&header("es+en", "large-v3-turbo")).unwrap() + "\n";
    for s in [seg(1, 5.0), seg(1, 40.0), seg(0, 20.0), seg(0, 5.0)] {
        body += &(serde_json::to_string(&s).unwrap() + "\n");
    }
    body += "{\"track\":0,\"st";
    std::fs::write(partial_path(&dir), body).unwrap();
    let got: Vec<(usize, f64)> = so_far(&dir)
        .unwrap()
        .iter()
        .map(|s| (s.track, s.start))
        .collect();
    assert_eq!(got, vec![(0, 5.0), (1, 5.0), (0, 20.0), (1, 40.0)]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reading_on_as_the_partial_grows_gives_what_reading_it_whole_gives() {
    let dir = std::env::temp_dir().join(format!("junctur-sofar-on-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The model tells apart chunks of one track and start, whose order is the file's.
    let seg = |track, start: f64, model: &str| {
        serde_json::to_string(&Segment {
            track,
            start,
            end: start + 1.0,
            lang: "es".into(),
            model: model.into(),
            words: vec![],
        })
        .unwrap()
            + "\n"
    };
    let head = |model| serde_json::to_string(&header("es+en", model)).unwrap() + "\n";
    let mut r = SoFar::default();
    let check = |r: &mut SoFar, body: &str, what: &str| {
        std::fs::write(partial_path(&dir), body).unwrap();
        let whole = so_far(&dir);
        assert_eq!(r.read(&dir).map(|c| c.into_owned()), whole, "{what}");
        // Asked again with nothing new.
        assert_eq!(
            r.read(&dir).map(|c| c.into_owned()),
            so_far(&dir),
            "{what}, again"
        );
    };
    assert!(r.read(&dir).is_none(), "no partial");
    let mut body = head("m");
    check(&mut r, &body, "a header alone");
    check(&mut r, body.trim_end(), "a header without its newline");
    body += &seg(1, 5.0, "a");
    body += &seg(0, 20.0, "b");
    check(&mut r, &body, "two chunks");
    body += &seg(0, 5.0, "c");
    body += &seg(1, 5.0, "d");
    check(&mut r, &body, "two more, one starting with one read before");
    let whole_lines = body.clone();
    body += r#"{"track":0,"st"#;
    check(&mut r, &body, "a torn line");
    body = whole_lines.clone() + seg(2, 1.0, "e").trim_end();
    check(&mut r, &body, "a last line whole but for its newline");
    body = whole_lines + &seg(2, 1.0, "e") + &seg(1, 5.0, "f") + "\n" + "junk\n";
    check(
        &mut r,
        &body,
        "it, finished, then more, a blank line and junk",
    );
    // Written afresh by a new run: longer than what was read, a different start.
    let fresh = head("m2")
        + &seg(3, 1.0, "g")
        + &seg(3, 2.0, "h")
        + &seg(3, 3.0, "i")
        + &seg(3, 4.0, "j")
        + &seg(3, 5.0, "k")
        + &seg(3, 6.0, "l")
        + &seg(3, 7.0, "n");
    assert!(fresh.len() > body.len());
    check(&mut r, &fresh, "a partial written afresh");
    check(&mut r, &seg(0, 1.0, "x"), "no header");
    check(&mut r, &head("m"), "a header again");
    std::fs::write(partial_path(&dir), [0xffu8, b'\n']).unwrap();
    assert_eq!(
        r.read(&dir).map(|c| c.into_owned()),
        so_far(&dir),
        "not UTF-8"
    );
    std::fs::remove_file(partial_path(&dir)).unwrap();
    assert!(r.read(&dir).is_none(), "gone");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn finished_transcript_reuse_follows_the_model_and_languages() {
    let t = |model: &str| Transcript {
        choice: Choice {
            model: model.into(),
            device: Device::Cuda,
            note: None,
        },
        languages: Languages::default().allowed,
        segments: vec![],
        audio_s: 0.0,
        elapsed_s: 0.0,
        resumed_chunks: 0,
    };
    let opts = |model: ModelPref| TranscribeOptions {
        model,
        ..Default::default()
    };
    // Auto keeps whatever made it, a q5_0 transcript included.
    assert!(reusable(&t("large-v3-q5_0"), &opts(ModelPref::Auto)));
    assert!(reusable(
        &t("large-v3-turbo"),
        &opts(ModelPref::LargeV3Turbo)
    ));
    // An explicit model that differs re-transcribes.
    assert!(!reusable(&t("large-v3-turbo"), &opts(ModelPref::LargeV3)));
    assert!(!reusable(
        &t("large-v3-q5_0"),
        &opts(ModelPref::LargeV3Turbo)
    ));
    let mut other = t("large-v3-turbo");
    other.languages = vec!["en".into()];
    assert!(!reusable(&other, &opts(ModelPref::Auto)));
}

#[test]
fn again_drops_a_partial_and_never_reuses_the_finished_transcript() {
    // With nothing to hear (no sources) the run is a finish without Whisper, which shows what
    // it starts from.
    let dir = std::env::temp_dir().join(format!("junctur-again-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let old = Transcript {
        choice: Choice {
            model: "large-v3-turbo".into(),
            device: Device::Cpu,
            note: None,
        },
        languages: Languages::default().allowed,
        segments: vec![],
        audio_s: 42.0,
        elapsed_s: 0.0,
        resumed_chunks: 0,
    };
    std::fs::write(path(&dir), serde_json::to_vec(&old).unwrap()).unwrap();
    let run = |opts: &TranscribeOptions| {
        transcribe(
            &dir.join("rec.mp4"),
            &[],
            10.0,
            &dir,
            &dir,
            opts,
            &mut |_| {},
            Reports {
                reach: &mut |_, _| {},
                transcribed: &mut |_| {},
                between: &mut || {},
            },
        )
        .unwrap()
    };
    // Without it the finished transcript answers.
    assert_eq!(run(&TranscribeOptions::default()).audio_s, 42.0);
    // With it a partial of an older run is not resumed and the transcript is made anew.
    std::fs::write(partial_path(&dir), "torn").unwrap();
    assert!(interrupted(&dir));
    let opts = TranscribeOptions {
        again: true,
        ..Default::default()
    };
    assert_eq!(run(&opts).audio_s, 0.0);
    assert!(!interrupted(&dir));
    assert_eq!(load(&dir).unwrap().audio_s, 0.0);
    std::fs::remove_dir_all(&dir).unwrap();
}
