use super::*;
use crate::edit::Captions;

fn sample() -> EditList {
    EditList {
        source: PathBuf::from(r"C:\Videos\my stream.mp4"),
        duration: 100.0,
        settings: CutSettings::default(),
        keep: vec![
            Range {
                start: 1.5,
                end: 4.25,
            },
            Range {
                start: 10.0,
                end: 12.0,
            },
        ],
        speakers: Speakers::default(),
        captions: None,
        fixes: Vec::new(),
        times: Vec::new(),
        windows: vec![Window {
            start: 0.0,
            score: 0.5,
            speech: 0.5,
            words: 7,
        }],
    }
}

fn with_speakers() -> EditList {
    EditList {
        speakers: Speakers {
            names: vec![
                SpeakerName {
                    id: 1,
                    name: "Speaker 1".into(),
                    track: None,
                    color: "#F2B33D".into(),
                },
                SpeakerName {
                    id: 2,
                    name: "Lucía de la Cruz".into(),
                    track: Some(1),
                    color: "#123ABC".into(),
                },
            ],
            turns: vec![
                Turn {
                    start: 1.5,
                    end: 3.0,
                    speaker: 1,
                },
                Turn {
                    start: 2.5,
                    end: 4.0,
                    speaker: 2,
                },
            ],
        },
        ..sample()
    }
}

#[test]
fn speakers_round_trip_and_stay_out_of_lists_without_them() {
    let e = with_speakers();
    let text = e.to_text();
    assert!(text.starts_with("junctur-edit 2\n"), "{text}");
    assert!(
        text.contains("speaker 2 track=1 color=#123ABC Lucía de la Cruz\n"),
        "{text}"
    );
    assert!(text.contains("turn 2.500 4.000 2\n"));
    assert!(
        !text.contains("captions"),
        "a list without its own captions writes none"
    );
    assert_eq!(parse(&text).unwrap(), e);
    let plain = sample().to_text();
    assert!(!plain.contains("speaker"));
    assert!(plain.starts_with("junctur-edit 1\n"));
    let newer = format!("junctur-edit {}\n", FORMAT_VERSION + 1);
    assert!(parse(&newer).is_err());
    // Caption settings alone make a version-2 list, and round-trip.
    let c = EditList {
        captions: Some(Captions {
            on: true,
            chars: 32,
            lines: 1,
            seconds: 4.5,
            ..Captions::default()
        }),
        ..sample()
    };
    let text = c.to_text();
    assert!(text.starts_with("junctur-edit 2\n"));
    assert!(
        text.contains("captions on chars=32 lines=1 seconds=4.5\n"),
        "{text}"
    );
    assert_eq!(parse(&text).unwrap(), c);
    // Own settings equal to the built-in default are still the list's own: written, and
    // read back as set rather than as following the user's defaults.
    let own = EditList {
        captions: Some(Captions::default()),
        ..sample()
    };
    let text = own.to_text();
    assert!(text.starts_with("junctur-edit 2\n"), "{text}");
    assert!(
        text.contains("captions off chars=42 lines=2 seconds=6\n"),
        "{text}"
    );
    assert_eq!(parse(&text).unwrap().captions, Some(Captions::default()));
}

#[test]
fn hand_edited_colours_and_captions_are_read_or_refused() {
    let head = "junctur-edit 2\nsource x.mp4\n";
    let e = parse(&format!(
        "{head}speaker 1 color=#a0b0c0 Ana\nspeaker 2 track=3 Bo\ncaptions off\n"
    ))
    .unwrap();
    assert_eq!(e.speakers.color(1), Some("#A0B0C0"));
    assert_eq!(e.speakers.color(2), Some(default_color(2).as_str()));
    assert_eq!(e.speakers.name(2), Some("Bo"));
    assert_eq!(e.captions, Some(Captions::default()));
    // Not a colour: part of the name, as an older list would have it.
    let old = parse(&format!("{head}speaker 1 color=red Ana\n")).unwrap();
    assert_eq!(old.speakers.name(1), Some("color=red Ana"));
    assert_eq!(old.speakers.color(1), Some(default_color(1).as_str()));
    let back = parse(&old.to_text()).unwrap();
    assert_eq!(back.speakers, old.speakers);
    assert!(parse(&format!("{head}captions on chars=42.9\n")).is_err());
    assert!(parse(&format!("{head}captions maybe\n")).is_err());
    assert!(parse(&format!("{head}captions on chars=500\n")).is_err());
    assert!(parse(&format!("{head}captions on shadow=3\n")).is_err());
    // `size`: 3 % is in range, 1 % is not.
    assert!(parse(&format!("{head}captions on size=3\n")).is_ok());
    assert!(parse(&format!("{head}captions on size=1\n")).is_err());
    assert!(parse(&format!("{head}captions on bold=yes\n")).is_err());
    assert!(parse(&format!("{head}captions on underline=yes\n")).is_err());
}

