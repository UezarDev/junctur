use super::*;

fn word(start: f64, text: &str) -> Word {
    Word {
        start,
        end: start + 0.3,
        text: text.into(),
        p: 0.9,
        p_min: None,
    }
}

fn on(track: usize, words: &[Word]) -> Vec<(usize, &Word)> {
    words.iter().map(|w| (track, w)).collect()
}

#[test]
fn phrases_are_found_folded_in_english_and_spanish() {
    let words = [
        word(10.0, "ok"),
        word(40.0, " Clip"),
        word(40.4, "THAT!"),
        word(100.0, "¡Márcalo,"),
        word(200.0, "clip-this"),
        word(300.0, "marca"),
        word(300.3, "eso."),
    ];
    let m = voice_markers(&on(0, &words));
    let at: Vec<f64> = m.iter().map(|x| x.at).collect();
    assert_eq!(at, [40.0, 100.0, 200.0, 300.0]);
    assert_eq!(m[0].text, "Clip THAT!");
    assert_eq!(m[0].cut, Some((40.0, 40.7)));
    assert_eq!(m[1].cut, Some((100.0, 100.3)));
    assert!(m.iter().all(|x| x.source == Source::Voice));
}

#[test]
fn a_phrase_is_one_tracks_words_said_together() {
    // "clip" on one track and "that" on another is no phrase, nor is "clip ... that" 5 s apart.
    let a = [word(10.0, "clip"), word(30.0, "clip")];
    let b = [word(10.4, "that")];
    let late = word(35.6, "that");
    let mut words = on(0, &a);
    words.extend(on(1, &b));
    words.push((0, &late));
    words.sort_by(|x, y| x.1.start.total_cmp(&y.1.start));
    assert!(voice_markers(&words).is_empty());
    // "clipped that" or "cut thistle" are other words.
    let other = [
        word(1.0, "clipped"),
        word(1.4, "that"),
        word(5.0, "cut"),
        word(5.3, "thistle"),
    ];
    assert!(voice_markers(&on(0, &other)).is_empty());
}

#[test]
fn every_listed_phrase_matches_itself() {
    for p in PHRASES {
        let words: Vec<Word> = p
            .split_whitespace()
            .enumerate()
            .map(|(i, t)| word(50.0 + i as f64 * 0.4, t))
            .collect();
        assert_eq!(voice_markers(&on(0, &words)).len(), 1, "{p}");
    }
}

#[test]
fn timestamps_read_as_seconds_with_their_labels() {
    let text = "\u{feff}# OBS markers\n\n754.2\n12:34 big fight\n1:02:03.250,baron steal\n\t5:00\tace \nnope\n1:75\n-3\n1e3\n";
    let (m, bad) = hotkey_markers(text);
    let at: Vec<f64> = m.iter().map(|x| x.at).collect();
    assert_eq!(at, [300.0, 754.0, 754.2, 3723.25]);
    assert_eq!(bad, 4);
    let label = |t: f64| m.iter().find(|x| x.at == t).unwrap().text.clone();
    assert_eq!(label(754.0), "big fight");
    assert_eq!(label(3723.25), "baron steal");
    assert_eq!(label(300.0), "ace");
    assert_eq!(label(754.2), "");
    assert!(m
        .iter()
        .all(|x| x.source == Source::Hotkey && x.cut.is_none()));
}

