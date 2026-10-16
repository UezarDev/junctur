//! Small text rules several modules apply to a word or a line, kept in one place so the word list,
//! fixes, the carry onto a new transcript and captions read a word alike.

/// Runs of whitespace as one space, trimmed.
pub fn one_spaced(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// [`one_spaced`] after control characters become spaces: a NUL would make whisper-rs panic and a
/// line break would split a line where one is expected.
pub fn squeezed(s: &str) -> String {
    let spaced: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_spaced(&spaced)
}

/// `s` without the punctuation around it (`"Ana,"` is the word `Ana`).
pub fn bare(s: &str) -> &str {
    s.trim_matches(|c: char| !c.is_alphanumeric())
}

/// `s` as what comes before its first letter or digit, from there to its last ([`bare`]), and the
/// rest; with no letter or digit, all of `s` comes first.
pub fn split_bare(s: &str) -> (&str, &str, &str) {
    let Some(a) = s.find(char::is_alphanumeric) else {
        return (s, "", "");
    };
    let b = s
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map_or(s.len(), |(k, c)| k + c.len_utf8());
    (&s[..a], &s[a..b], &s[b..])
}

/// `s` lowercase, its accents folded (ñ kept), anything not a letter or digit a space, one space
/// between words and one at either end, so a phrase is matched as whole words: how smart keep's
/// prompt and the moment markers' phrases are read.
pub fn folded(s: &str) -> String {
    let mut out = String::from(" ");
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        if c != ' ' || !out.ends_with(' ') {
            out.push(c);
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_and_the_word_inside_its_punctuation() {
        assert_eq!(one_spaced("  a \t b\n\nc "), "a b c");
        assert_eq!(one_spaced("a\0b"), "a\0b");
        assert_eq!(squeezed(" a\0b\u{7}\r\nc "), "a b c");
        for s in [
            "",
            "...",
            "Ana,",
            "«¿Qué?»",
            "it's",
            "  x  ",
            "-1.5%",
            "İstanbul!",
        ] {
            let (lead, core, trail) = split_bare(s);
            assert_eq!(format!("{lead}{core}{trail}"), s);
            assert_eq!(core, bare(s), "{s:?}");
        }
        assert_eq!(split_bare("«¿Qué?»"), ("«¿", "Qué", "?»"));
        assert_eq!(split_bare("..."), ("...", "", ""));
    }
}