#[test]
fn caption_switches_and_styles_round_trip() {
    // Each written only when on, after the cue settings; off by hand reads as off.
    let c = EditList {
        captions: Some(Captions {
            preview: true,
            italic: true,
            underline: true,
            ..Captions::default()
        }),
        ..sample()
    };
    let text = c.to_text();
    assert!(
        text.contains(
            "captions off chars=42 lines=2 seconds=6 preview=on italic=on underline=on\n"
        ),
        "{text}"
    );
    assert_eq!(parse(&text).unwrap(), c);
    let head = "junctur-edit 2\nsource x.mp4\n";
    let e = parse(&format!("{head}captions on bold=on preview=off\n")).unwrap();
    let c = e.captions.unwrap();
    assert!(c.on && c.bold && !c.preview && !c.italic && !c.underline);
    let e = parse(&format!("{head}captions on underline=off\n")).unwrap();
    assert!(!e.captions.unwrap().underline);
}

#[test]
fn hand_edited_speakers_are_read_or_refused_by_line() {
    let head = "junctur-edit 2\nsource x.mp4\n";
    // A rename by hand, a blank name, turns before their speakers and out of order.
    let e = parse(&format!(
        "{head}turn 5 6 2\nturn 1 2 1\nspeaker 2   Ana  \nspeaker 1\n"
    ))
    .unwrap();
    assert_eq!(e.speakers.name(1), Some("Speaker 1"));
    assert_eq!(e.speakers.name(2), Some("Ana"));
    assert_eq!(e.speakers.turns[0].speaker, 1);
    let err = parse(&format!("{head}speaker 1 A\nturn 1 2 3\n")).unwrap_err();
    assert!(
        err.starts_with("line 4") && err.contains("speaker 3"),
        "{err}"
    );
    assert!(parse(&format!("{head}speaker 0 A\n")).is_err());
    assert!(parse(&format!("{head}speaker 1 A\nspeaker 1 B\n")).is_err());
    assert!(parse(&format!("{head}speaker 1 track=x A\n")).is_err());
    assert!(parse(&format!("{head}speaker 1 A\nturn 2 1 1\n")).is_err());
}

#[test]
fn fixed_words_round_trip_as_version_3_and_bad_lines_are_refused() {
    let e = EditList {
        fixes: vec![
            Fix {
                track: 0,
                start: 2.5,
                nth: 0,
                through: None,
                text: "Tsun Heto,".into(),
            },
            Fix {
                track: 1,
                start: 1.25,
                nth: 0,
                through: None,
                text: "Uezar".into(),
            },
        ],
        ..with_speakers()
    };
    let text = e.to_text();
    assert!(text.starts_with("junctur-edit 3\n"), "{text}");
    assert!(
        text.contains("fix 1 1.250 Uezar\nfix 0 2.500 Tsun Heto,\n"),
        "{text}"
    );
    let back = parse(&text).unwrap();
    assert_eq!(back.fixes[0].text, "Uezar", "in time order");
    assert_eq!(back.fixes.len(), 2);
    assert_eq!(parse(&back.to_text()).unwrap(), back);
    // Without fixes a list is the version it was.
    assert!(sample().to_text().starts_with("junctur-edit 1\n"));
    // A start half a millisecond in survives the round trip (written as its key's millisecond).
    let half = EditList {
        fixes: vec![Fix {
            track: 0,
            start: 12.3125,
            nth: 0,
            through: None,
            text: "Uezar".into(),
        }],
        ..sample()
    };
    let text = half.to_text();
    assert!(text.contains("fix 0 12.313 Uezar\n"), "{text}");
    let back = parse(&text).unwrap();
    assert!(back.fixes[0].same_word(&half.fixes[0]));
    let head = "junctur-edit 3\nsource x.mp4\n";
    let e = parse(&format!("{head}fix 2 10.5   Delay  \n")).unwrap();
    assert_eq!(e.fixes[0].text, "Delay");
    for bad in [
        "fix 1 2.000\n",
        "fix x 2.000 Ana\n",
        "fix 1 soon Ana\n",
        "fix -1 2.000 Ana\n",
        "fix 1 2.000 Ana\nfix 1 2.0001 Bo\n",
        "fix 1 2.000+ Ana\n",
        "fix 1 2.000+x Ana\n",
        "fix 1 2.000+-1 Ana\n",
        "fix 1 2.000+1 Ana\nfix 1 2.000+1 Bo\n",
    ] {
        assert!(parse(&format!("{head}{bad}")).is_err(), "{bad}");
    }
}

