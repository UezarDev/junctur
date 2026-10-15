use super::*;

#[test]
fn colours_and_limits() {
    assert_eq!(parse_color("#a1b2c3").as_deref(), Some("#A1B2C3"));
    assert_eq!(parse_color("a1b2c3"), None);
    assert_eq!(parse_color("#a1b2c"), None);
    assert_eq!(parse_color("#a1b2cg"), None);
    assert_eq!(default_color(1), "#F2B33D");
    assert_eq!(default_color(9), default_color(1));
    assert!(Captions::default().check().is_ok());
    let bad = |c: Captions| c.check().is_err();
    assert!(bad(Captions {
        chars: 5,
        ..Default::default()
    }));
    assert!(bad(Captions {
        lines: 0,
        ..Default::default()
    }));
    assert!(bad(Captions {
        words: 21,
        ..Default::default()
    }));
    assert!(Captions {
        words: 1,
        ..Default::default()
    }
    .check()
    .is_ok());
    assert!(bad(Captions {
        seconds: 30.0,
        ..Default::default()
    }));
    // The look.
    for c in [
        Captions {
            size: 1.0,
            ..Default::default()
        },
        Captions {
            position: 50.0,
            ..Default::default()
        },
        Captions {
            outline_width: 0,
            ..Default::default()
        },
        Captions {
            outline_color: "black".into(),
            ..Default::default()
        },
        Captions {
            font: " ".into(),
            ..Default::default()
        },
        Captions {
            protagonist: "Ana \"A\"".into(),
            ..Default::default()
        },
        Captions {
            template: "a\nb".into(),
            ..Default::default()
        },
        // A `|` would let two clip tags read alike ("x|keep" with the look not kept,
        // "x" with it kept).
        Captions {
            template: "x|keep".into(),
            ..Default::default()
        },
    ] {
        assert!(bad(c));
    }
}

#[test]
fn the_protagonist_is_found_by_name() {
    let name = |id: u32, name: &str| super::super::SpeakerName {
        id,
        name: name.into(),
        track: None,
        color: "#FFFFFF".into(),
    };
    let names = [name(1, "Ana"), name(2, "Lucía")];
    let c = |p: &str| Captions {
        protagonist: p.into(),
        ..Default::default()
    };
    assert_eq!(c("ana").protagonist_in(&names), Some(1));
    assert_eq!(c(" LUCÍA ").protagonist_in(&names), Some(2));
    assert_eq!(c("Bo").protagonist_in(&names), None);
    assert_eq!(c("").protagonist_in(&names), None);
}

