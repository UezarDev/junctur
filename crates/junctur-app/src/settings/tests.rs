use super::*;

#[test]
fn every_model_is_one_the_config_reads_and_turbo_comes_first() {
    let m = models();
    assert_eq!(m[0].key, "auto");
    for c in &m {
        assert_eq!(c.key.parse::<ModelPref>().unwrap().key(), c.key);
    }
    assert_eq!(
        m.len(),
        ModelPref::NAMES
            .iter()
            .filter(|(n, p)| p.key() == *n)
            .count()
    );
}

#[test]
fn every_model_has_its_own_name_and_an_icon_the_picker_draws() {
    let m = models();
    for (i, c) in m.iter().enumerate() {
        assert!(
            ["auto", "turbo", "large"].contains(&c.family),
            "{}",
            c.family
        );
        assert!(!c.summary.is_empty() && !c.model.is_empty());
        assert!(m[..i].iter().all(|o| o.name != c.name), "{}", c.name);
    }
    assert_eq!(m[0].name, "Automatic");
    assert_eq!(m[0].model, "large-v3-turbo");
}

#[test]
fn defaults_are_not_written_and_the_rest_round_trips() {
    let plain = from_config(&Config::default());
    assert_eq!(plain.whisper_model, "auto");
    assert_eq!(to_config(&plain).unwrap(), Config::default());
    let s = Settings {
        whisper_model: "large-v3".into(),
        captions: Captions {
            on: true,
            ..Default::default()
        },
        words: vec!["Uezar".into(), "Tsunheto".into()],
        flag_below: 25,
        labels: Default::default(),
        keep: Default::default(),
    };
    let c = to_config(&s).unwrap();
    assert_eq!(c.whisper_model, Some(ModelPref::LargeV3));
    assert_eq!(from_config(&c), s);
}

#[test]
fn a_recordings_remembered_choice_for_clips_in_resolve_never_becomes_a_default() {
    // "Save as default" from a recording that remembered "new track" saves asking.
    let s = Settings {
        whisper_model: "auto".into(),
        captions: Captions {
            on: true,
            existing_clips: ExistingClips::NewTrack,
            ..Default::default()
        },
        words: vec![],
        flag_below: config::FLAG_BELOW,
        labels: Default::default(),
        keep: Default::default(),
    };
    let saved = to_config(&s).unwrap().captions.unwrap();
    assert!(saved.on);
    assert_eq!(saved.existing_clips, ExistingClips::Ask);
    // Only the choice differed from the defaults: nothing is written.
    let only = Settings {
        captions: Captions {
            existing_clips: ExistingClips::Replace,
            ..Default::default()
        },
        ..s
    };
    assert_eq!(to_config(&only).unwrap().captions, None);
}

#[test]
fn the_word_list_is_saved_trimmed_without_blanks_or_repeats() {
    let s = Settings {
        whisper_model: "auto".into(),
        captions: Captions::default(),
        words: vec![" Uezar ".into(), "".into(), "Delay".into(), "Uezar".into()],
        flag_below: config::FLAG_BELOW,
        labels: Default::default(),
        keep: Default::default(),
    };
    assert_eq!(
        to_config(&s).unwrap().words,
        vec!["Uezar".to_string(), "Delay".to_string()]
    );
    // An empty list is not written.
    let none = Settings {
        words: vec!["  ".into()],
        ..s
    };
    assert_eq!(to_config(&none).unwrap(), Config::default());
}

#[test]
fn a_bad_model_or_out_of_limits_captions_are_refused() {
    let s = Settings {
        whisper_model: "large-v4".into(),
        captions: Captions::default(),
        words: vec![],
        flag_below: config::FLAG_BELOW,
        labels: Default::default(),
        keep: Default::default(),
    };
    assert!(to_config(&s).is_err());
    let s = Settings {
        whisper_model: "auto".into(),
        captions: Captions {
            chars: 2,
            ..Default::default()
        },
        words: vec![],
        flag_below: config::FLAG_BELOW,
        labels: Default::default(),
        keep: Default::default(),
    };
    assert!(to_config(&s).unwrap_err().contains("caption defaults"));
}

/// The flag threshold is written only off its default, 0 (none flagged) included, and a
/// percentage over 100 is refused.
#[test]
fn the_flag_threshold_is_saved_off_its_default_only() {
    let s = from_config(&Config::default());
    assert_eq!(s.flag_below, config::FLAG_BELOW);
    assert_eq!(to_config(&s).unwrap().flag_below, None);
    let off = Settings {
        flag_below: 0,
        ..s.clone()
    };
    assert_eq!(to_config(&off).unwrap().flag_below, Some(0));
    assert_eq!(from_config(&to_config(&off).unwrap()).flag_below, 0);
    let bad = Settings {
        flag_below: 101,
        ..s.clone()
    };
    assert!(to_config(&bad).is_err());
    // Settings sent without it: the default.
    let mut v = serde_json::to_value(Settings { flag_below: 7, ..s }).unwrap();
    v.as_object_mut().unwrap().remove("flagBelow").unwrap();
    let old: Settings = serde_json::from_value(v).unwrap();
    assert_eq!(old.flag_below, config::FLAG_BELOW);
}

/// Labels round-trip, off by default and not written then; added ones are trimmed, once each.
#[test]
fn labels_settings_round_trip_and_added_ones_are_cleaned() {
    let plain = from_config(&Config::default());
    assert!(!plain.labels.on);
    assert_eq!(
        to_config(&plain).unwrap().labels,
        labels::Settings::default()
    );
    let on = Settings {
        labels: labels::Settings {
            on: true,
            kind: Kind::League,
            added: labels::Added {
                generic: vec![],
                league: vec![" baron ".into(), "".into(), "Baron".into(), "drake".into()],
            },
        },
        ..plain
    };
    let c = to_config(&on).unwrap();
    assert!(c.labels.on);
    assert_eq!(c.labels.added.league, vec!["baron", "drake"]);
    assert_eq!(from_config(&c).labels, c.labels);
    // The dialog's kinds are every kind, each with its built-in labels.
    let kinds = label_kinds();
    assert_eq!(kinds.len(), Kind::ALL.len());
    assert!(kinds.iter().all(|k| !k.builtin.is_empty()));
    let v = serde_json::to_value(&kinds[1]).unwrap();
    assert_eq!(v["key"], "league");
}

/// Smart keep round-trips, off by default and not written then; a blank last prompt is none.
#[test]
fn smart_keep_settings_round_trip_off_by_default() {
    let plain = from_config(&Config::default());
    assert!(!plain.keep.on);
    assert_eq!(to_config(&plain).unwrap(), Config::default());
    let on = Settings {
        keep: keep::Settings {
            on: true,
            last: Some(" League highlights ".into()),
        },
        ..plain.clone()
    };
    let c = to_config(&on).unwrap();
    assert!(c.keep.on);
    assert_eq!(c.keep.last.as_deref(), Some("League highlights"));
    assert_eq!(from_config(&c).keep, c.keep);
    let blank = Settings {
        keep: keep::Settings {
            on: false,
            last: Some("  ".into()),
        },
        ..plain
    };
    assert_eq!(to_config(&blank).unwrap(), Config::default());
}