#[test]
fn a_later_word_of_one_millisecond_is_written_with_its_place_as_version_4() {
    // "en" and "él" both start at 54.944; each word its own fix.
    let e = EditList {
        fixes: vec![
            Fix {
                track: 0,
                start: 54.944,
                nth: 1,
                through: None,
                text: "miel".into(),
            },
            Fix {
                track: 0,
                start: 54.944,
                nth: 0,
                through: None,
                text: "de".into(),
            },
        ],
        ..sample()
    };
    let text = e.to_text();
    assert!(text.starts_with("junctur-edit 4\n"), "{text}");
    assert!(
        text.contains("fix 0 54.944 de\nfix 0 54.944+1 miel\n"),
        "{text}"
    );
    let back = parse(&text).unwrap();
    assert_eq!(back.fixes, [e.fixes[1].clone(), e.fixes[0].clone()]);
    assert!(!back.fixes[0].same_word(&back.fixes[1]));
    assert_eq!(back.to_text(), text);
    // A bare line names the first word of its millisecond and is written back byte for byte,
    // still version 3.
    let old = EditList {
        fixes: vec![Fix {
            track: 0,
            start: 54.944,
            nth: 0,
            through: None,
            text: "mel".into(),
        }],
        ..sample()
    }
    .to_text();
    assert!(old.starts_with("junctur-edit 3\n"), "{old}");
    assert!(old.contains("\nfix 0 54.944 mel\n"), "{old}");
    let back = parse(&old).unwrap();
    assert_eq!(back.fixes[0].nth, 0);
    assert_eq!(back.to_text(), old);
    // An exponent's sign is the start's, not a place.
    let e = parse("junctur-edit 4\nsource x.mp4\nfix 0 1e+1 Ana\nfix 0 1E+1+2 Bo\n").unwrap();
    assert_eq!((e.fixes[0].start, e.fixes[0].nth), (10.0, 0));
    assert_eq!((e.fixes[1].start, e.fixes[1].nth), (10.0, 2));
}

#[test]
fn a_span_is_its_own_line_and_makes_the_list_version_5() {
    // A run of words fixed as one text.
    let at = |start, nth| WordAt { start, nth };
    let e = EditList {
        fixes: vec![
            Fix {
                track: 1,
                start: 54.944,
                nth: 1,
                through: Some(at(56.0, 2)),
                text: "Uezar went home.".into(),
            },
            Fix {
                track: 0,
                start: 2.5,
                nth: 0,
                through: None,
                text: "Ana".into(),
            },
        ],
        ..sample()
    };
    let text = e.to_text();
    assert!(text.starts_with("junctur-edit 5\n"), "{text}");
    assert!(
        text.contains("fix 0 2.500 Ana\nspan 1 54.944+1 56.000+2 Uezar went home.\n"),
        "{text}"
    );
    let back = parse(&text).unwrap();
    assert_eq!(back.fixes, [e.fixes[1].clone(), e.fixes[0].clone()]);
    assert_eq!(back.to_text(), text);
    // Without a span the list is written at the older version.
    let plain = EditList {
        fixes: vec![e.fixes[1].clone()],
        ..sample()
    };
    assert!(plain.to_text().starts_with("junctur-edit 3\n"));
    assert!(!plain.to_text().contains("span"));
    // Refused: a span whose last word is not after its first, one over a fixed word or another
    // span, a missing text or word.
    let head = "junctur-edit 5\nsource x.mp4\n";
    for bad in [
        "span 0 3.000 2.000 x\n",
        "span 0 2.000 2.000 x\n",
        "span 0 2.000+1 2.000 x\n",
        "fix 0 2.500 Ana\nspan 0 2.000 3.000 x\n",
        "span 0 2.000 3.000 x\nfix 0 3.000 Ana\n",
        "span 0 2.000 3.000 x\nspan 0 3.000 4.000 y\n",
        "span 0 2.000 3.000\n",
        "span 0 2.000 x y\n",
    ] {
        assert!(parse(&format!("{head}{bad}")).is_err(), "{bad}");
    }
    // Touching but not sharing a word, or on another track, is fine.
    let ok = parse(&format!(
        "{head}span 0 2.000 3.000 x\nspan 0 3.000+1 4.000 y\nspan 1 2.000 4.000 z\nfix 0 4.001 w\n"
    ))
    .unwrap();
    assert_eq!(ok.fixes.len(), 4);
}

