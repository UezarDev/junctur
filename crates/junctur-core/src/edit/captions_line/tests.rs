use super::*;

#[test]
fn quoted_values_keep_their_spaces() {
    assert_eq!(
        tokens(r#"on font="Open Sans"  protagonist="" x=1"#).unwrap(),
        ["on", "font=Open Sans", "protagonist=", "x=1"]
    );
    assert!(tokens(r#"on font="Open"#).is_err());
}

#[test]
fn the_look_round_trips_and_defaults_are_not_written() {
    assert_eq!(
        write(&Captions::default()),
        "captions off chars=42 lines=2 seconds=6"
    );
    let c = Captions {
        on: true,
        font: "Noto Sans".into(),
        size: 6.5,
        position: 10.0,
        outline: false,
        outline_width: 4,
        outline_color: "#202020".into(),
        layout: Layout::TopBottom,
        protagonist: "Ana María".into(),
        template: "My Title".into(),
        ..Default::default()
    };
    let line = write(&c);
    assert_eq!(
        line,
        "captions on chars=42 lines=2 seconds=6 font=\"Noto Sans\" size=6.5 position=10 \
             outline=off outline_width=4 outline_color=#202020 layout=topBottom \
             protagonist=\"Ana María\" template=\"My Title\""
    );
    assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
}

#[test]
fn the_word_limit_is_written_only_when_set() {
    let c = Captions {
        on: true,
        words: 1,
        ..Default::default()
    };
    let line = write(&c);
    assert_eq!(line, "captions on chars=42 lines=2 seconds=6 words=1");
    assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
    assert_eq!(parse("on words=0").unwrap().words, 0);
    for bad in ["on words=21", "on words=-1", "on words=two"] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_text_options_round_trip_and_defaults_are_not_written() {
    let c = Captions {
        on: true,
        case: Case::Title,
        punctuation: false,
        censor: vec!["damn".into(), "what the heck".into()],
        censor_style: CensorStyle::Whole,
        replace: vec![
            Replace {
                from: "gonna".into(),
                to: "going to".into(),
            },
            Replace {
                from: "um".into(),
                to: String::new(),
            },
        ],
        ..Default::default()
    };
    let line = write(&c);
    assert_eq!(
        line,
        "captions on chars=42 lines=2 seconds=6 case=title punctuation=off \
             censor=\"damn|what the heck\" censor_style=whole replace=\"gonna>going to|um>\""
    );
    assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
    assert!(parse("on censor=\"\"").unwrap().censor.is_empty());
    assert_eq!(parse("on case=asSpoken").unwrap().case, Case::AsSpoken);
    for bad in [
        "on case=shout",
        "on punctuation=no",
        "on censor_style=all",
        "on replace=gonna",
        "on replace=\">x\"",
        "on censor=\"...\"",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_translation_is_written_only_when_set() {
    let c = Captions {
        on: true,
        translate: "fr".into(),
        ..Default::default()
    };
    let line = write(&c);
    assert_eq!(line, "captions on chars=42 lines=2 seconds=6 translate=fr");
    assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
    assert_eq!(parse("on translate=").unwrap().translate, "");
    assert!(parse("on translate=xx").is_err());
}

#[test]
fn the_highlight_and_animation_are_written_only_when_set() {
    let c = Captions {
        on: true,
        highlight: true,
        highlight_color: "#00FF00".into(),
        animation: Animation::Pop,
        animation_length: 0.35,
        ..Default::default()
    };
    let line = write(&c);
    assert_eq!(
        line,
        "captions on chars=42 lines=2 seconds=6 highlight=on highlight_color=#00FF00 \
             animation=pop animation_length=0.35"
    );
    assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
    let off = parse("on highlight=off animation=none highlight_color=").unwrap();
    assert!(!off.highlight && off.highlight_color.is_empty());
    for bad in [
        "on highlight=yes",
        "on highlight_color=green",
        "on animation=spin",
        "on animation_length=3",
        "on animation_length=quick",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn keeping_the_templates_look_is_written_only_when_on() {
    let c = Captions {
        on: true,
        template: "My Title".into(),
        keep_look: true,
        ..Default::default()
    };
    let line = write(&c);
    assert_eq!(
        line,
        "captions on chars=42 lines=2 seconds=6 template=\"My Title\" keep_look=on"
    );
    assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
    assert!(!parse("on keep_look=off").unwrap().keep_look);
    assert!(parse("on keep_look=yes").is_err());
}

#[test]
fn a_remembered_choice_for_clips_in_the_way_is_written_only_when_there_is_one() {
    // Asking (the default) is not written, so a list that never chose reads in older builds.
    let ask = Captions {
        on: true,
        ..Default::default()
    };
    assert!(!write(&ask).contains("existing_clips"));
    for (choice, word) in [
        (ExistingClips::Replace, "replace"),
        (ExistingClips::Skip, "skip"),
        (ExistingClips::NewTrack, "newTrack"),
    ] {
        let c = Captions {
            existing_clips: choice,
            ..ask.clone()
        };
        let line = write(&c);
        assert_eq!(
            line,
            format!("captions on chars=42 lines=2 seconds=6 existing_clips={word}")
        );
        assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
    }
    assert_eq!(
        parse("on existing_clips=ask").unwrap().existing_clips,
        ExistingClips::Ask
    );
    assert!(parse("on existing_clips=overwrite").is_err());
    assert!(parse("on existing_clips=new_track").is_err());
}

#[test]
fn bad_look_settings_are_refused() {
    for bad in [
        "on size=1",
        "on size=big",
        "on position=60",
        "on outline=yes",
        "on outline_width=0",
        "on outline_color=black",
        "on layout=diagonal",
        "on font=\"\"",
        "on font=\"Open",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
    // A bare one-word font needs no quotes.
    assert_eq!(parse("off font=Arial").unwrap().font, "Arial");
}
