use super::*;
use crate::edit::{SpeakerName, Speakers};
use crate::export::layout::PlacedPart;

fn edit() -> EditList {
    let name = |id: u32, n: &str, color: &str| SpeakerName {
        id,
        name: n.into(),
        track: None,
        color: color.into(),
    };
    EditList {
        source: "C:\\Videos\\stream.mp4".into(),
        duration: 100.0,
        settings: Default::default(),
        keep: Vec::new(),
        speakers: Speakers {
            names: vec![name(1, "Ana", "#FF0000"), name(2, "Bo", "#00FF80")],
            turns: Vec::new(),
        },
        captions: None,
        fixes: Vec::new(),
        times: Vec::new(),
        windows: Vec::new(),
    }
}

fn timeline(fps: f64) -> Timeline {
    Timeline {
        name: "Main".into(),
        id: "T-1".into(),
        fps,
        start: 86400,
        end: 86400 + 100 * 24,
        width: 1920,
        height: 1080,
        template: "Junctur Caption".into(),
        template_fps: 24.0,
    }
}

fn placed(start: f64, end: f64, parts: &[(u32, &str)]) -> Placed {
    Placed {
        start,
        end,
        parts: parts
            .iter()
            .enumerate()
            .map(|(i, &(speaker, text))| PlacedPart {
                speaker,
                lines: text.split('/').map(String::from).collect(),
                x: 0.5,
                y: 0.1 + i as f64 * 0.1,
                words: Vec::new(),
            })
            .collect(),
    }
}

#[test]
fn a_track_per_speaker_and_a_clip_per_part_at_integer_frames() {
    let cues = [
        placed(0.0, 1.5, &[(1, "hola/a todos")]),
        // Bo and Ana at once: one clip each, on their own tracks, Bo's above.
        placed(1.5, 3.02, &[(2, "qué tal"), (1, "bien")]),
        placed(3.02, 4.0, &[(0, "…")]),
    ];
    let j = build(&cues, &edit(), &Captions::default(), &timeline(24.0));
    assert_eq!(j.tracks, ["Junctur", "Junctur 1 Ana", "Junctur 2 Bo"]);
    let got: Vec<_> = j
        .clips
        .iter()
        .map(|c| (c.track, c.start, c.end, c.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (1, 86400 + 72, 86400 + 96, "…"),
            (2, 86400, 86400 + 36, "hola\na todos"),
            (2, 86400 + 36, 86400 + 72, "bien"),
            (3, 86400 + 36, 86400 + 72, "qué tal"),
        ]
    );
    // 3.02 s is 72.48 frames: rounded, never a fraction.
    let bo = &j.clips[3];
    assert_eq!((bo.fill, bo.y), ([0.0, 1.0, 128.0 / 255.0], 0.1));
    assert_eq!(j.clips[0].fill, [1.0, 1.0, 1.0]);
    assert_eq!(bo.key, "Junctur 2 Bo|86436|86472");
    assert_eq!(bo.length, 36);
    assert_eq!(j.recording, recording_key(&edit()));
    assert_eq!(j.recording.len(), 12);
}

#[test]
fn speakers_sharing_a_name_or_having_none_get_tracks_of_their_own() {
    let mut e = edit();
    e.speakers.names[1].name = "Ana".into();
    e.speakers.names.push(SpeakerName {
        id: 3,
        name: " ".into(),
        track: None,
        color: "#FFFFFF".into(),
    });
    let cues = [placed(0.0, 1.0, &[(1, "a"), (2, "b"), (3, "c"), (4, "d")])];
    let j = build(&cues, &e, &Captions::default(), &timeline(24.0));
    assert_eq!(
        j.tracks,
        ["Junctur 1 Ana", "Junctur 2 Ana", "Junctur 3", "Junctur 4"]
    );
    // The template is trimmed once, for both requests; the timeline `info` saw goes along.
    let c = Captions {
        template: "  My Title ".into(),
        ..Default::default()
    };
    assert_eq!(template(&c), "My Title");
    let j = build(&cues, &e, &c, &timeline(24.0));
    assert_eq!(
        (j.template.as_str(), j.timeline.as_str()),
        ("My Title", "T-1")
    );
}

#[test]
fn clips_on_a_track_never_overlap_and_lengths_are_in_template_frames() {
    // At 29.97 a 0.01 s cue rounds to nothing and a touching one rounds onto it.
    let cues = [
        placed(1.0, 1.01, &[(1, "a")]),
        placed(1.01, 2.0, &[(1, "b")]),
    ];
    let t = Timeline {
        fps: 30000.0 / 1001.0,
        template_fps: 24.0,
        ..timeline(24.0)
    };
    let j = build(&cues, &edit(), &Captions::default(), &t);
    let frames: Vec<_> = j
        .clips
        .iter()
        .map(|c| (c.start - t.start, c.end - t.start))
        .collect();
    assert_eq!(frames, [(30, 31), (31, 60)]);
    // 29 timeline frames at 29.97 are 23 frames of a 24 fps template.
    assert_eq!(j.clips[1].length, 23);
    // Past the timeline's end: not placed.
    let late = [placed(200.0, 201.0, &[(1, "x")])];
    assert!(build(&late, &edit(), &Captions::default(), &timeline(24.0))
        .clips
        .is_empty());
}

