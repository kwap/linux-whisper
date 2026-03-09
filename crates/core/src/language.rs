use serde::{Deserialize, Serialize};
use std::fmt;

/// Languages supported by OpenAI Whisper for speech recognition and transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Afrikaans,
    Arabic,
    Armenian,
    Azerbaijani,
    Belarusian,
    Bosnian,
    Bulgarian,
    Catalan,
    Chinese,
    Croatian,
    Czech,
    Danish,
    Dutch,
    English,
    Estonian,
    Finnish,
    French,
    Galician,
    German,
    Greek,
    Hebrew,
    Hindi,
    Hungarian,
    Icelandic,
    Indonesian,
    Italian,
    Japanese,
    Kannada,
    Kazakh,
    Korean,
    Latvian,
    Lithuanian,
    Macedonian,
    Malay,
    Marathi,
    Maori,
    Nepali,
    Norwegian,
    Persian,
    Polish,
    Portuguese,
    Romanian,
    Russian,
    Serbian,
    Slovak,
    Slovenian,
    Spanish,
    Swahili,
    Swedish,
    Tamil,
    Thai,
    Turkish,
    Ukrainian,
    Urdu,
    Vietnamese,
    Welsh,
}

static ALL_LANGUAGES: [Language; 56] = [
    Language::Afrikaans,
    Language::Arabic,
    Language::Armenian,
    Language::Azerbaijani,
    Language::Belarusian,
    Language::Bosnian,
    Language::Bulgarian,
    Language::Catalan,
    Language::Chinese,
    Language::Croatian,
    Language::Czech,
    Language::Danish,
    Language::Dutch,
    Language::English,
    Language::Estonian,
    Language::Finnish,
    Language::French,
    Language::Galician,
    Language::German,
    Language::Greek,
    Language::Hebrew,
    Language::Hindi,
    Language::Hungarian,
    Language::Icelandic,
    Language::Indonesian,
    Language::Italian,
    Language::Japanese,
    Language::Kannada,
    Language::Kazakh,
    Language::Korean,
    Language::Latvian,
    Language::Lithuanian,
    Language::Macedonian,
    Language::Malay,
    Language::Marathi,
    Language::Maori,
    Language::Nepali,
    Language::Norwegian,
    Language::Persian,
    Language::Polish,
    Language::Portuguese,
    Language::Romanian,
    Language::Russian,
    Language::Serbian,
    Language::Slovak,
    Language::Slovenian,
    Language::Spanish,
    Language::Swahili,
    Language::Swedish,
    Language::Tamil,
    Language::Thai,
    Language::Turkish,
    Language::Ukrainian,
    Language::Urdu,
    Language::Vietnamese,
    Language::Welsh,
];