#[test]
fn settings_saved_before_a_field_existed_still_load() {
    // The first four fields only, as an older build's caption defaults would hold them.
    let old: Captions =
        serde_json::from_str(r#"{"on":true,"chars":30,"lines":1,"seconds":4}"#).unwrap();
    assert_eq!(
        old,
        Captions {
            on: true,
            chars: 30,
            lines: 1,
            seconds: 4.0,
            ..Default::default()
        }
    );
    assert!(!old.preview && !old.bold && !old.italic && !old.underline);
    let all: Captions = serde_json::from_str(
        r#"{"on":false,"preview":true,"chars":42,"lines":2,"seconds":6,"bold":true,"italic":true}"#,
    )
    .unwrap();
    assert!(all.preview && all.bold && all.italic && !all.underline);
    // The underline beside them.
    let u: Captions = serde_json::from_str(r#"{"underline":true}"#).unwrap();
    assert!(u.underline && !u.bold);
    // Settings without the look load it at its defaults; its own fields read back.
    assert_eq!(u.font, "Open Sans");
    assert_eq!((u.size, u.position, u.outline), (4.5, 6.0, true));
    assert_eq!(u.layout, Layout::Stacked);
    let g: Captions = serde_json::from_str(
        r##"{"font":"Noto Sans","size":6,"outlineWidth":3,"outlineColor":"#202020","layout":"topBottom","protagonist":"Ana","template":"My Title"}"##,
    )
    .unwrap();
    assert_eq!(g.layout, Layout::TopBottom);
    assert_eq!((g.size, g.outline_width), (6.0, 3));
    assert_eq!(
        (g.protagonist.as_str(), g.template.as_str()),
        ("Ana", "My Title")
    );
    assert!(serde_json::from_str::<Captions>(r#"{"layout":"diagonal"}"#).is_err());
    // Without a word limit, it is off; its own value reads back.
    assert_eq!(g.words, 0);
    let i: Captions = serde_json::from_str(r#"{"words":1}"#).unwrap();
    assert_eq!(i.words, 1);
    // Without text options, the text is as spoken; their own fields read back.
    assert_eq!((i.case, i.punctuation), (Case::AsSpoken, true));
    assert!(i.censor.is_empty() && i.replace.is_empty());
    let j: Captions = serde_json::from_str(
        r#"{"case":"title","punctuation":false,"censor":["damn"],"censorStyle":"whole","replace":[{"from":"gonna","to":"going to"}]}"#,
    )
    .unwrap();
    assert_eq!(
        (j.case, j.punctuation, j.censor_style),
        (Case::Title, false, CensorStyle::Whole)
    );
    assert_eq!(j.censor, ["damn"]);
    assert_eq!(j.replace[0].to, "going to");
    assert!(j.check().is_ok());
    let bad: Captions = serde_json::from_str(r#"{"censor":["a|b"]}"#).unwrap();
    assert!(bad.check().is_err());
    // Without a language, captions are untranslated; its own value reads back and is checked.
    assert_eq!(j.translate, "");
    let k: Captions = serde_json::from_str(r#"{"translate":"fr"}"#).unwrap();
    assert_eq!(k.translate, "fr");
    assert!(k.check().is_ok());
    let bad: Captions = serde_json::from_str(r#"{"translate":"klingon"}"#).unwrap();
    assert!(bad.check().is_err());
    // Without them, highlight and animation are off; their own fields read back.
    assert!(!k.highlight && !k.animated());
    assert_eq!((k.animation, k.animation_length), (Animation::None, 0.2));
    let l: Captions = serde_json::from_str(
        r##"{"highlight":true,"highlightColor":"#00FF00","animation":"slide","animationLength":0.4}"##,
    )
    .unwrap();
    assert!(l.highlight && l.animated() && l.check().is_ok());
    assert_eq!(
        (l.highlight_color.as_str(), l.animation, l.animation_length),
        ("#00FF00", Animation::Slide, 0.4)
    );
    // Without it, the keep-look switch is off; its own value reads back, and counts only
    // with a template named.
    assert!(!l.keep_look && !l.keeps_look());
    let m: Captions = serde_json::from_str(r#"{"keepLook":true}"#).unwrap();
    assert!(m.keep_look && !m.keeps_look());
    let m: Captions = serde_json::from_str(r#"{"keepLook":true,"template":" My Title "}"#).unwrap();
    assert!(m.keeps_look());
    assert!(serde_json::from_str::<Captions>(r#"{"animation":"spin"}"#).is_err());
    // Without a choice for clips in the way, it asks; its own value reads back.
    assert_eq!(m.existing_clips, ExistingClips::Ask);
    let n: Captions = serde_json::from_str(r#"{"existingClips":"newTrack"}"#).unwrap();
    assert_eq!(n.existing_clips, ExistingClips::NewTrack);
    assert!(serde_json::from_str::<Captions>(r#"{"existingClips":"merge"}"#).is_err());
}

#[test]
fn the_highlight_colours_follow_the_speaker_unless_one_is_picked() {
    let mut c = Captions::default();
    // The speaker's colour lights the word; the text is white.
    assert_eq!(
        c.highlight_colors("#f2b33d"),
        ("#FFFFFF".into(), "#F2B33D".into())
    );
    // No colour (or white): a yellow word, not white on white.
    assert_eq!(c.highlight_colors(""), ("#FFFFFF".into(), "#FFD400".into()));
    assert_eq!(c.highlight_colors("#FFFFFF").1, "#FFD400");
    // A picked colour lights the word over the speaker's colour.
    c.highlight_color = "#00ff00".into();
    assert_eq!(
        c.highlight_colors("#F2B33D"),
        ("#F2B33D".into(), "#00FF00".into())
    );
    let bad = |c: Captions| c.check().is_err();
    assert!(bad(Captions {
        highlight_color: "green".into(),
        ..Default::default()
    }));
    assert!(bad(Captions {
        animation_length: 2.0,
        ..Default::default()
    }));
    assert!(Captions {
        animation: Animation::Fade,
        ..Default::default()
    }
    .animated());
}