#[test]
fn the_markers_file_is_beside_the_recording_or_in_the_project() {
    let dir = std::env::temp_dir().join(format!("junctur-moments-{}", std::process::id()));
    let project = dir.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let source = dir.join("stream 1.mkv");
    assert_eq!(markers_file(&source, &project), None);
    std::fs::write(project.join(MARKERS), "1\n").unwrap();
    assert_eq!(markers_file(&source, &project), Some(project.join(MARKERS)));
    std::fs::write(dir.join("stream 1.markers.txt"), "2\n").unwrap();
    assert_eq!(
        markers_file(&source, &project),
        Some(dir.join("stream 1.markers.txt"))
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn marker(at: f64, source: Source, text: &str) -> Marker {
    Marker {
        at,
        source,
        cut: (source == Source::Voice).then_some((at, at + 0.6)),
        text: text.into(),
    }
}

#[test]
fn each_marker_proposes_the_half_minute_before_it() {
    let m = moments(
        vec![
            marker(100.0, Source::Voice, "clip that"),
            marker(12.0, Source::Hotkey, ""),
            marker(0.0, Source::Hotkey, "at the very start"),
            marker(500.0, Source::Hotkey, "past the end"),
        ],
        400.0,
    );
    assert_eq!(m.len(), 3);
    // Nothing before 0 s: a clip from the start, and none for a marker at 0.
    assert_eq!(
        (m[0].start, m[0].end, m[0].label.as_str()),
        (0.0, 12.0, "Hotkey marker")
    );
    assert_eq!((m[1].start, m[1].end), (70.0, 100.0));
    assert_eq!(m[1].label, "\u{201c}clip that\u{201d}");
    assert_eq!(m[1].cuts, [(100.0, 100.6)]);
    // Past the end: at the end.
    assert_eq!((m[2].start, m[2].end), (370.0, 400.0));
}

#[test]
fn markers_close_together_are_one_moment_with_every_phrase_cut() {
    let m = moments(
        vec![
            marker(100.0, Source::Voice, "clip that"),
            marker(102.0, Source::Voice, "clip that"),
            marker(104.5, Source::Hotkey, ""),
            marker(120.0, Source::Hotkey, ""),
        ],
        1000.0,
    );
    assert_eq!(m.len(), 2);
    assert_eq!((m[0].at, m[0].start, m[0].end), (100.0, 70.0, 104.5));
    assert_eq!(m[0].cuts, [(100.0, 100.6), (102.0, 102.6)]);
    assert_eq!(m[0].source, Source::Voice);
    assert_eq!((m[1].start, m[1].end), (90.0, 120.0));
}

fn r(start: f64, end: f64) -> Range {
    Range { start, end }
}

#[test]
fn accepting_keeps_the_clip_cuts_the_phrase_and_undo_puts_back_what_was_kept() {
    let all = moments(vec![marker(100.0, Source::Voice, "clip that")], 1000.0);
    let m = &all[0];
    let now = [r(0.0, 60.0), r(75.0, 80.0), r(95.0, 130.0)];
    let mut answers = Answers::default();
    let edits = accept(&[m], &all, &now, &mut answers);
    assert_eq!(edits, [(70.0, 100.0, true), (100.0, 100.6, false)]);
    let a = answers.of(m).unwrap();
    assert_eq!(a.how, How::Accepted);
    assert_eq!(a.before, [r(75.0, 80.0), r(95.0, 100.6)]);
    let back = undo(m, &mut answers);
    assert_eq!(
        back,
        [
            (70.0, 100.6, false),
            (75.0, 80.0, true),
            (95.0, 100.6, true)
        ]
    );
    // Waiting again, and a second undo does nothing.
    assert!(answers.of(m).is_none());
    assert!(undo(m, &mut answers).is_empty());
}

#[test]
fn rejecting_edits_nothing_and_answers_survive_a_save() {
    let m = &moments(vec![marker(42.0, Source::Hotkey, "")], 100.0)[0];
    let mut answers = Answers::default();
    reject(m, &mut answers);
    assert_eq!(answers.of(m).unwrap().how, How::Rejected);
    // A rejected moment has nothing to undo.
    assert!(undo(m, &mut answers).is_empty());
    let dir = std::env::temp_dir().join(format!("junctur-answers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(load_answers(&dir).unwrap(), Answers::default());
    save_answers(&dir, &answers).unwrap();
    assert_eq!(load_answers(&dir).unwrap(), answers);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_clip_reaching_an_accepted_moment_keeps_its_phrase_cut() {
    let all = moments(
        vec![
            marker(100.0, Source::Voice, "clip that"),
            marker(120.0, Source::Voice, "clip that"),
        ],
        1000.0,
    );
    let mut answers = Answers::default();
    assert_eq!(
        accept(&[&all[0]], &all, &[], &mut answers),
        [(70.0, 100.0, true), (100.0, 100.6, false)]
    );
    // The second clip (90 to 120 s) holds the first phrase: it is cut again after the keep.
    assert_eq!(
        accept(&[&all[1]], &all, &[], &mut answers),
        [
            (90.0, 120.0, true),
            (100.0, 100.6, false),
            (120.0, 120.6, false)
        ]
    );
    // Accepting both at once: every keep first, then every cut.
    let mut fresh = Answers::default();
    let both: Vec<&Moment> = all.iter().collect();
    assert_eq!(
        accept(&both, &all, &[], &mut fresh),
        [
            (70.0, 100.0, true),
            (90.0, 120.0, true),
            (100.0, 100.6, false),
            (120.0, 120.6, false)
        ]
    );
}
