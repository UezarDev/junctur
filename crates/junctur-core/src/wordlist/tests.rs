use super::*;

fn list(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn entries_are_trimmed_cleaned_and_never_repeated() {
    assert_eq!(entry("  Tsun\u{0}heto \n"), Some("Tsun heto".into()));
    assert_eq!(entry(" \t"), None);
    assert_eq!(entry(&"a".repeat(150)).unwrap().chars().count(), MAX_ENTRY);
    assert_eq!(
        normalize(&list(&["Uezar", " ", "Delay", "Uezar ", "uezar"])),
        list(&["Uezar", "Delay", "uezar"])
    );
    let mut l = list(&["Uezar"]);
    assert!(!add_first(&mut l, " Uezar"));
    assert!(add_first(&mut l, "Tsunheto"));
    assert!(!add_first(&mut l, "   "));
    assert_eq!(l, list(&["Tsunheto", "Uezar"]), "a new word goes first");
    // So it reaches Whisper even when the list is full: a later fix is told, the oldest is cut.
    let chars = |s: &str| s.chars().count();
    assert!(add_first(&mut l, "Delay"));
    let p = prompt(&l, 15, chars);
    assert_eq!(p.text, "Delay, Tsunheto");
    assert_eq!(p.left_out, list(&["Uezar"]));
}

#[test]
fn a_retyped_sentence_adds_its_new_words_in_order() {
    // Every word not heard, once, in the order typed.
    assert_eq!(
        learned_all("the wezar went hom", "Uezar went home, Uezar!"),
        ["Uezar", "home"]
    );
    // A sentence's capital is not a name; a name's capital is.
    assert_eq!(learned_all("so tsunheto", "So, Tsunheto."), ["Tsunheto"]);
    assert!(learned_all("ok. so? yes", "Ok. So? Yes").is_empty());
    assert_eq!(learned_all("ok. so", "ok so Ok"), ["Ok"]);
    assert!(learned_all("hola amigos", "amigos, hola.").is_empty());
    assert!(learned_all("eh eh", "... !").is_empty());
}

#[test]
fn a_fix_teaches_only_words_the_recording_never_heard() {
    // A retyped sentence's common words stay out of the list, names go in.
    let rec = Heard::new([
        " the",
        " Wezar,",
        " went",
        " home.",
        " tsunheto",
        " del",
        " ay",
    ]);
    assert_eq!(
        learned_all_in("the wezar went hom", "Uezar went home, Uezar!", &rec),
        ["Uezar"]
    );
    // A name heard in lower case is still learned with its capital, in a sentence or alone.
    assert_eq!(
        learned_all_in("so tsunheto", "So, Tsunheto.", &rec),
        ["Tsunheto"]
    );
    assert_eq!(
        learned_in("tsunheto", "Tsunheto", &rec),
        Some("Tsunheto".into())
    );
    // One word fixed to a word heard elsewhere teaches nothing; to a new one, it does.
    assert_eq!(learned_in("hom", "home", &rec), None);
    assert_eq!(learned_in("Bane,", "Vayne,", &rec), Some("Vayne".into()));
    // A phrase is new when any of its words is.
    assert_eq!(learned_in("delay", "Del Ay", &rec), None);
    assert_eq!(learned_in("delay", "Del Rey", &rec), Some("Del Rey".into()));
}

#[test]
fn a_fix_adds_its_spelling_not_its_punctuation_or_case() {
    assert_eq!(learned("Wezar,", "Uezar,"), Some("Uezar".into()));
    assert_eq!(learned("tsun eto", "Tsunheto."), Some("Tsunheto".into()));
    assert_eq!(
        learned("tsunheto", "Tsunheto"),
        Some("Tsunheto".into()),
        "case"
    );
    assert_eq!(learned("Uezar", "Uezar."), None, "punctuation only");
    assert_eq!(learned(" Uezar,", "Uezar"), None, "punctuation only");
    assert_eq!(learned("eh", "..."), None, "nothing left");
    assert_eq!(learned("del ay", "Delay"), Some("Delay".into()));
}

#[test]
fn a_long_list_keeps_its_first_entries_that_fit() {
    // One token per character, for the test.
    let chars = |s: &str| s.chars().count();
    let l = list(&["Uezar", "Delay", "Tsunheto", "Junctur"]);
    let p = prompt(&l, 100, chars);
    assert_eq!(p.text, "Uezar, Delay, Tsunheto, Junctur");
    assert_eq!((p.kept, p.left_out.len()), (4, 0));
    // "Uezar, Delay" is 12; with ", Tsunheto" 22.
    let p = prompt(&l, 21, chars);
    assert_eq!(p.text, "Uezar, Delay");
    assert_eq!(p.left_out, list(&["Tsunheto", "Junctur"]));
    // A later short entry does not jump the queue: the order is fixed.
    let p = prompt(&list(&["Uezar", "Tsunheto", "Ok"]), 13, chars);
    assert_eq!((p.text.as_str(), p.kept), ("Uezar", 1));
    // Nothing fits, or nothing to say.
    assert_eq!(prompt(&l, 3, chars).text, "");
    assert_eq!(prompt(&[], 100, chars).kept, 0);
    assert_eq!(prompt(&list(&[" ", "\n"]), 100, chars).text, "");
}