#[test]
fn the_look_maps_onto_text_plus_inputs() {
    let c = Captions {
        bold: true,
        italic: true,
        underline: true,
        size: 8.0,
        outline_width: 3,
        outline_color: "#FFFFFF".into(),
        template: "My Title".into(),
        ..Default::default()
    };
    let j = build(&[], &edit(), &c, &timeline(24.0));
    assert_eq!(j.look.style, "Bold Italic");
    assert!(j.look.underline);
    // 8 % of a 1080-high frame, measured against its 1920 width: an assumption about Text+'s
    // `Size` (Fusion sizing text by the frame width) that only a real Resolve can confirm.
    assert!((j.look.size - 0.08 * 1080.0 / 1920.0).abs() < 1e-12);
    let (color, thickness) = j.look.outline.unwrap();
    assert_eq!(color, [1.0, 1.0, 1.0]);
    assert!((thickness - 0.06).abs() < 1e-12);
    assert_eq!(j.template, "My Title");
    let vertical = Timeline {
        width: 1080,
        height: 1920,
        ..timeline(24.0)
    };
    assert!((size(&c, &vertical) - 0.08 * 1920.0 / 1080.0).abs() < 1e-12);
    let plain = build(
        &[],
        &edit(),
        &Captions {
            outline: false,
            ..Default::default()
        },
        &timeline(24.0),
    );
    assert_eq!(plain.look.style, "Regular");
    assert_eq!(plain.look.outline, None);
}

