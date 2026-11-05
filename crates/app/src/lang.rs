//! Language and style choices offered in the toolbar.

use engine::pfml::{Form, JA_EASY, LANGUAGES, find_language};
use gpui_kit::SharedString;
use gpui_kit::component::select::SelectItem;
use rust_i18n::t;

use crate::settings::UiLanguage;

/// Source language choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceChoice {
    /// Let the model detect the language.
    Detect,
    Lang(&'static str),
}

impl SourceChoice {
    pub fn code(self) -> Option<&'static str> {
        match self {
            Self::Detect => None,
            Self::Lang(code) => Some(code),
        }
    }
}

/// Target language choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetChoice {
    /// Japanese text goes to English and everything else to Japanese.
    JapaneseEnglish,
    Lang(&'static str),
}

impl TargetChoice {
    /// The concrete target language for `text`.
    pub fn resolve(self, text: &str) -> &'static str {
        match self {
            Self::Lang(code) => code,
            Self::JapaneseEnglish if looks_japanese(text) => "en",
            Self::JapaneseEnglish => "ja",
        }
    }
}

/// Japanese output style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StyleChoice {
    /// Leave the style to the model.
    #[default]
    Default,
    Polite,
    Plain,
}

impl StyleChoice {
    pub fn form(self) -> Option<Form> {
        match self {
            Self::Default => None,
            Self::Polite => Some(Form::Polite),
            Self::Plain => Some(Form::Plain),
        }
    }
}

/// Whether `text` contains kana. Kanji alone is ambiguous with Chinese, so it does not count.
pub fn looks_japanese(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{3040}'..='\u{309f}' | '\u{30a0}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}'))
}

/// Localized display name of a language code.
pub fn language_name(code: &str) -> String {
    let key = format!("lang.{}", code.replace('-', "_").replace("(easy)", "_easy"));
    let name = t!(&key);
    if name == key {
        // Missing translation: fall back to the English name from the model card.
        find_language(code).map_or_else(|| code.to_owned(), |l| l.english_name.to_owned())
    } else {
        name.into_owned()
    }
}

/// An entry in a language or style select.
#[derive(Debug, Clone)]
pub struct Choice<V> {
    value: V,
    title: SharedString,
    /// Lowercased text matched against the search query.
    keywords: String,
}

impl<V> Choice<V> {
    pub fn new(value: V, title: String, extra_keywords: &[&str]) -> Self {
        let mut keywords = title.to_lowercase();
        for k in extra_keywords {
            keywords.push('\n');
            keywords.push_str(&k.to_lowercase());
        }
        Self {
            value,
            title: title.into(),
            keywords,
        }
    }

    fn language(value: V, code: &'static str) -> Self {
        let mut extra = vec![code];
        if let Some(l) = find_language(code) {
            extra.extend([l.english_name, l.native_name]);
        }
        Self::new(value, language_name(code), &extra)
    }
}

impl<V: Clone + PartialEq> SelectItem for Choice<V> {
    type Value = V;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &V {
        &self.value
    }

    fn matches(&self, query: &str) -> bool {
        self.keywords.contains(&query.to_lowercase())
    }
}

pub fn source_choices() -> Vec<Choice<SourceChoice>> {
    let mut choices = vec![Choice::new(
        SourceChoice::Detect,
        t!("source.detect").into_owned(),
        &["auto"],
    )];
    choices.extend(
        LANGUAGES
            .iter()
            .map(|l| Choice::language(SourceChoice::Lang(l.code), l.code)),
    );
    choices
}

pub fn target_choices() -> Vec<Choice<TargetChoice>> {
    let mut choices = vec![Choice::new(
        TargetChoice::JapaneseEnglish,
        t!("target.ja_en").into_owned(),
        &["auto", "ja", "en"],
    )];
    choices.extend(
        LANGUAGES
            .iter()
            .map(|l| Choice::language(TargetChoice::Lang(l.code), l.code)),
    );
    choices.push(Choice::language(TargetChoice::Lang(JA_EASY), JA_EASY));
    choices
}

/// UI language options. Languages are named in their own language.
pub fn ui_language_choices() -> Vec<Choice<UiLanguage>> {
    let system = match crate::settings::system_locale() {
        "ja" => "日本語",
        _ => "English",
    };
    vec![
        Choice::new(
            UiLanguage::System,
            t!("settings.language_system", lang = system).into_owned(),
            &[],
        ),
        Choice::new(UiLanguage::English, "English".to_owned(), &[]),
        Choice::new(UiLanguage::Japanese, "日本語".to_owned(), &[]),
    ]
}

pub fn style_choices() -> Vec<Choice<StyleChoice>> {
    [
        (StyleChoice::Default, "style.default"),
        (StyleChoice::Polite, "style.polite"),
        (StyleChoice::Plain, "style.plain"),
    ]
    .into_iter()
    .map(|(value, key)| Choice::new(value, t!(key).into_owned(), &[]))
    .collect()
}

/// Index of `value` in `choices`.
pub fn position<V: PartialEq>(choices: &[Choice<V>], value: &V) -> Option<usize> {
    choices.iter().position(|c| &c.value == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_japanese_by_kana() {
        assert!(looks_japanese("会議は来週です"));
        assert!(looks_japanese("カタカナ"));
        assert!(!looks_japanese("Hello, world"));
        // Kanji only: could be Chinese.
        assert!(!looks_japanese("你好世界"));
    }

    #[test]
    fn japanese_english_pair() {
        assert_eq!(TargetChoice::JapaneseEnglish.resolve("こんにちは"), "en");
        assert_eq!(TargetChoice::JapaneseEnglish.resolve("Hello"), "ja");
        assert_eq!(TargetChoice::JapaneseEnglish.resolve("你好"), "ja");
        assert_eq!(TargetChoice::Lang("fr").resolve("こんにちは"), "fr");
    }

    #[test]
    fn every_language_has_a_localized_name() {
        for locale in ["en", "ja"] {
            rust_i18n::set_locale(locale);
            for l in LANGUAGES {
                let key = format!("lang.{}", l.code.replace('-', "_"));
                assert_ne!(t!(&key), key, "missing {key} for {locale}");
            }
            assert_ne!(language_name(JA_EASY), JA_EASY);
        }
        rust_i18n::set_locale("en");
    }
}
