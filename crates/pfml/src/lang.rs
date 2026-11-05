use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// A language code accepted by PLaMo 3 Translate.
///
/// Valid codes are a lowercase primary subtag of 2 or 3 letters, optionally followed by a
/// 4-letter lowercase script subtag (`ja`, `ceb`, `zh-hans`), or the special code `ja(easy)`.
/// Region subtags (`en-us`) are not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LangCode(String);

/// Code of the "easy Japanese" target accepted by the official chat template.
///
/// The model card does not document it, so it is treated as experimental.
pub const JA_EASY: &str = "ja(easy)";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "invalid language code {0:?}: expected a lowercase code such as en, ja, zh-hans or ja(easy)"
)]
pub struct LangCodeError(pub String);

impl LangCode {
    pub fn new(code: &str) -> Result<Self, LangCodeError> {
        if is_valid_code(code) {
            Ok(Self(code.to_owned()))
        } else {
            Err(LangCodeError(code.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is plain Japanese (`ja`), the only target that accepts a style (`form`).
    pub fn is_japanese(&self) -> bool {
        self.0 == "ja"
    }

    /// Looks up display metadata for this code, if it is one of the supported languages.
    pub fn language(&self) -> Option<&'static Language> {
        find_language(&self.0)
    }
}

impl FromStr for LangCode {
    type Err = LangCodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for LangCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_valid_code(code: &str) -> bool {
    if code == JA_EASY {
        return true;
    }
    let lower = |s: &str| s.bytes().all(|b| b.is_ascii_lowercase());
    let mut parts = code.split('-');
    let primary = parts.next().unwrap_or_default();
    let script = parts.next();
    if parts.next().is_some() {
        return false;
    }
    let primary_ok = matches!(primary.len(), 2 | 3) && lower(primary);
    let script_ok = script.is_none_or(|s| s.len() == 4 && lower(s));
    primary_ok && script_ok
}

/// Display metadata for a supported language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    pub code: &'static str,
    pub english_name: &'static str,
    pub native_name: &'static str,
}

const fn lang(
    code: &'static str,
    english_name: &'static str,
    native_name: &'static str,
) -> Language {
    Language {
        code,
        english_name,
        native_name,
    }
}

/// The 53 languages listed on the PLaMo 3 Translate model card, in model card order.
pub const LANGUAGES: &[Language] = &[
    lang("ja", "Japanese", "日本語"),
    lang("en", "English", "English"),
    lang("zh-hans", "Chinese (Simplified)", "简体中文"),
    lang("zh-hant", "Chinese (Traditional)", "繁體中文"),
    lang("ko", "Korean", "한국어"),
    lang("ar", "Arabic", "العربية"),
    lang("az", "Azerbaijani", "Azərbaycan dili"),
    lang("be", "Belarusian", "Беларуская"),
    lang("bg", "Bulgarian", "Български"),
    lang("bn", "Bengali", "বাংলা"),
    lang("ca", "Catalan", "Català"),
    lang("ceb", "Cebuano", "Cebuano"),
    lang("de", "German", "Deutsch"),
    lang("es", "Spanish", "Español"),
    lang("et", "Estonian", "Eesti"),
    lang("eu", "Basque", "Euskara"),
    lang("fa", "Persian", "فارسی"),
    lang("fr", "French", "Français"),
    lang("gu", "Gujarati", "ગુજરાતી"),
    lang("he", "Hebrew", "עברית"),
    lang("hi", "Hindi", "हिन्दी"),
    lang("hr", "Croatian", "Hrvatski"),
    lang("hu", "Hungarian", "Magyar"),
    lang("hy", "Armenian", "Հայերեն"),
    lang("id", "Indonesian", "Bahasa Indonesia"),
    lang("it", "Italian", "Italiano"),
    lang("jv", "Javanese", "Basa Jawa"),
    lang("ka", "Georgian", "ქართული"),
    lang("kk", "Kazakh", "Қазақ тілі"),
    lang("km", "Khmer", "ខ្មែរ"),
    lang("lt", "Lithuanian", "Lietuvių"),
    lang("lv", "Latvian", "Latviešu"),
    lang("ml", "Malayalam", "മലയാളം"),
    lang("mr", "Marathi", "मराठी"),
    lang("ms", "Malay", "Bahasa Melayu"),
    lang("my", "Burmese", "မြန်မာ"),
    lang("ne", "Nepali", "नेपाली"),
    lang("nl", "Dutch", "Nederlands"),
    lang("pa", "Punjabi", "ਪੰਜਾਬੀ"),
    lang("pl", "Polish", "Polski"),
    lang("ru", "Russian", "Русский"),
    lang("si", "Sinhala", "සිංහල"),
    lang("sk", "Slovak", "Slovenčina"),
    lang("sl", "Slovenian", "Slovenščina"),
    lang("sr", "Serbian", "Српски"),
    lang("sw", "Swahili", "Kiswahili"),
    lang("ta", "Tamil", "தமிழ்"),
    lang("te", "Telugu", "తెలుగు"),
    lang("th", "Thai", "ไทย"),
    lang("tl", "Tagalog", "Tagalog"),
    lang("ur", "Urdu", "اردو"),
    lang("uz", "Uzbek", "Oʻzbekcha"),
    lang("vi", "Vietnamese", "Tiếng Việt"),
];

pub fn find_language(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_table_matches_model_card() {
        assert_eq!(LANGUAGES.len(), 53);
        let mut codes: Vec<_> = LANGUAGES.iter().map(|l| l.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 53, "duplicate language codes");
        for l in LANGUAGES {
            assert!(LangCode::new(l.code).is_ok(), "{} should be valid", l.code);
        }
    }

    #[test]
    fn accepts_valid_codes() {
        for code in ["ja", "en", "ceb", "zh-hans", "zh-hant", "ja(easy)"] {
            assert!(LangCode::new(code).is_ok(), "{code}");
        }
    }

    #[test]
    fn rejects_invalid_codes() {
        for code in [
            "",
            "j",
            "jpan",
            "JA",
            "en-US",
            "en-us",
            "zh-han",
            "zh-hans-x",
            "ja(Easy)",
            "e1",
        ] {
            assert!(LangCode::new(code).is_err(), "{code:?}");
        }
    }

    #[test]
    fn only_plain_ja_is_japanese() {
        assert!(LangCode::new("ja").unwrap().is_japanese());
        assert!(!LangCode::new(JA_EASY).unwrap().is_japanese());
        assert!(!LangCode::new("en").unwrap().is_japanese());
    }
}