impl Language {
    /// Returns the ISO 639-1 two-letter language code for this language.
    pub fn code(&self) -> &'static str {
        match self {
            Language::Afrikaans => "af",
            Language::Arabic => "ar",
            Language::Armenian => "hy",
            Language::Azerbaijani => "az",
            Language::Belarusian => "be",
            Language::Bosnian => "bs",
            Language::Bulgarian => "bg",
            Language::Catalan => "ca",
            Language::Chinese => "zh",
            Language::Croatian => "hr",
            Language::Czech => "cs",
            Language::Danish => "da",
            Language::Dutch => "nl",
            Language::English => "en",
            Language::Estonian => "et",
            Language::Finnish => "fi",
            Language::French => "fr",
            Language::Galician => "gl",
            Language::German => "de",
            Language::Greek => "el",
            Language::Hebrew => "he",
            Language::Hindi => "hi",
            Language::Hungarian => "hu",
            Language::Icelandic => "is",
            Language::Indonesian => "id",
            Language::Italian => "it",
            Language::Japanese => "ja",
            Language::Kannada => "kn",
            Language::Kazakh => "kk",
            Language::Korean => "ko",
            Language::Latvian => "lv",
            Language::Lithuanian => "lt",
            Language::Macedonian => "mk",
            Language::Malay => "ms",
            Language::Marathi => "mr",
            Language::Maori => "mi",
            Language::Nepali => "ne",
            Language::Norwegian => "no",
            Language::Persian => "fa",
            Language::Polish => "pl",
            Language::Portuguese => "pt",
            Language::Romanian => "ro",
            Language::Russian => "ru",
            Language::Serbian => "sr",
            Language::Slovak => "sk",
            Language::Slovenian => "sl",
            Language::Spanish => "es",
            Language::Swahili => "sw",
            Language::Swedish => "sv",
            Language::Tamil => "ta",
            Language::Thai => "th",
            Language::Turkish => "tr",
            Language::Ukrainian => "uk",
            Language::Urdu => "ur",
            Language::Vietnamese => "vi",
            Language::Welsh => "cy",
        }
    }

    /// Returns the English name of this language.
    pub fn name(&self) -> &'static str {
        match self {
            Language::Afrikaans => "Afrikaans",
            Language::Arabic => "Arabic",
            Language::Armenian => "Armenian",
            Language::Azerbaijani => "Azerbaijani",
            Language::Belarusian => "Belarusian",
            Language::Bosnian => "Bosnian",
            Language::Bulgarian => "Bulgarian",
            Language::Catalan => "Catalan",
            Language::Chinese => "Chinese",
            Language::Croatian => "Croatian",
            Language::Czech => "Czech",
            Language::Danish => "Danish",
            Language::Dutch => "Dutch",
            Language::English => "English",
            Language::Estonian => "Estonian",
            Language::Finnish => "Finnish",
            Language::French => "French",
            Language::Galician => "Galician",
            Language::German => "German",
            Language::Greek => "Greek",
            Language::Hebrew => "Hebrew",
            Language::Hindi => "Hindi",
            Language::Hungarian => "Hungarian",
            Language::Icelandic => "Icelandic",
            Language::Indonesian => "Indonesian",
            Language::Italian => "Italian",
            Language::Japanese => "Japanese",
            Language::Kannada => "Kannada",
            Language::Kazakh => "Kazakh",
            Language::Korean => "Korean",
            Language::Latvian => "Latvian",
            Language::Lithuanian => "Lithuanian",
            Language::Macedonian => "Macedonian",
            Language::Malay => "Malay",
            Language::Marathi => "Marathi",
            Language::Maori => "Maori",
            Language::Nepali => "Nepali",
            Language::Norwegian => "Norwegian",
            Language::Persian => "Persian",
            Language::Polish => "Polish",
            Language::Portuguese => "Portuguese",
            Language::Romanian => "Romanian",
            Language::Russian => "Russian",
            Language::Serbian => "Serbian",
            Language::Slovak => "Slovak",
            Language::Slovenian => "Slovenian",
            Language::Spanish => "Spanish",
            Language::Swahili => "Swahili",
            Language::Swedish => "Swedish",
            Language::Tamil => "Tamil",
            Language::Thai => "Thai",
            Language::Turkish => "Turkish",
            Language::Ukrainian => "Ukrainian",
            Language::Urdu => "Urdu",
            Language::Vietnamese => "Vietnamese",
            Language::Welsh => "Welsh",
        }
    }

    /// Looks up a language by its ISO 639-1 two-letter code.
    ///
    /// Returns `None` if the code does not match any supported language.
    pub fn from_code(code: &str) -> Option<Language> {
        match code {
            "af" => Some(Language::Afrikaans),
            "ar" => Some(Language::Arabic),
            "hy" => Some(Language::Armenian),
            "az" => Some(Language::Azerbaijani),
            "be" => Some(Language::Belarusian),
            "bs" => Some(Language::Bosnian),
            "bg" => Some(Language::Bulgarian),
            "ca" => Some(Language::Catalan),
            "zh" => Some(Language::Chinese),
            "hr" => Some(Language::Croatian),
            "cs" => Some(Language::Czech),
            "da" => Some(Language::Danish),
            "nl" => Some(Language::Dutch),
            "en" => Some(Language::English),
            "et" => Some(Language::Estonian),
            "fi" => Some(Language::Finnish),
            "fr" => Some(Language::French),
            "gl" => Some(Language::Galician),
            "de" => Some(Language::German),
            "el" => Some(Language::Greek),
            "he" => Some(Language::Hebrew),
            "hi" => Some(Language::Hindi),
            "hu" => Some(Language::Hungarian),
            "is" => Some(Language::Icelandic),
            "id" => Some(Language::Indonesian),
            "it" => Some(Language::Italian),
            "ja" => Some(Language::Japanese),
            "kn" => Some(Language::Kannada),
            "kk" => Some(Language::Kazakh),
            "ko" => Some(Language::Korean),
            "lv" => Some(Language::Latvian),
            "lt" => Some(Language::Lithuanian),
            "mk" => Some(Language::Macedonian),
            "ms" => Some(Language::Malay),
            "mr" => Some(Language::Marathi),
            "mi" => Some(Language::Maori),
            "ne" => Some(Language::Nepali),
            "no" => Some(Language::Norwegian),
            "fa" => Some(Language::Persian),
            "pl" => Some(Language::Polish),
            "pt" => Some(Language::Portuguese),
            "ro" => Some(Language::Romanian),
            "ru" => Some(Language::Russian),
            "sr" => Some(Language::Serbian),
            "sk" => Some(Language::Slovak),
            "sl" => Some(Language::Slovenian),
            "es" => Some(Language::Spanish),
            "sw" => Some(Language::Swahili),
            "sv" => Some(Language::Swedish),
            "ta" => Some(Language::Tamil),
            "th" => Some(Language::Thai),
            "tr" => Some(Language::Turkish),
            "uk" => Some(Language::Ukrainian),
            "ur" => Some(Language::Urdu),
            "vi" => Some(Language::Vietnamese),
            "cy" => Some(Language::Welsh),
            _ => None,
        }
    }

    /// Returns a static slice of all supported language variants.
    pub fn all() -> &'static [Language] {
        &ALL_LANGUAGES
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_returns_correct_iso_codes() {
        assert_eq!(Language::English.code(), "en");
        assert_eq!(Language::Spanish.code(), "es");
        assert_eq!(Language::French.code(), "fr");
        assert_eq!(Language::German.code(), "de");
        assert_eq!(Language::Japanese.code(), "ja");
        assert_eq!(Language::Chinese.code(), "zh");
        assert_eq!(Language::Arabic.code(), "ar");
        assert_eq!(Language::Russian.code(), "ru");
        assert_eq!(Language::Korean.code(), "ko");
        assert_eq!(Language::Hindi.code(), "hi");
        assert_eq!(Language::Portuguese.code(), "pt");
        assert_eq!(Language::Turkish.code(), "tr");
        assert_eq!(Language::Welsh.code(), "cy");
        assert_eq!(Language::Persian.code(), "fa");
        assert_eq!(Language::Armenian.code(), "hy");
        assert_eq!(Language::Maori.code(), "mi");
    }

    #[test]
    fn name_returns_correct_english_names() {
        assert_eq!(Language::English.name(), "English");
        assert_eq!(Language::Spanish.name(), "Spanish");
        assert_eq!(Language::French.name(), "French");
        assert_eq!(Language::German.name(), "German");
        assert_eq!(Language::Japanese.name(), "Japanese");
        assert_eq!(Language::Chinese.name(), "Chinese");
        assert_eq!(Language::Arabic.name(), "Arabic");
        assert_eq!(Language::Icelandic.name(), "Icelandic");
        assert_eq!(Language::Swahili.name(), "Swahili");
        assert_eq!(Language::Maori.name(), "Maori");
    }

    #[test]
    fn from_code_succeeds_for_valid_codes() {
        assert_eq!(Language::from_code("en"), Some(Language::English));
        assert_eq!(Language::from_code("es"), Some(Language::Spanish));
        assert_eq!(Language::from_code("fr"), Some(Language::French));
        assert_eq!(Language::from_code("de"), Some(Language::German));
        assert_eq!(Language::from_code("ja"), Some(Language::Japanese));
        assert_eq!(Language::from_code("zh"), Some(Language::Chinese));
        assert_eq!(Language::from_code("ar"), Some(Language::Arabic));
        assert_eq!(Language::from_code("ru"), Some(Language::Russian));
        assert_eq!(Language::from_code("ko"), Some(Language::Korean));
        assert_eq!(Language::from_code("hi"), Some(Language::Hindi));
        assert_eq!(Language::from_code("sv"), Some(Language::Swedish));
        assert_eq!(Language::from_code("cy"), Some(Language::Welsh));
        assert_eq!(Language::from_code("mi"), Some(Language::Maori));
    }

    #[test]
    fn from_code_returns_none_for_invalid_codes() {
        assert_eq!(Language::from_code("xx"), None);
        assert_eq!(Language::from_code(""), None);
        assert_eq!(Language::from_code("eng"), None);
        assert_eq!(Language::from_code("123"), None);
        assert_eq!(Language::from_code("zz"), None);
    }

    #[test]
    fn all_returns_non_empty_slice() {
        let languages = Language::all();
        assert!(!languages.is_empty());
        assert_eq!(languages.len(), 56);
    }

    #[test]
    fn display_formatting_returns_english_name() {
        assert_eq!(format!("{}", Language::English), "English");
        assert_eq!(format!("{}", Language::Spanish), "Spanish");
        assert_eq!(format!("{}", Language::French), "French");
        assert_eq!(format!("{}", Language::Japanese), "Japanese");
        assert_eq!(format!("{}", Language::Arabic), "Arabic");
        assert_eq!(format!("{}", Language::Welsh), "Welsh");
    }

    #[test]
    fn round_trip_from_code_of_code_returns_same_language() {
        for &lang in Language::all() {
            let code = lang.code();
            let recovered = Language::from_code(code);
            assert_eq!(
                recovered,
                Some(lang),
                "Round-trip failed for {:?} with code {:?}",
                lang,
                code
            );
        }
    }

    #[test]
    fn all_languages_have_unique_codes() {
        let languages = Language::all();
        let mut codes: Vec<&str> = languages.iter().map(|l| l.code()).collect();
        let original_len = codes.len();
        codes.sort();
        codes.dedup();
        assert_eq!(
            codes.len(),
            original_len,
            "Duplicate ISO codes found among languages"
        );
    }

    #[test]
    fn all_languages_have_unique_names() {
        let languages = Language::all();
        let mut names: Vec<&str> = languages.iter().map(|l| l.name()).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            original_len,
            "Duplicate names found among languages"
        );
    }
}
