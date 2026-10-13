//! The languages captions can be translated into and from: the translation model's own hundred,
//! by the codes it names them with (ISO 639-1 where there is one), which are Whisper's too but for
//! Javanese. Data, so the settings, the line's check and the app's list all read one table.

/// `(code, English name)`, in the model's order.
pub const LANGUAGES: [(&str, &str); 100] = [
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("ast", "Asturian"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("be", "Belarusian"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("br", "Breton"),
    ("bs", "Bosnian"),
    ("ca", "Catalan"),
    ("ceb", "Cebuano"),
    ("cs", "Czech"),
    ("cy", "Welsh"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fa", "Persian"),
    ("ff", "Fulah"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("fy", "Western Frisian"),
    ("ga", "Irish"),
    ("gd", "Scottish Gaelic"),
    ("gl", "Galician"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("ht", "Haitian Creole"),
    ("hu", "Hungarian"),
    ("hy", "Armenian"),
    ("id", "Indonesian"),
    ("ig", "Igbo"),
    ("ilo", "Iloko"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jv", "Javanese"),
    ("ka", "Georgian"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("lb", "Luxembourgish"),
    ("lg", "Ganda"),
    ("ln", "Lingala"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mg", "Malagasy"),
    ("mk", "Macedonian"),
    ("ml", "Malayalam"),
    ("mn", "Mongolian"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("my", "Burmese"),
    ("ne", "Nepali"),
    ("nl", "Dutch"),
    ("no", "Norwegian"),
    ("ns", "Northern Sotho"),
    ("oc", "Occitan"),
    ("or", "Oriya"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("ps", "Pashto"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("so", "Somali"),
    ("sq", "Albanian"),
    ("sr", "Serbian"),
    ("ss", "Swati"),
    ("su", "Sundanese"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("th", "Thai"),
    ("tl", "Tagalog"),
    ("tn", "Tswana"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("wo", "Wolof"),
    ("xh", "Xhosa"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("zh", "Chinese"),
    ("zu", "Zulu"),
];

/// Whether captions can be translated into `code`.
pub fn known(code: &str) -> bool {
    LANGUAGES.iter().any(|(c, _)| *c == code)
}

/// The model's code for a language Whisper heard, if the model has it.
pub fn from_whisper(code: &str) -> Option<&'static str> {
    // Whisper writes Javanese the old way.
    let code = if code == "jw" { "jv" } else { code };
    LANGUAGES.iter().map(|(c, _)| *c).find(|c| *c == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds_each_code_once_and_maps_whisper_codes() {
        let mut codes: Vec<&str> = LANGUAGES.iter().map(|(c, _)| *c).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), LANGUAGES.len());
        assert!(known("fr") && known("es") && !known("") && !known("xx"));
        assert_eq!(from_whisper("es"), Some("es"));
        assert_eq!(from_whisper("jw"), Some("jv"));
        assert_eq!(from_whisper("haw"), None);
    }
}
