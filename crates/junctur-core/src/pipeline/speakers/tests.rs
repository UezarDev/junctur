use super::*;
use crate::pipeline::SpeechTrack;
use crate::test_support::scratch;
use crate::vad::Region;

#[test]
fn several_speech_tracks_are_one_speaker_each() {
    let track = |track, regions: Vec<(f64, f64)>| SpeechTrack {
        track,
        regions: regions
            .into_iter()
            .map(|(start, end)| Region { start, end })
            .collect(),
        share: 0.5,
    };
    let (a, b) = (
        track(0, vec![(0.0, 2.0), (5.0, 6.0)]),
        track(2, vec![(1.0, 3.0)]),
    );
    let f = by_track(&[&a, &b]);
    assert_eq!(f.method, Method::Tracks);
    assert_eq!(f.tracks, vec![Some(0), Some(2)]);
    let ids: Vec<u32> = f.turns.iter().map(|t| t.speaker).collect();
    assert_eq!(ids, vec![1, 2, 1]);
}

#[test]
fn no_speech_no_speakers() {
    let speech = Speech {
        tracks: vec![],
        seconds: 10.0,
    };
    let f = find(
        Path::new("missing.mp4"),
        &speech,
        &[],
        &[],
        &[],
        Path::new("."),
        Path::new("."),
        &mut |_, _| panic!("nothing to hear"),
    )
    .unwrap();
    assert_eq!(f.method, Method::None);
    assert!(f.turns.is_empty());
    assert_eq!(f.speech, speech_key(&speech));
}

