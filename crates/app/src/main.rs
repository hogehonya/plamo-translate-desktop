//! PLaMo Translate Desktop.
//!
//! Models are picked and downloaded in the app. A GGUF path given as the first argument or in
//! `PLAMO_MODEL` overrides the saved selection, which is handy for development.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod lang;
mod models;
mod settings;
mod view;

use std::path::PathBuf;

use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::*;

use crate::view::TranslatorView;

rust_i18n::i18n!("locales", fallback = "en");

fn custom_model() -> Option<PathBuf> {
    std::env::args_os()
        .nth(1)
        .or_else(|| std::env::var_os("PLAMO_MODEL"))
        .map(PathBuf::from)
}

/// UI language: `PLAMO_LANG` if set, else the OS locale. Only `en` and `ja` are translated.
fn ui_locale() -> &'static str {
    let requested = std::env::var("PLAMO_LANG")
        .ok()
        .or_else(sys_locale::get_locale)
        .unwrap_or_default()
        .to_lowercase();
    if requested.starts_with("ja") {
        "ja"
    } else {
        "en"
    }
}

fn main() {
    let custom_model = custom_model();
    let settings = settings::Settings::load();
    // Shared with gpui-component, so its built-in strings follow the same locale.
    rust_i18n::set_locale(ui_locale());

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        view::bind_keys(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.), px(720.)),
                cx,
            ))),
            window_min_size: Some(size(px(640.), px(400.))),
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
            cx.new(|cx| TranslatorView::new(custom_model, settings, window, cx))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
