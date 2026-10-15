use super::*;

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn prefs_values_are_read_by_whole_key_last_one_wins() {
    let text = format!(
        "{{ Global = {{ AutoSubsBridge = {{ Ack = \"9\", }}, JuncturBridge = {{ \
             JuncturAck = \"1\", NotJuncturAck = \"x\", JuncturAck = \"2\", \
             JuncturReply = \"2:{}\", }}, }} }}",
        hex("placed\t3\nfirstError\t\nremoved\t1\nunstyled\tUnderline, Thickness2")
    );
    assert_eq!(pref(&text, "JuncturAck"), Some("2"));
    assert!(acked(&text, "2") && !acked(&text, "1"));
    let a = answer(&text, "2").unwrap();
    assert_eq!(a["placed"], "3");
    let s = sent(&a, "Main");
    assert_eq!((s.placed, s.removed, s.failed), (3, 1, 0));
    assert_eq!(s.first_error, None);
    assert_eq!(s.unstyled, ["Underline", "Thickness2"]);
    // An answer without the in-the-way fields, or one that found nothing in the way: all zero.
    assert_eq!(
        (
            s.in_the_way,
            s.blocked,
            s.replaced_clips,
            s.skipped,
            s.new_tracks
        ),
        (0, 0, 0, 0, 0)
    );
    // Another request's answer, or one cut off mid-write, is not this one's.
    assert!(answer(&text, "1").is_none());
    assert!(answer("JuncturReply = \"2:7061f\"", "2").is_none());
    assert!(answer("JuncturReply = \"2:zz\"", "2").is_none());
}

#[test]
fn timeline_info_and_frame_rates() {
    let text = format!(
        "JuncturReply = \"5:{}\"",
        hex("name\tMain édit\nid\tT-1\nfps\t29.97\nstart\t108000\nend\t110000\nwidth\t1920\nheight\t1080\ntemplate\tJunctur Caption\ntemplateFps\t")
    );
    let t = timeline(&answer(&text, "5").unwrap()).unwrap();
    assert_eq!((t.name.as_str(), t.id.as_str()), ("Main édit", "T-1"));
    assert!((t.fps - 30000.0 / 1001.0).abs() < 1e-9);
    assert_eq!(
        (t.start, t.end, t.width, t.height),
        (108000, 110000, 1920, 1080)
    );
    assert_eq!(t.template_fps, t.fps);
    assert_eq!(rate("25"), Some(25.0));
    assert_eq!(rate("24"), Some(24.0));
    assert!((rate("23.976").unwrap() - 24000.0 / 1001.0).abs() < 1e-9);
    assert!((rate("59.94").unwrap() - 60000.0 / 1001.0).abs() < 1e-9);
    assert_eq!(rate("0"), None);
    assert_eq!(rate("fast"), None);
    let mut bad = answer(&text, "5").unwrap();
    bad.insert("width".into(), "wide".into());
    assert!(timeline(&bad).is_err());
    let e: HashMap<String, String> = [("error".to_string(), "No timeline".to_string())].into();
    assert_eq!(error(&e).as_deref(), Some("No timeline"));
}

#[test]
fn clips_in_the_way_and_what_was_done_with_them_are_read() {
    let text = format!(
        "JuncturReply = \"3:{}\"",
        hex("inTheWay\t2\nblocked\t3\nplaced\t0")
    );
    let s = sent(&answer(&text, "3").unwrap(), "Main");
    assert_eq!((s.in_the_way, s.blocked, s.placed), (2, 3, 0));
    let text = format!(
        "JuncturReply = \"4:{}\"",
        hex("placed\t5\nreplaced\t1\nskipped\t2\nnewTracks\t1")
    );
    let s = sent(&answer(&text, "4").unwrap(), "Main");
    assert_eq!((s.replaced_clips, s.skipped, s.new_tracks), (1, 2, 1));
}

#[test]
fn only_a_script_of_this_round_or_newer_is_current() {
    let info = |v: Option<&str>| -> HashMap<String, String> {
        v.map(|v| [("script".to_string(), v.to_string())].into())
            .unwrap_or_default()
    };
    assert!(!script_is_current(&info(None)));
    assert!(!script_is_current(&info(Some("5m"))));
    assert!(!script_is_current(&info(Some("5n"))));
    assert!(!script_is_current(&info(Some("5o"))));
    assert!(!script_is_current(&info(Some("5z"))));
    assert!(!script_is_current(&info(Some("5aa"))));
    assert!(script_is_current(&info(Some("5ab"))));
    assert!(script_is_current(&info(Some("5ac"))));
    assert!(!script_is_current(&info(Some(""))));
}

#[test]
fn the_script_says_the_version_junctur_needs() {
    // The version the script answers in `check` and the one Rust asks for are kept
    // in two languages; bumping one without the other fails here, not in Resolve.
    let script = include_str!("../junctur.lua");
    assert!(
        script.contains(&format!("\nlocal SCRIPT = \"{SCRIPT}\"")),
        "junctur.lua's SCRIPT is not {SCRIPT:?}"
    );
}

#[test]
fn the_templates_found_are_read_in_order_and_unusable_names_dropped() {
    let text = format!(
        "JuncturReply = \"7:{}\"",
        hex("count\t5\nt1\tLower third \nt2\tSay \"hi\"\nt3\tTítulo grande\nt4\t \nt5\ta|b")
    );
    let a = answer(&text, "7").unwrap();
    assert_eq!(templates(&a), ["Lower third", "Título grande"]);
    // ASCII spaces only, as the script trims a clip's name: the listed one is the one it finds.
    assert_eq!(
        crate::edit::title_name("\u{a0}Lower third \t"),
        "\u{a0}Lower third"
    );
    let none: HashMap<String, String> = [("count".to_string(), "0".to_string())].into();
    assert!(templates(&none).is_empty());
    assert!(templates(&HashMap::new()).is_empty());
}

#[test]
fn the_projects_timelines_as_check_lists_them() {
    let text = format!(
        "JuncturReply = \"8:{}\"",
        hex("timelines\t2\ntimeline1\tMain\ntimeline2\trec (Junctur)")
    );
    let a = answer(&text, "8").unwrap();
    assert_eq!(timeline_names(&a), ["Main", "rec (Junctur)"]);
    assert!(timeline_names(&HashMap::new()).is_empty());
    // What a send made, beside the captions' counts: none unless the build says.
    let s = sent(&a, "rec (Junctur)");
    assert_eq!((s.replaced, s.note), (false, None));
}
