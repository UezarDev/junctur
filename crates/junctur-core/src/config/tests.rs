use super::*;

#[test]
fn reads_models_and_aliases() {
    let c = parse(br#"{"whisper_model":"large-v3"}"#).unwrap();
    assert_eq!(c.whisper_model, Some(ModelPref::LargeV3));
    let c = parse(br#"{"whisper_model":"turbo"}"#).unwrap();
    assert_eq!(c.whisper_model, Some(ModelPref::LargeV3Turbo));
    assert_eq!(parse(b"{}").unwrap(), Config::default());
}

#[test]
fn skips_a_byte_order_mark() {
    let c = parse(b"\xEF\xBB\xBF{\"whisper_model\":\"best\"}").unwrap();
    assert_eq!(c.whisper_model, Some(ModelPref::LargeV3));
}

#[test]
fn typos_are_errors() {
    assert!(parse(br#"{"whisper_model":"large-v4"}"#).is_err());
    assert!(parse(br#"{"whisper_modle":"large-v3"}"#).is_err());
    // `models_dir` is not supported: models live in the cache folder.
    assert!(parse(br#"{"models_dir":"D:/m"}"#).is_err());
}

#[test]
fn model_flag_beats_config_beats_auto() {
    let cfg = |m: Option<ModelPref>| {
        move || {
            Ok(Config {
                whisper_model: m,
                ..Default::default()
            })
        }
    };
    let broken = || Err(Error::Config("bad config".into()));
    assert_eq!(
        whisper_model(Some("turbo"), cfg(Some(ModelPref::LargeV3))).unwrap(),
        ModelPref::LargeV3Turbo
    );
    // An explicit choice never reads the config, so a broken one cannot block it.
    assert_eq!(
        whisper_model(Some("best"), broken).unwrap(),
        ModelPref::LargeV3
    );
    assert_eq!(
        whisper_model(None, cfg(Some(ModelPref::LargeV3Q5))).unwrap(),
        ModelPref::LargeV3Q5
    );
    assert_eq!(whisper_model(None, cfg(None)).unwrap(), ModelPref::Auto);
    assert!(whisper_model(None, broken).is_err());
    assert!(whisper_model(Some("large-v4"), cfg(None)).is_err());
}

#[test]
fn caption_defaults_are_read_checked_and_resolved() {
    let c = parse(br#"{"captions":{"on":true,"chars":30}}"#).unwrap();
    // A field the file leaves out (as an older file lacks a newer one) is the built-in value.
    let want = Captions {
        on: true,
        chars: 30,
        ..Default::default()
    };
    assert_eq!(c.captions, Some(want.clone()));
    assert!(
        parse(br#"{"captions":{"chars":500}}"#).is_err(),
        "out of limits"
    );
    let e = parse(br#"{"captions":{"chrs":30}}"#).unwrap_err();
    assert!(
        e.contains("chrs"),
        "a misspelt caption setting is an error: {e}"
    );
    let broken = || Err(Error::Config("bad config".into()));
    let own = Captions {
        lines: 1,
        ..Default::default()
    };
    // A list's own settings win, and never read the config.
    assert_eq!(captions(Some(own.clone()), broken).unwrap(), own);
    assert_eq!(captions(None, || Ok(c.clone())).unwrap(), want);
    assert_eq!(
        captions(None, || Ok(Config::default())).unwrap(),
        Captions::default()
    );
    assert!(captions(None, broken).is_err());
}

#[test]
fn saves_what_it_reads_with_canonical_names() {
    let d = std::env::temp_dir().join(format!("junctur-config-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let path = d.join("junctur").join("config.json");
    let c = parse(br#"{"whisper_model":"best"}"#).unwrap();
    save_to(&path, &c).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(r#""whisper_model": "large-v3""#), "{text}");
    assert!(
        !text.contains("captions") && !text.contains("keep"),
        "unset keys are not written: {text}"
    );
    assert_eq!(load_from(&path, true).unwrap(), c);
    let c = Config {
        whisper_model: None,
        captions: Some(Captions {
            on: true,
            ..Default::default()
        }),
        words: vec!["Uezar".into(), "Tsunheto".into()],
        flag_below: Some(0),
        labels: crate::labels::Settings {
            on: true,
            kind: crate::labels::Kind::League,
            added: crate::labels::Added {
                league: vec!["baron fight".into()],
                ..Default::default()
            },
        },
        keep: crate::keep::Settings {
            on: true,
            last: Some("League highlights".into()),
        },
        cut_up_front: Some(true),
    };
    save_to(&path, &c).unwrap();
    assert_eq!(load_from(&path, true).unwrap(), c);
    let _ = std::fs::remove_dir_all(&d);
}

/// The flag threshold is read, defaults when unset, and is not written unset.
#[test]
fn the_flag_threshold_reads_and_defaults() {
    assert_eq!(parse(b"{}").unwrap().flag_below(), FLAG_BELOW);
    let c = parse(br#"{"flag_below":25}"#).unwrap();
    assert_eq!((c.flag_below, c.flag_below()), (Some(25), 25));
    assert_eq!(parse(br#"{"flag_below":0}"#).unwrap().flag_below(), 0);
    assert_eq!(parse(br#"{"flag_below":250}"#).unwrap().flag_below(), 100);
    // Past a byte too: the rest of the file still reads.
    let c = parse(br#"{"flag_below":300,"words":["Uezar"]}"#).unwrap();
    assert_eq!((c.flag_below(), c.words.len()), (100, 1));
    assert!(parse(br#"{"flag_below":-1}"#).is_err());
    assert!(parse(br#"{"flag_below":"40"}"#).is_err());
    assert!(!serde_json::to_string(&Config::default())
        .unwrap()
        .contains("flag_below"));
}

#[test]
fn the_word_list_is_read_and_goes_with_the_model() {
    let c = parse(br#"{"words":["Uezar","Delay"]}"#).unwrap();
    assert_eq!(c.words, vec!["Uezar".to_string(), "Delay".to_string()]);
    assert!(
        parse(br#"{"words":"Uezar"}"#).is_err(),
        "a list, not a string"
    );
    let ok = || Ok(c.clone());
    let broken = || Err(Error::Config("bad config".into()));
    assert_eq!(
        transcription(None, ok()).unwrap(),
        (ModelPref::Auto, c.words.clone())
    );
    assert_eq!(
        transcription(Some("best"), ok()).unwrap(),
        (ModelPref::LargeV3, c.words.clone())
    );
    // An explicit model is not blocked by a broken file; its list is then not known.
    assert_eq!(
        transcription(Some("best"), broken()).unwrap(),
        (ModelPref::LargeV3, vec![])
    );
    assert!(transcription(None, broken()).is_err());
    assert!(transcription(Some("large-v4"), ok()).is_err());
}

#[test]
fn where_the_file_is() {
    let base = PathBuf::from("cfg").join("junctur");
    assert_eq!(
        locate(None, Some(base.clone())).unwrap(),
        Some((base.join("config.json"), false))
    );
    assert_eq!(locate(None, None).unwrap(), None);
    assert_eq!(
        locate(Some("x.json".into()), Some(base)).unwrap(),
        Some((PathBuf::from("x.json"), true))
    );
    assert!(locate(Some(OsString::new()), None).is_err());
}

#[test]
fn missing_default_is_defaults_but_missing_named_file_and_broken_file_are_errors() {
    let d = std::env::temp_dir().join(format!("junctur-config-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let none = d.join("none.json");
    assert_eq!(load_from(&none, false).unwrap(), Config::default());
    let e = load_from(&none, true).unwrap_err().to_string();
    assert!(e.contains("none.json"), "{e}");
    let bad = d.join("bad.json");
    std::fs::write(&bad, "{ not json").unwrap();
    let e = load_from(&bad, false).unwrap_err().to_string();
    assert!(e.contains("bad.json"), "{e}");
    // Any other read failure (here: a folder) names the path too.
    let e = load_from(&d, false).unwrap_err().to_string();
    assert!(e.contains("junctur-config-"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Labels are off unless the file turns them on, and not written while off and empty.
#[test]
fn labels_are_off_unless_set_and_written_only_when_set() {
    assert!(!parse(b"{}").unwrap().labels.on);
    let c = parse(br#"{"labels":{"on":true,"kind":"league"}}"#).unwrap();
    assert!(c.labels.on);
    assert_eq!(c.labels.kind, crate::labels::Kind::League);
    assert!(parse(br#"{"labels":{"on":true,"kind":"chess"}}"#).is_err());
    assert!(!serde_json::to_string(&Config::default())
        .unwrap()
        .contains("labels"));
}
