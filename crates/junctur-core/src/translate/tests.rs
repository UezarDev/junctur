use super::*;

fn seg(track: usize, start: f64, end: f64, lang: &str) -> Segment {
    Segment {
        track,
        start,
        end,
        lang: lang.into(),
        model: String::new(),
        words: Vec::new(),
    }
}

#[test]
fn a_sentence_takes_its_segments_language() {
    let s = [
        seg(0, 0.0, 10.0, "es"),
        seg(0, 10.0, 20.0, "en"),
        seg(1, 0.0, 30.0, "fr"),
    ];
    assert_eq!(spoken(&s, 0, 5.0), Some("es"));
    assert_eq!(spoken(&s, 0, 15.0), Some("en"));
    assert_eq!(spoken(&s, 0, 25.0), Some("en"), "nearest");
    assert_eq!(spoken(&s, 1, 5.0), Some("fr"));
    assert_eq!(spoken(&s, 2, 5.0), None);
}

/// The index answers as the scan does: transcript-shaped tracks (touching, overlapping, gaps, zero
/// length, equal starts), a track out of order, and times before, inside, between and after.
#[test]
fn the_language_index_answers_as_the_scan_does() {
    let langs = ["en", "es", "fr", "de", "it"];
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let mut segments = Vec::new();
    for track in 0..3 {
        let (mut start, mut end) = (0.0f64, 0.0f64);
        for k in 0..300 {
            start += [0.0, 0.0, 0.25, 1.5, 9.0][next(5) as usize];
            end = end.max(start + [0.0, 0.1, 2.0, 7.3][next(4) as usize]);
            let lang = langs[(next(5) + k) as usize % 5];
            segments.push(seg(track, start, end, lang));
        }
    }
    // A track whose segments are out of order takes the scan.
    for k in 0..50 {
        let s = (next(1000) as f64) / 3.0;
        segments.push(seg(3, s, s + (k % 4) as f64, langs[k % 5]));
    }
    let index = Spoken::new(&segments);
    assert!((0..3).all(|t| index.tracks[&t].sorted) && !index.tracks[&3].sorted);
    for track in 0..5 {
        for i in 0..4000 {
            let t = i as f64 * 0.37 - 20.0;
            assert_eq!(
                index.at(track, t),
                spoken(&segments, track, t),
                "track {track} at {t}"
            );
        }
        for s in segments.iter().filter(|s| s.track == track) {
            for t in [s.start, s.end, (s.start + s.end) / 2.0] {
                assert_eq!(index.at(track, t), spoken(&segments, track, t));
            }
        }
    }
}

#[test]
fn sentences_already_in_the_target_language_are_kept_without_the_model() {
    let d = std::env::temp_dir().join(format!("junctur-tr-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let segments = [seg(0, 0.0, 10.0, "en"), seg(0, 10.0, 20.0, "haw")];
    let sentences = [
        Sentence {
            track: 0,
            start: 1.0,
            text: "Hello there.".into(),
        },
        Sentence {
            track: 0,
            start: 12.0,
            text: "Aloha.".into(),
        },
        Sentence {
            track: 0,
            start: 2.0,
            text: "Hello there.".into(),
        },
        // No segment on its track: its language is not known, so it is not kept.
        Sentence {
            track: 1,
            start: 2.0,
            text: "Who?".into(),
        },
    ];
    let mut seen = Vec::new();
    // A cache dir with no model in it: opening the model would fail the run.
    let none = d.join("no-models");
    let done = run(
        &d,
        &none,
        "en",
        &sentences,
        &segments,
        &mut |a, b| seen.push((a, b)),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        done,
        Done {
            kept: 2,
            unknown: 1,
            ..Done::default()
        }
    );
    assert_eq!(seen, [(0, 2), (2, 2)]);
    let t = Translations::load(&d);
    assert_eq!(t.get("en", "Aloha."), Some("Aloha."));
    assert_eq!(t.get("en", "Who?"), None);
    // Nothing left: a second run does nothing.
    let again = run(
        &d,
        &none,
        "en",
        &sentences,
        &segments,
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        again,
        Done {
            unknown: 1,
            ..Done::default()
        }
    );
    // A transcript that read as nothing is an error, not every sentence kept as said.
    assert!(run(
        &d,
        &none,
        "fr",
        &sentences,
        &[],
        &mut |_, _| {},
        &AtomicBool::new(false)
    )
    .is_err());
    // Stopped before the first: nothing done, nothing opened.
    let stopped = run(
        &d,
        &none,
        "fr",
        &sentences,
        &segments,
        &mut |_, _| {},
        &AtomicBool::new(true),
    )
    .unwrap();
    assert!(stopped.cancelled && stopped.translated == 0);
    // A translations file that does not read is an error, and left as it is.
    std::fs::write(d.join(cache::FILE), "{").unwrap();
    assert!(run(
        &d,
        &none,
        "en",
        &sentences,
        &segments,
        &mut |_, _| {},
        &AtomicBool::new(false)
    )
    .is_err());
    assert_eq!(std::fs::read_to_string(d.join(cache::FILE)).unwrap(), "{");
    assert!(run(
        &d,
        &none,
        "xx",
        &[],
        &[],
        &mut |_, _| {},
        &AtomicBool::new(false)
    )
    .is_err());
    let _ = std::fs::remove_dir_all(&d);
}
