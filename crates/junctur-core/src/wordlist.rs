//! The user's word list: names and terms Whisper is told before every chunk, so they come out
//! spelled the user's way. The rules (what an entry is, what a fix adds, how a long list is cut to
//! the prompt) live here once for the config file, the app and the transcriber.

/// Longest entry kept, in characters: a name or a short phrase, not a paragraph.
pub const MAX_ENTRY: usize = 100;

/// `entry` as the list keeps it: trimmed, control characters (a NUL would make whisper-rs panic, a
/// line break would split it in Settings) as spaces, runs of space as one, at most [`MAX_ENTRY`]
/// characters; `None` when nothing is left.
pub fn entry(raw: &str) -> Option<String> {
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let joined = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = joined.chars().take(MAX_ENTRY).collect();
    let cut = cut.trim_end().to_string();
    (!cut.is_empty()).then_some(cut)
}

/// The list as it is saved: every entry through [`entry`], blanks and repeats (exact) dropped, the
/// first of a repeat kept where it was.
pub fn normalize(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in list.iter().filter_map(|e| entry(e)) {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// Adds `raw` at the top of `list` unless it is there already; whether it was added. At the top, not
/// the end: [`prompt`] keeps the first entries, so a word added at the end of a full list would never
/// reach Whisper. Entries the user types in Settings keep the order they are typed in.
pub fn add_first(list: &mut Vec<String>, raw: &str) -> bool {
    match entry(raw) {
        Some(e) if !list.contains(&e) => {
            list.insert(0, e);
            true
        }
        _ => false,
    }
}

/// `s` without the punctuation around it (`"Ana,"` is the name `Ana`).
fn bare(s: &str) -> &str {
    s.trim_matches(|c: char| !c.is_alphanumeric())
}

/// What a word fixed from `heard` to `fixed` adds to the list: the fixed spelling without the
/// punctuation around it; `None` when that is empty or the fix only changes the punctuation around
/// what was heard. A change of case is learned: a name heard in lower case (`tsunheto`) is the
/// commonest fix there is.
pub fn learned(heard: &str, fixed: &str) -> Option<String> {
    let new = bare(fixed);
    if new.is_empty() || new == bare(heard) {
        return None;
    }
    entry(new)
}

/// What a run of words heard as `heard` and retyped as `fixed` adds to the list: each new word not
/// among the heard ones (compared as [`learned`] does), in the order typed, each once. A sentence's
/// capital (a heard word recased at a sentence start) is not a name and adds nothing.
pub fn learned_all(heard: &str, fixed: &str) -> Vec<String> {
    let was: Vec<&str> = heard.split_whitespace().map(bare).collect();
    let mut out: Vec<String> = Vec::new();
    let mut starts = true;
    for w in fixed.split_whitespace() {
        let new = bare(w);
        let capital = starts && was.iter().any(|h| h.to_lowercase() == new.to_lowercase());
        starts = w
            .trim_end_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '.' | '?' | '!'))
            .ends_with(['.', '?', '!']);
        if new.is_empty() || capital || was.contains(&new) {
            continue;
        }
        if let Some(e) = entry(new) {
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

/// The words a recording's transcript heard, as a fix's new words are compared with them: each
/// without the punctuation around it, lower-cased ([`crate::edit::likeness`]). A fix teaches the word
/// list only words the recording never heard: a retyped sentence's common words ("went", "home") would
/// otherwise push the names out of Whisper's ~223-token prompt ([`prompt`] keeps the first entries).
#[derive(Debug, Clone, Default)]
pub struct Heard(std::collections::HashSet<String>);

impl Heard {
    pub fn new<'a>(words: impl IntoIterator<Item = &'a str>) -> Self {
        Self(
            words
                .into_iter()
                .map(crate::edit::likeness)
                .filter(|w| !w.is_empty())
                .collect(),
        )
    }

    /// Every word of `entry` was heard somewhere in the recording.
    fn has(&self, entry: &str) -> bool {
        entry
            .split_whitespace()
            .map(crate::edit::likeness)
            .filter(|w| !w.is_empty())
            .all(|w| self.0.contains(&w))
    }
}

/// [`learned`], for a word of a recording that heard `recording`: the spelling only when it has a word
/// the recording never heard, or when it changes nothing but the heard word's case (a name heard in
/// lower case, `tsunheto` to `Tsunheto`: heard, but not as the user spells it).
pub fn learned_in(heard: &str, fixed: &str, recording: &Heard) -> Option<String> {
    let e = learned(heard, fixed)?;
    let case_only = bare(fixed).to_lowercase() == bare(heard).to_lowercase();
    (case_only || !recording.has(&e)).then_some(e)
}

/// [`learned_all`], for a run of words of a recording that heard `recording`: its new words the
/// recording never heard, and those that only change the case of one of the run's own heard words
/// (a name, not a sentence's capital, which [`learned_all`] already leaves out).
pub fn learned_all_in(heard: &str, fixed: &str, recording: &Heard) -> Vec<String> {
    let own: Vec<String> = heard
        .split_whitespace()
        .map(|w| bare(w).to_lowercase())
        .collect();
    learned_all(heard, fixed)
        .into_iter()
        .filter(|e| own.contains(&e.to_lowercase()) || !recording.has(e))
        .collect()
}

/// What Whisper is told: the list's leading entries that fit, and how many were left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    /// The initial prompt, entries joined with ", "; empty when nothing fits or the list is empty.
    pub text: String,
    /// Entries in it.
    pub kept: usize,
    /// The entries left out (the end of the list), for the log.
    pub left_out: Vec<String>,
}

/// The longest leading run of `list` whose prompt `tokens` counts within `budget`. A fixed order (the
/// list's own, so an entry higher in Settings wins) rather than whisper.cpp's own cut, which keeps
/// the last tokens of a long prompt and so drops the start mid-word.
pub fn prompt(list: &[String], budget: usize, tokens: impl Fn(&str) -> usize) -> Prompt {
    let list = normalize(list);
    let mut text = String::new();
    let mut kept = 0;
    for e in &list {
        let next = if text.is_empty() {
            e.clone()
        } else {
            format!("{text}, {e}")
        };
        if tokens(&next) > budget {
            break;
        }
        text = next;
        kept += 1;
    }
    Prompt {
        text,
        kept,
        left_out: list[kept..].to_vec(),
    }
}

#[cfg(test)]
mod tests {
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
}
