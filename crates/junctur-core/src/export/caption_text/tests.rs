use super::*;
use crate::edit::Replace;

fn shown(words: &[&str], c: &Captions) -> Vec<String> {
    let w: Vec<(usize, String)> = words.iter().map(|w| (0, w.to_string())).collect();
    units(&w, c).into_iter().map(|u| u.text).collect()
}

fn pair(from: &str, to: &str) -> Replace {
    Replace {
        from: from.into(),
        to: to.into(),
    }
}

#[test]
fn the_defaults_show_the_words_as_spoken() {
    let w = ["¿Qué", "tal?", "...", "NASA"];
    assert_eq!(shown(&w, &Captions::default()), w);
    let u = units(&[(0, "a".into()), (1, "b".into())], &Captions::default());
    assert_eq!((u[1].first, u[1].last), (1, 1));
}

#[test]
fn case_changes_letters_only() {
    let w = ["¿qué", "tal?", "NASA", "iPhone", "e-mail"];
    let with = |case| {
        shown(
            &w,
            &Captions {
                case,
                ..Default::default()
            },
        )
    };
    assert_eq!(
        with(Case::Upper),
        ["¿QUÉ", "TAL?", "NASA", "IPHONE", "E-MAIL"]
    );
    assert_eq!(
        with(Case::Lower),
        ["¿qué", "tal?", "nasa", "iphone", "e-mail"]
    );
    assert_eq!(
        with(Case::Title),
        ["¿Qué", "Tal?", "NASA", "IPhone", "E-Mail"]
    );
}

#[test]
fn punctuation_goes_but_words_and_symbols_stay() {
    let c = Captions {
        punctuation: false,
        ..Default::default()
    };
    assert_eq!(
        shown(
            &["¿Qué", "tal?", "don't", "e-mail,", "50%", "$5.", "—", "\"hi\"", "(sí)"],
            &c
        ),
        ["Qué", "tal", "don't", "e-mail", "50%", "$5", "hi", "sí"]
    );
    // A point, comma or colon inside a number is part of it; one after it is not.
    assert_eq!(
        shown(&["$5.50.", "3.14,", "1,000", "10:30:", "a.b"], &c),
        ["$5.50", "3.14", "1,000", "10:30", "ab"]
    );
    // Only between two digits: a comma after a word goes even with a digit after it.
    assert_eq!(shown(&["apples,2", "1995.2"], &c), ["apples2", "1995.2"]);
}

#[test]
fn censored_words_are_masked_whole_words_case_aside() {
    let c = Captions {
        censor: vec!["damn".into(), "what the heck".into(), "ok".into()],
        ..Default::default()
    };
    assert_eq!(
        shown(
            &[
                "Damn!",
                "damnation",
                "what",
                "the",
                "HECK?",
                "the",
                "heck",
                "OK",
                "a"
            ],
            &c
        ),
        [
            "D**n!",
            "damnation",
            "w**t",
            "t*e",
            "H**K?",
            "the",
            "heck",
            "O*",
            "a"
        ]
    );
    let whole = Captions {
        censor_style: CensorStyle::Whole,
        ..c
    };
    assert_eq!(shown(&["Damn!", "x"], &whole), ["****!", "x"]);
    // A one-letter word.
    let one = Captions {
        censor: vec!["a".into()],
        ..Default::default()
    };
    assert_eq!(shown(&["a", "b"], &one), ["*", "b"]);
    // Case aside covers `İ`, which lower-cases with a combining dot.
    let city = Captions {
        censor: vec!["istanbul".into()],
        ..Default::default()
    };
    assert_eq!(shown(&["İstanbul"], &city), ["İ******l"]);
    // Only that dot: a letter written with a combining dot of its own (`ż` as `z` and
    // U+0307) is not its bare letter.
    let zed = Captions {
        censor: vec!["zero".into()],
        ..Default::default()
    };
    assert_eq!(
        shown(&["z\u{307}ero", "zero"], &zed),
        ["z\u{307}ero", "z**o"]
    );
    let dotted = Captions {
        censor: vec!["z\u{307}ero".into()],
        ..zed
    };
    assert_eq!(shown(&["z\u{307}ero"], &dotted), ["z***o"]);
}