#[test]
fn a_retimed_word_is_a_time_line_and_makes_the_list_version_6() {
    // A word's start and end moved with a cut's edge, named by its heard start.
    let t = |track, start, nth, from, to| Retime {
        track,
        start,
        nth,
        from,
        to,
    };
    let e = EditList {
        times: vec![t(1, 54.944, 1, 54.95, 55.3), t(0, 2.5, 0, 2.5, 2.9)],
        ..sample()
    };
    let text = e.to_text();
    assert!(text.starts_with("junctur-edit 6\n"), "{text}");
    assert!(
        text.contains("time 0 2.500 2.500 2.900\ntime 1 54.944+1 54.950 55.300\n"),
        "{text}"
    );
    let back = parse(&text).unwrap();
    assert_eq!(back.times, [e.times[1], e.times[0]]);
    assert_eq!(back.to_text(), text);
    // Every older version reads as before, with no times, and is written back byte for byte.
    let older = [
        sample(),
        EditList {
            fixes: vec![Fix {
                track: 0,
                start: 2.5,
                nth: 1,
                through: Some(WordAt { start: 3.0, nth: 0 }),
                text: "Ana went".into(),
            }],
            ..sample()
        },
    ];
    for old in older {
        let text = old.to_text();
        assert!(!text.contains("time "), "{text}");
        let back = parse(&text).unwrap();
        assert!(back.times.is_empty());
        assert_eq!(back.to_text(), text);
    }
    // Refused: a second line for one word, an end not after its start, a missing number.
    let head = "junctur-edit 6\nsource x.mp4\n";
    for bad in [
        "time 0 2.000 2.100 2.000\n",
        "time 0 2.000 2.100\n",
        "time 0 2.000 2.100 x\n",
        "time x 2.000 2.100 2.300\n",
        "time 0 2.000 2.1 2.3\ntime 0 2.0004 2.0 2.4\n",
    ] {
        assert!(parse(&format!("{head}{bad}")).is_err(), "{bad}");
    }
    let ok = parse(&format!(
        "{head}time 0 2.000 2.1 2.3\ntime 0 2.000+1 2.3 2.4\n"
    ))
    .unwrap();
    assert_eq!(ok.times.len(), 2);
    // Two tracks' words of one millisecond keep their order through a round trip.
    let one_ms = EditList {
        times: vec![t(1, 5.0001, 0, 5.0, 5.2), t(0, 5.0004, 0, 5.0, 5.3)],
        ..sample()
    };
    let text = one_ms.to_text();
    assert!(
        text.contains("time 0 5.000 5.000 5.300\ntime 1 5.000 5.000 5.200\n"),
        "{text}"
    );
    assert_eq!(parse(&text).unwrap().to_text(), text);
    // A `time` line under an older header is refused, and so is a version this build does
    // not know.
    assert!(parse("junctur-edit 5\nsource x.mp4\ntime 0 2.000 2.1 2.3\n").is_err());
    assert!(parse("junctur-edit 7\nsource x.mp4\n").is_err());
}

#[test]
fn round_trips() {
    let e = sample();
    let text = e.to_text();
    assert!(text.contains("keep 1.500 4.250\n"));
    assert_eq!(parse(&text).unwrap(), e);
}

#[test]
fn hand_edits_are_normalised() {
    let text = "junctur-edit 1\n# note\nsource x.mp4\nkeep 5 8\nkeep 1 2\nkeep 7 9\n";
    let e = parse(text).unwrap();
    assert_eq!(
        e.keep,
        vec![
            Range {
                start: 1.0,
                end: 2.0
            },
            Range {
                start: 5.0,
                end: 9.0
            }
        ]
    );
}

#[test]
fn errors_name_the_line() {
    assert!(parse("nope").is_err());
    let err = parse("junctur-edit 1\nkeep 3 1\n").unwrap_err();
    assert!(err.starts_with("line 2"), "{err}");
    assert!(parse("junctur-edit 1\nfoo 1\n").is_err());
}
