//! What the desktop app and its browser host share: settings, file locations, logging, device
//! selection, and the few texts both produce.

pub mod applog;
pub mod devices;
pub mod glossary;
pub mod settings;

/// Whether `text` contains kana. Kanji alone is ambiguous with Chinese, so it does not count.
/// "Japanese ⇄ English" translates such text into English and everything else into Japanese.
pub fn looks_japanese(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{3040}'..='\u{309f}' | '\u{30a0}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}'))
}

/// Text marking where a translation was cut after it kept repeating, in the UI language
/// (`en` or `ja`, see [`settings::UiLanguage::locale`]).
pub fn cut_marker(locale: &str) -> &'static str {
    match locale {
        "ja" => "〔繰り返しが止まらなかったため、この文の訳を打ち切りました〕",
        _ => " [translation cut off here: it kept repeating]",
    }
}