#[test]
fn the_censor_list_sees_each_word_of_a_replacement() {
    let c = Captions {
        replace: vec![pair("darn", "damn it"), pair("gosh", "oh my god")],
        censor: vec!["damn".into(), "my god".into()],
        case: Case::Title,
        ..Default::default()
    };
    let w: Vec<(usize, String)> = ["Darn!", "gosh,", "so"]
        .iter()
        .map(|w| (0, w.to_string()))
        .collect();
    // Still one unit each, masked word by word, the punctuation round the whole.
    let text: Vec<(usize, usize, String)> = units(&w, &c)
        .into_iter()
        .map(|u| (u.first, u.last, u.text))
        .collect();
    assert_eq!(
        text,
        [
            (0, 0, "D**n It!".to_string()),
            (1, 1, "Oh M* G*d,".to_string()),
            (2, 2, "So".to_string())
        ]
    );
    // A phrase replaced across words keeps their span.
    let p = Captions {
        replace: vec![pair("new york", "New York City")],
        ..Default::default()
    };
    let w: Vec<(usize, String)> = ["new", "york."]
        .iter()
        .map(|w| (0, w.to_string()))
        .collect();
    assert_eq!(
        units(&w, &p),
        [Unit {
            first: 0,
            last: 1,
            text: "New York City.".into()
        }]
    );
}

#[test]
fn replacements_match_whole_words_and_phrases_before_case_and_censoring() {
    let c = Captions {
        replace: vec![
            pair("gonna", "going to"),
            pair("you know", ""),
            pair("you", "ya"),
            pair("frick", "damn"),
        ],
        censor: vec!["damn".into()],
        case: Case::Upper,
        ..Default::default()
    };
    let w: Vec<(usize, String)> = [
        "¿Gonna",
        "go?",
        "You",
        "know,",
        "you",
        "gonnadance",
        "Frick!",
    ]
    .iter()
    .map(|w| (0, w.to_string()))
    .collect();
    let u = units(&w, &c);
    let text: Vec<&str> = u.iter().map(|u| u.text.as_str()).collect();
    // The longer phrase wins over `you`; its empty `to` drops it, comma and all; the censor
    // list sees the replacement.
    assert_eq!(text, ["¿GOING TO", "GO?", "YA", "GONNADANCE", "D**N!"]);
    assert_eq!((u[0].first, u[0].last), (0, 0));
    assert_eq!((u[2].first, u[2].last), (4, 4));
    // A phrase becomes one unit spanning its words.
    let p = Captions {
        replace: vec![pair("new york", "NYC")],
        ..Default::default()
    };
    let w: Vec<(usize, String)> = ["in", "New", "York."]
        .iter()
        .map(|w| (0, w.to_string()))
        .collect();
    let u = units(&w, &p);
    assert_eq!(
        u[1],
        Unit {
            first: 1,
            last: 2,
            text: "NYC.".into()
        }
    );
}

#[test]
fn a_phrase_does_not_match_across_two_tracks() {
    let c = Captions {
        replace: vec![pair("new york", "NYC")],
        ..Default::default()
    };
    // Track 0 says "new", track 1 says "york" in between; track 0 then says "york".
    let w = vec![
        (0, "new".to_string()),
        (1, "york".to_string()),
        (0, "york".to_string()),
    ];
    let u = units(&w, &c);
    assert_eq!(
        u,
        vec![
            Unit {
                first: 0,
                last: 2,
                text: "NYC".into()
            },
            Unit {
                first: 1,
                last: 1,
                text: "york".into()
            }
        ]
    );
}