#[test]
fn the_request_holds_integers_for_frames() {
    let j = build(
        &[placed(0.0, 1.0, &[(1, "say \"hi\"")])],
        &edit(),
        &Captions::default(),
        &timeline(24.0),
    );
    let text = super::super::lua::chunk(&Value::Table(j.to_lua()));
    assert!(
        text.contains("[\"start\"] = 86400, [\"end\"] = 86424"),
        "{text}"
    );
    assert!(text.contains("[\"text\"] = \"say \\\"hi\\\"\""), "{text}");
    assert!(text.contains("[\"timeline\"] = \"T-1\""), "{text}");
    // Asking unless the recording remembered a choice.
    assert!(text.contains("[\"existing\"] = \"ask\""), "{text}");
    let new_track = Captions {
        existing_clips: ExistingClips::NewTrack,
        ..Default::default()
    };
    let j = build(
        &[placed(0.0, 1.0, &[(1, "x")])],
        &edit(),
        &new_track,
        &timeline(24.0),
    );
    assert_eq!(j.existing, ExistingClips::NewTrack);
    let text = super::super::lua::chunk(&Value::Table(j.to_lua()));
    assert!(text.contains("[\"existing\"] = \"newTrack\""), "{text}");
    assert!(
        text.contains("[\"outline\"] = true, [\"outlineColor\"] = {0.000000"),
        "{text}"
    );
}
#[test]
fn the_highlight_gives_each_clip_its_words_in_the_macros_shape() {
    use crate::edit::Animation;
    use crate::export::layout::WordSpan;
    let span = |word: usize, line: usize, from: usize, to: usize, start: f64| WordSpan {
        word,
        line,
        from,
        to,
        start,
        end: start + 0.5,
    };
    // "¿qué tal" / "todos" from 1 s, a word each as layout cuts them; "¿" and "é" are one
    // character each, as the macro's `utf8len` counts them.
    let mut cue = placed(1.0, 3.0, &[(1, "¿qué tal/todos")]);
    cue.parts[0].words = vec![
        span(0, 0, 0, 4, 1.0),
        span(1, 0, 5, 8, 1.25),
        span(2, 1, 0, 5, 1.5),
    ];
    let c = Captions {
        highlight: true,
        animation: Animation::Pop,
        animation_length: 0.3,
        ..Default::default()
    };
    let t = Timeline {
        template_fps: 48.0,
        ..timeline(24.0)
    };
    let j = build(&[cue.clone()], &edit(), &c, &t);
    assert!(j.animated);
    let clip = &j.clips[0];
    // Ana's colour lights the word; the text is white. The key says the clip is highlighted,
    // and carries its word timing's tag.
    assert_eq!(clip.base, [1.0, 1.0, 1.0]);
    assert_eq!(clip.highlight, [1.0, 0.0, 0.0]);
    // A plain Text+ (a user's own template) keeps the speaker's colour.
    assert_eq!(clip.fill, [1.0, 0.0, 0.0]);
    assert!(
        clip.key.contains("|86424|86472|highlight|w"),
        "{}",
        clip.key
    );
    // 2 s of a 48 fps template. Each later word's range starts at the separator before it (a
    // space, then a line break), and the last ends on the text's last character.
    assert_eq!(clip.length, 96);
    let got: Vec<_> = clip
        .words
        .iter()
        .map(|w| (w.from, w.to, w.start, w.end))
        .collect();
    assert_eq!(got, [(0, 3, 0, 24), (4, 7, 12, 36), (8, 13, 24, 48)]);
    assert_eq!(clip.text.chars().count(), 14);
    assert_eq!(
        (
            j.look.highlight,
            j.look.animation.as_str(),
            j.look.animation_length
        ),
        (true, "pop", 0.3)
    );
    let text = super::super::lua::chunk(&Value::Table(j.to_lua()));
    assert!(text.contains("[\"animated\"] = true"), "{text}");
    assert!(
        text.contains("{[\"startIndex\"] = 4, [\"endIndex\"] = 7, [\"startFrame\"] = 12, [\"endFrame\"] = 36}"),
        "{text}"
    );
    // The same words at other times are another key (the macro only re-keys its highlight
    // when its text changes); the same times, the same key.
    let mut moved = cue.clone();
    moved.parts[0].words[1].start = 1.3;
    assert_ne!(build(&[moved], &edit(), &c, &t).clips[0].key, clip.key);
    assert_eq!(
        build(&[cue.clone()], &edit(), &c, &t).clips[0].key,
        clip.key
    );
    // A word said before the clip (a cue pushed later) starts at its first frame; one past its
    // end is held inside it, ending before the clip's last frame; frames never go back.
    let mut late = cue.clone();
    late.parts[0].words = vec![span(0, 0, 0, 4, 0.2), span(1, 0, 5, 8, 9.0)];
    let w = &build(&[late], &edit(), &c, &t).clips[0].words;
    assert_eq!((w[0].start, w[0].end, w[1].start, w[1].end), (0, 1, 94, 95));
    // An animation alone makes the clips animated, with no word timing and the speaker's
    // colour; neither, and the key is the plain one.
    let anim = Captions {
        animation: Animation::Fade,
        ..Default::default()
    };
    let mut plain = cue.clone();
    plain.parts[0].words.clear();
    let j = build(&[plain.clone()], &edit(), &anim, &t);
    assert!(j.animated && j.clips[0].words.is_empty());
    assert_eq!(j.clips[0].base, [1.0, 0.0, 0.0]);
    // Its own kind of key: a clip that had the highlight is replaced, not restyled without it.
    assert!(
        j.clips[0].key.ends_with("|86472|animated"),
        "{}",
        j.clips[0].key
    );
    let j = build(&[plain], &edit(), &Captions::default(), &t);
    assert!(!j.animated && j.clips[0].key.ends_with("|86472"));
    assert_eq!(j.look.animation, "none");
}

#[test]
fn a_picked_template_and_its_kept_look_are_in_the_key() {
    let cues = [placed(0.0, 1.0, &[(1, "a"), (0, "b")])];
    let t = timeline(24.0);
    // Junctur's own: the plain key, the switch ignored, nothing kept.
    let own = Captions {
        keep_look: true,
        ..Default::default()
    };
    let j = build(&cues, &edit(), &own, &t);
    assert!(!j.look.keep);
    assert_eq!(j.clips[1].key, "Junctur 1 Ana|86400|86424");
    // A template of the user's: its name in the key, so picking another replaces the clips.
    let mine = Captions {
        template: " My Title ".into(),
        ..Default::default()
    };
    let j = build(&cues, &edit(), &mine, &t);
    assert!(!j.look.keep);
    assert_eq!(
        j.clips[1].key,
        "Junctur 1 Ana|86400|86424|template=My Title"
    );
    // Its look kept: said in the key too, so switching it on or off replaces them as well.
    let kept = Captions {
        keep_look: true,
        highlight: true,
        ..mine
    };
    let j = build(&cues, &edit(), &kept, &t);
    assert!(j.look.keep);
    assert_eq!(
        j.clips[1].key,
        "Junctur 1 Ana|86400|86424|highlight|template=My Title|keep"
    );
    // Ana has a colour; words with no speaker have none, so the template keeps its own fill.
    assert!(j.clips[1].colored && !j.clips[0].colored);
    let text = super::super::lua::chunk(&Value::Table(j.to_lua()));
    assert!(text.contains("[\"keep\"] = true"), "{text}");
    assert!(text.contains("[\"colored\"] = false"), "{text}");
}
