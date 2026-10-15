use super::*;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("junctur-voices-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn enroll_again_keeps_the_colour_and_new_people_get_free_colours() {
    let mut v = Voices::default();
    let e = [1.0f32, 0.0];
    let ee: [&[f32]; 1] = [&e];
    assert_eq!(v.enroll("Uezar", &ee, 5.0).unwrap().color, "#F2B33D");
    v.recolor("uezar", "#123456").unwrap();
    assert_eq!(v.enroll("Ana", &ee, 5.0).unwrap().color, "#F2B33D");
    // Enrolled again by name (any case): one voice, the samples weighted by seconds, same
    // colour and name.
    let f = [0.0f32, 1.0];
    let ff: [&[f32]; 1] = [&f];
    assert_eq!(v.enroll("UEZAR", &ff, 5.0).unwrap().color, "#123456");
    assert_eq!(v.voices.len(), 2);
    let u = v.get("uezar").unwrap();
    assert_eq!((u.name.as_str(), u.seconds), ("Uezar", 10.0));
    assert!((u.print[0] - u.print[1]).abs() < 1e-6 && (u.print[0] - 0.70710677).abs() < 1e-5);
    // Any length of speech is remembered, however short; no sample, or a name the edit
    // file cannot hold, is not.
    assert_eq!(v.enroll("Bea", &ee, 0.3).unwrap().seconds, 0.3);
    assert!(v.forget("bea"));
    assert!(v.enroll("color=x", &ee, 5.0).is_err());
    assert!(v.enroll("Bea", &[], 5.0).is_err());
    assert!(v.forget("ana") && !v.forget("ana"));
    assert!(v.rename("UEZAR", "Uez").unwrap());
    assert!(!v.rename("nobody", "x").unwrap());
    assert!(v.recolor("Uez", "red").is_err());
}

#[test]
fn a_ninth_person_gets_a_colour_of_their_own() {
    let mut v = Voices::default();
    let e = [1.0f32, 0.0];
    for i in 0..10 {
        v.enroll(&format!("P{i}"), &[&e], 5.0).unwrap();
    }
    let mut colours: Vec<&str> = v.voices.iter().map(|x| x.color.as_str()).collect();
    assert!(
        colours
            .iter()
            .all(|c| parse_color(c).as_deref() == Some(*c)),
        "{colours:?}"
    );
    colours.sort();
    colours.dedup();
    assert_eq!(colours.len(), 10);
    // Names match whatever their case, accents included.
    v.enroll("Lucía", &[&e], 5.0).unwrap();
    v.enroll("LUCÍA", &[&e], 5.0).unwrap();
    assert_eq!(v.get("lucía").unwrap().seconds, 10.0);
}

#[test]
fn the_file_round_trips_and_a_broken_one_is_an_error() {
    let d = scratch("file");
    let path = d.join("junctur").join("voices.json");
    assert_eq!(Voices::load(&path).unwrap(), Voices::default());
    let mut v = Voices::default();
    let e = [0.6f32, 0.8];
    v.enroll("Ana", &[&e], 3.0).unwrap();
    v.save(&path).unwrap();
    assert_eq!(Voices::load(&path).unwrap(), v);
    assert_eq!(v.usable().len(), 1);
    // A print from another model is not used, and not even read back.
    v.voices[0].model = "other.onnx".into();
    assert!(v.usable().is_empty());
    v.save(&path).unwrap();
    assert!(Voices::load(&path).unwrap().voices.is_empty());
    std::fs::write(&path, "{ nope").unwrap();
    assert!(Voices::load(&path).is_err());
    // A hand-edited colour or name the edit file cannot hold.
    let mut odd = v.clone();
    odd.voices[0].model = print_model();
    odd.voices[0].color = "red".into();
    std::fs::write(&path, serde_json::to_vec(&odd).unwrap()).unwrap();
    assert!(Voices::load(&path).unwrap_err().to_string().contains("red"));
    odd.voices[0].color = "#123456".into();
    odd.voices[0].name = "track=2".into();
    std::fs::write(&path, serde_json::to_vec(&odd).unwrap()).unwrap();
    assert!(Voices::load(&path).is_err());
    // Saves leave no temporary file behind.
    v.save(&path).unwrap();
    let left: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(left.len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn voices_from_before_5r_are_dropped_when_read_and_leave_the_file_at_its_next_save() {
    let d = scratch("before-5r");
    let path = d.join("voices.json");
    let mut v = Voices::default();
    let e = [1.0f32, 0.0];
    v.enroll("Uezar", &[&e], 30.0).unwrap();
    v.enroll("Ana", &[&e], 30.0).unwrap();
    assert!(v.voices.iter().all(|x| x.model.ends_with("+cpu")));
    // An old print under the bare model digest, with a colour this build would refuse: an old
    // voice never makes the file unreadable.
    v.voices[0].model = models::SPEAKER_EMBEDDING.sha256.into();
    v.voices[0].color = "red".into();
    v.save(&path).unwrap();
    let read = Voices::load(&path).unwrap();
    let names: Vec<&str> = read.voices.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Ana"]);
    // Saved again, it is gone from the file too.
    read.save(&path).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("Uezar"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_voiceprint_is_the_unit_mean() {
    let (a, b) = ([1.0f32, 0.0], [0.0f32, 1.0]);
    let p = voiceprint(&[&a, &b]);
    assert!((p[0] - p[1]).abs() < 1e-6 && (p[0] * p[0] + p[1] * p[1] - 1.0).abs() < 1e-5);
}