#[test]
fn pins_survive_every_plan() {
    let pins = vec![Pin {
        start: 1.0,
        end: 2.0,
        name: "Ana".into(),
    }];
    let track = |track| SpeechTrack {
        track,
        regions: vec![Region {
            start: 0.0,
            end: 3.0,
        }],
        share: 0.5,
    };
    for (tracks, voice) in [(vec![], vec![]), (vec![track(1), track(2)], vec![1, 2])] {
        let speech = Speech {
            tracks,
            seconds: 10.0,
        };
        let f = find(
            Path::new("missing.mp4"),
            &speech,
            &voice,
            &[],
            &pins,
            Path::new("."),
            Path::new("."),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(f.pins, pins, "{:?}", f.method);
    }
}

#[test]
fn a_mix_is_never_a_speaker_beside_voice_tracks() {
    let track = |track| SpeechTrack {
        track,
        regions: vec![Region {
            start: track as f64,
            end: track as f64 + 1.0,
        }],
        share: 0.5,
    };
    let speech = |ts: &[usize]| Speech {
        tracks: ts.iter().map(|&t| track(t)).collect(),
        seconds: 10.0,
    };
    // OBS: mix on 0, mic on 1, Discord on 2. The voices are the speakers.
    let s = speech(&[0, 1, 2]);
    let Plan::ByTrack(v) = plan(&s, &[1, 2]) else {
        panic!("by track")
    };
    assert_eq!(v.iter().map(|t| t.track).collect::<Vec<_>>(), vec![1, 2]);
    // Mix plus one voice: told apart by voice on the mix, which holds everyone.
    let s = speech(&[0, 1]);
    assert_eq!(plan(&s, &[1]), Plan::Diarize(&s.tracks[0]));
    // One voice track alone, or only a mix: that track.
    let s = speech(&[3]);
    assert_eq!(plan(&s, &[3]), Plan::Diarize(&s.tracks[0]));
    assert_eq!(plan(&s, &[]), Plan::Diarize(&s.tracks[0]));
    assert_eq!(plan(&speech(&[]), &[]), Plan::Nothing);
    // A changed region makes another key.
    let mut moved = speech(&[0, 1]);
    moved.tracks[1].regions[0].end += 0.5;
    assert_ne!(speech_key(&speech(&[0, 1])), speech_key(&moved));
}

#[test]
fn the_hearing_key_says_the_windows_were_embedded_on_the_cpu() {
    // Without it, windows made on the GPU (not CAM++'s numbers) would be reused.
    assert!(diarize::hearing_settings().ends_with(" emb=cpu"));
}

fn unit(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

fn voice(name: &str, print: Vec<f32>) -> Voice {
    Voice {
        name: name.into(),
        color: "#123456".into(),
        seconds: 30.0,
        model: crate::voices::print_model(),
        print,
    }
}

/// Ana (axis 0) talks 0-15 s, Bea (axis 1) 15-30 s, in 1.5 s windows; overlap stretches given.
fn heard(overlaps: Vec<Overlap>) -> (Found, Vec<Vec<f32>>) {
    let windows: Vec<Region> = (0..20)
        .map(|i| Region {
            start: i as f64 * 1.5,
            end: i as f64 * 1.5 + 1.5,
        })
        .collect();
    let e: Vec<Vec<f32>> = (0..20)
        .map(|i| {
            unit(if i < 10 {
                &[1.0, 0.1, 0.0]
            } else {
                &[0.1, 1.0, 0.0]
            })
        })
        .collect();
    let bytes: Vec<u8> = e.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let found = Found {
        method: Method::Voices,
        tracks: vec![],
        people: vec![],
        turns: vec![],
        speech: "k".into(),
        heard: Some(Heard {
            track: 0,
            windows,
            overlaps,
            key: hearing_key(0),
            dim: 3,
            embeddings: format!("{:016x}", fnv(&bytes)),
        }),
        known: vec![],
        pins: vec![],
    };
    (found, e)
}

#[test]
fn voices_name_speakers_and_an_overlap_gives_both_people_a_turn() {
    // 5-6 s both talk: local a talks alone 3-5 s (Ana's time), local b 16-18 s (Bea's).
    let o = Overlap {
        start: 5.0,
        end: 6.0,
        a: vec![[3.0, 5.0]],
        b: vec![[16.0, 18.0]],
    };
    let (found, e) = heard(vec![o.clone()]);
    let voices = [
        voice("Bea", unit(&[0.0, 1.0, 0.0])),
        voice("Ana", unit(&[1.0, 0.0, 0.0])),
    ];
    let f = label(&found, &e, &voices);
    // Bea is speaker 1 (the voices' order), Ana speaker 2.
    let names: Vec<_> = f
        .people
        .iter()
        .map(|p| p.as_ref().map(|p| p.name.as_str()))
        .collect();
    assert_eq!(names, vec![Some("Bea"), Some("Ana")]);
    assert_eq!(f.tracks, vec![None, None]);
    let at = |t: f64| -> Vec<u32> {
        f.turns
            .iter()
            .filter(|x| x.start <= t && t < x.end)
            .map(|x| x.speaker)
            .collect()
    };
    assert_eq!(at(2.0), vec![2]);
    let mut both = at(5.5);
    both.sort();
    both.dedup();
    assert_eq!(both, vec![1, 2]);
    assert_eq!(at(20.0), vec![1]);
    // No voices: plain cluster numbering, and the same overlap turns.
    let plain = label(&found, &e, &[]);
    assert_eq!(plain.people, vec![None, None]);
    assert_eq!(plain.turns.len(), f.turns.len());
    // One local speaker with no evidence: the other is the nearest other speaker. None
    // near: no overlap turn.
    let lone = Overlap {
        start: 13.5,
        end: 14.5,
        a: vec![[10.0, 13.0]],
        b: vec![],
    };
    let (found, e) = heard(vec![lone]);
    let l = label(&found, &e, &voices);
    assert_eq!(l.turns.len(), 4, "{:?}", l.turns);
    assert!(l.turns.iter().any(|t| t.start == 13.5 && t.speaker == 1));
    let far = Overlap {
        start: 2.0,
        end: 3.0,
        a: vec![[0.0, 2.0]],
        b: vec![],
    };
    let (found, e) = heard(vec![far]);
    assert_eq!(
        label(&found, &e, &voices).turns.len(),
        2,
        "Ana, then Bea: no overlap turn"
    );
    // Neither local speaker talks alone: the windows' speaker there and the nearest other.
    let blind = Overlap {
        start: 14.0,
        end: 14.6,
        a: vec![],
        b: vec![],
    };
    let (found, e) = heard(vec![blind]);
    let l = label(&found, &e, &voices);
    let mut at: Vec<u32> = l
        .turns
        .iter()
        .filter(|t| t.start == 14.0)
        .map(|t| t.speaker)
        .collect();
    at.sort();
    assert_eq!(at, vec![1, 2]);
}

#[test]
fn a_voice_sample_is_the_chosen_windows_without_overlapping_talk() {
    let d = scratch("speakers", "sample");
    let o = Overlap {
        start: 3.2,
        end: 3.4,
        a: vec![],
        b: vec![],
    };
    let (found, e) = heard(vec![o]);
    save_embeddings(&d, &e).unwrap();
    // Windows 0-1.5, 1.5-3, 3-4.5 (touches the overlap), 4.5-6.
    let (sample, secs) = voice_sample(&found, &d, &[(0.0, 6.0)]).unwrap();
    assert_eq!(sample.len(), 3);
    assert!((secs - 4.5).abs() < 1e-9);
    // Relabelling reads the same embeddings back.
    let again = relabel(&found, &d, &[]).unwrap();
    assert_eq!(again, label(&found, &e, &[]));
    assert_eq!(
        cached(
            found.clone(),
            &Speech {
                tracks: vec![],
                seconds: 0.0
            },
            &[],
            &d
        ),
        None
    );
    // The same windows in a cached result for this speech: reused with their embeddings.
    let speech = Speech {
        tracks: vec![SpeechTrack {
            track: 0,
            regions: vec![Region {
                start: 0.0,
                end: 30.0,
            }],
            share: 0.5,
        }],
        seconds: 30.0,
    };
    let mine = Found {
        speech: speech_key(&speech),
        ..found.clone()
    };
    assert!(cached(mine.clone(), &speech, &[], &d).is_some());
    // Not when the track is now a voice track of its own (another plan), or after other
    // settings or models.
    assert!(
        cached(mine.clone(), &speech, &[0, 1], &d).is_some(),
        "one voice: still diarized"
    );
    assert!(!mine.heard_by_an_older_junctur());
    let mut other = mine.clone();
    other.heard.as_mut().unwrap().key = "old".into();
    assert!(other.heard_by_an_older_junctur());
    assert!(cached(other.clone(), &speech, &[], &d).is_none());
    // Nor are windows another hearing made (maybe on the GPU) labelled or sampled from.
    let err = relabel(&other, &d, &[]).unwrap_err().to_string();
    assert!(err.contains("older Junctur"), "{err}");
    assert!(voice_sample(&other, &d, &[(0.0, 6.0)]).is_err());
    // A windows.f32 of the same size from another run is not these windows.
    let mut swapped = e.clone();
    swapped.swap(0, 19);
    save_embeddings(&d, &swapped).unwrap();
    assert!(cached(mine, &speech, &[], &d).is_none());
    assert!(relabel(&found, &d, &[]).is_err());
    save_embeddings(&d, &e).unwrap();
    // Speakers that are tracks have no voice to sample.
    let tracks = Found {
        method: Method::Tracks,
        heard: None,
        ..found.clone()
    };
    assert!(voice_sample(&tracks, &d, &[(0.0, 6.0)]).is_err());
    // A file of another size is not these windows.
    std::fs::write(d.join(EMBEDDINGS_FILE), [0u8; 8]).unwrap();
    assert!(relabel(&found, &d, &[]).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

fn speakers_at(f: &Found, t: f64) -> Vec<u32> {
    let mut s: Vec<u32> = f
        .turns
        .iter()
        .filter(|x| x.start <= t && t < x.end)
        .map(|x| x.speaker)
        .collect();
    s.sort();
    s.dedup();
    s
}

fn pins(f: &Found) -> Vec<(f64, f64, &str)> {
    f.pins
        .iter()
        .map(|p| (p.start, p.end, p.name.as_str()))
        .collect()
}

#[test]
fn a_newer_pin_takes_its_seconds_and_one_persons_pins_join() {
    let (mut f, _) = heard(vec![]);
    f.pin("Ana", &[(2.0, 5.0)]);
    f.pin("Bea", &[(4.0, 6.0), (10.0, 11.0)]);
    assert_eq!(
        pins(&f),
        vec![(2.0, 4.0, "Ana"), (4.0, 6.0, "Bea"), (10.0, 11.0, "Bea")]
    );
    // Ana again over the middle of Bea's: Bea keeps what is left either side; Ana's pins
    // are found whatever the case of the name, and take the name as given (enrolling gives
    // the voice's own).
    f.pin("ana", &[(4.5, 5.5)]);
    assert_eq!(
        pins(&f),
        vec![
            (2.0, 4.0, "ana"),
            (4.0, 4.5, "Bea"),
            (4.5, 5.5, "ana"),
            (5.5, 6.0, "Bea"),
            (10.0, 11.0, "Bea")
        ]
    );
    f.pin("Ana", &[(3.5, 4.5)]);
    assert_eq!(pins(&f)[0], (2.0, 5.5, "Ana"));
    assert_eq!(f.pins.len(), 3);
    f.rename_pins("ANA", "Anita");
    assert_eq!(pins(&f)[0], (2.0, 5.5, "Anita"));
    // An empty stretch, or a sliver under a millisecond, pins nothing.
    let (mut g, _) = heard(vec![]);
    g.pin("Ana", &[(3.0, 3.0), (5.0, 5.0005)]);
    assert!(g.pins.is_empty());
}

#[test]
fn pinned_seconds_are_the_pinned_person_and_nobody_else() {
    // Bea is pinned over 3-6 s, where the windows hear Ana, and where an overlap stretch
    // would give both a turn.
    let o = Overlap {
        start: 4.0,
        end: 5.0,
        a: vec![[0.0, 3.0]],
        b: vec![[16.0, 18.0]],
    };
    let (mut found, e) = heard(vec![o]);
    let voices = [
        voice("Ana", unit(&[1.0, 0.0, 0.0])),
        voice("Bea", unit(&[0.0, 1.0, 0.0])),
    ];
    assert_eq!(speakers_at(&label(&found, &e, &voices), 4.5), vec![1, 2]);
    found.pin("Bea", &[(3.0, 6.0)]);
    let f = label(&found, &e, &voices);
    assert_eq!(f.pins, found.pins, "the pins are kept");
    for t in [3.0, 3.5, 4.5, 5.9] {
        assert_eq!(speakers_at(&f, t), vec![2], "at {t}: {:?}", f.turns);
    }
    assert_eq!(speakers_at(&f, 2.9), vec![1]);
    assert_eq!(speakers_at(&f, 6.0), vec![1]);
    assert_eq!(speakers_at(&f, 20.0), vec![2]);
    // Turns of window labels never overlap each other, pins included.
    let mut solo: Vec<&Turn> = f.turns.iter().collect();
    solo.sort_by(|a, b| a.start.total_cmp(&b.start));
    assert!(solo.windows(2).all(|w| w[0].end <= w[1].start + 1e-9));
    // A pin of a name no voice has is ignored, and kept.
    let mut odd = found.clone();
    odd.pin("Nobody", &[(20.0, 21.0)]);
    let f = label(&odd, &e, &voices);
    assert_eq!(speakers_at(&f, 20.5), vec![2]);
    assert_eq!(f.pins.len(), 2);
    // A pinned voice no window sounds like still gets a speaker, after the others.
    let cid = voice("Cid", unit(&[0.0, 0.0, 1.0]));
    let three = [voices[0].clone(), voices[1].clone(), cid];
    let mut c = found.clone();
    c.pin("Cid", &[(20.0, 21.0)]);
    let f = label(&c, &e, &three);
    let names: Vec<_> = f
        .people
        .iter()
        .map(|p| p.as_ref().map(|p| p.name.as_str()))
        .collect();
    assert_eq!(names, vec![Some("Ana"), Some("Bea"), Some("Cid")]);
    assert_eq!(speakers_at(&f, 20.5), vec![3]);
    assert_eq!(speakers_at(&f, 21.5), vec![2]);
}

#[test]
fn a_short_selection_takes_the_clear_windows_around_it() {
    let d = scratch("speakers", "short");
    let o = Overlap {
        start: 3.2,
        end: 3.4,
        a: vec![],
        b: vec![],
    };
    let (found, e) = heard(vec![o]);
    save_embeddings(&d, &e).unwrap();
    // One word, 3.3-3.6, in a window whose middle is outside it and which touches the
    // overlap (two people), and a word where hearing made no window: nothing, the caller
    // embeds the word's own audio.
    for word in [(3.3, 3.6), (31.0, 31.4)] {
        let (sample, secs) = voice_sample(&found, &d, &[word]).unwrap();
        assert!(sample.is_empty() && secs == 0.0, "{word:?}");
    }
    // A word across two windows clear of overlaps: both, by the seconds each covers.
    let (sample, secs) = voice_sample(&found, &d, &[(5.8, 6.2)]).unwrap();
    assert_eq!(sample, vec![e[3].clone(), e[4].clone()]);
    assert!((secs - 0.4).abs() < 1e-9, "{secs}");
    // A longer pick still takes its clean windows.
    assert_eq!(voice_sample(&found, &d, &[(4.0, 8.0)]).unwrap().0.len(), 2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn hearing_regions_are_cut_at_the_pins() {
    let r = |start, end| Region { start, end };
    let pin = |start, end| Pin {
        start,
        end,
        name: "Ana".into(),
    };
    let regions = [r(0.0, 10.0), r(20.0, 22.0)];
    let got = seeded(&regions, &[pin(2.0, 3.0), pin(9.0, 12.0), pin(15.0, 16.0)]);
    assert_eq!(
        got,
        vec![
            r(0.0, 2.0),
            r(2.0, 3.0),
            r(3.0, 9.0),
            r(9.0, 10.0),
            r(10.0, 12.0),
            r(15.0, 16.0),
            r(20.0, 22.0)
        ]
    );
    assert_eq!(seeded(&regions, &[]), regions.to_vec());
}
