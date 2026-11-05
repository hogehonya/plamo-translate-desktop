//! Persistent settings and application directories.

use std::path::PathBuf;

use engine::document::DocumentOptions;
use serde::{Deserialize, Serialize};

fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "plamo-translate-desktop")
}

/// Directory holding downloaded models.
pub fn models_dir() -> PathBuf {
    project_dirs().map_or_else(
        || PathBuf::from("models"),
        |dirs| dirs.data_local_dir().join("models"),
    )
}

/// Catalog entries found on Hugging Face, cached for offline starts.
pub fn catalog_cache_path() -> PathBuf {
    data_file("catalog.json")
}

/// The application log, next to the models.
pub fn log_path() -> PathBuf {
    data_file("app.log")
}

/// The browser host's log, next to the app's.
pub fn host_log_path() -> PathBuf {
    data_file("host.log")
}

/// A file in the data directory, which holds the models directory.
pub fn data_file(name: &str) -> PathBuf {
    models_dir()
        .parent()
        .map_or_else(|| PathBuf::from(name), |dir| dir.join(name))
}

/// Where the settings are saved.
pub fn settings_path() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.config_dir().join("settings.json"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Catalog id of the selected model.
    pub model: Option<String>,
    /// Whether the user accepted the PLaMo Community License before downloading.
    pub license_accepted: bool,
    pub backend: BackendSettings,
    /// Glossary text, one `source = target` per line.
    pub glossary: String,
    pub ui_language: UiLanguage,
    pub theme: ThemeSetting,
    pub document: DocumentSettings,
    /// Scroll the source and the translation together, and show the counterpart of the caret.
    pub sync_scroll: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: None,
            license_accepted: false,
            backend: BackendSettings::default(),
            glossary: String::new(),
            ui_language: UiLanguage::default(),
            theme: ThemeSetting::default(),
            document: DocumentSettings::default(),
            sync_scroll: true,
        }
    }
}

/// How long texts are split (see `engine::document`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentSettings {
    /// Upper bound for one chunk, in tokens.
    pub chunk_tokens: usize,
    /// Upper bound for the history sent with each chunk, in tokens.
    pub history_tokens: usize,
}

impl Default for DocumentSettings {
    fn default() -> Self {
        let defaults = DocumentOptions::default();
        Self {
            chunk_tokens: defaults.max_chunk_tokens,
            history_tokens: defaults.max_history_tokens,
        }
    }
}

impl DocumentSettings {
    /// The options for a translation; `cut_marker` marks passages that were cut.
    pub fn options(self, cut_marker: String) -> DocumentOptions {
        DocumentOptions {
            // A chunk of a few tokens would only split sentences apart.
            max_chunk_tokens: self.chunk_tokens.max(64),
            max_history_tokens: self.history_tokens,
            cut_marker,
        }
    }
}

/// Light or dark appearance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeSetting {
    /// Follow the operating system, including later changes.
    #[default]
    System,
    Light,
    Dark,
}

/// Language of the user interface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiLanguage {
    /// Follow the operating system.
    #[default]
    System,
    English,
    Japanese,
}

impl UiLanguage {
    /// The locale code to use. `PLAMO_LANG` overrides everything, for development.
    pub fn locale(self) -> &'static str {
        if let Some(code) = std::env::var("PLAMO_LANG")
            .ok()
            .and_then(|v| supported_locale(&v))
        {
            return code;
        }
        match self {
            Self::English => "en",
            Self::Japanese => "ja",
            Self::System => system_locale(),
        }
    }
}

/// The OS language if the UI is translated into it, else English.
pub fn system_locale() -> &'static str {
    sys_locale::get_locale()
        .and_then(|l| supported_locale(&l))
        .unwrap_or("en")
}

fn supported_locale(requested: &str) -> Option<&'static str> {
    let requested = requested.to_lowercase();
    if requested.starts_with("ja") {
        Some("ja")
    } else if requested.starts_with("en") {
        Some("en")
    } else {
        None
    }
}

/// Where and how the model runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendSettings {
    pub device: DeviceSetting,
}

impl Default for BackendSettings {
    fn default() -> Self {
        Self {
            device: DeviceSetting::Auto,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "name")]
pub enum DeviceSetting {
    /// The discrete GPU with the most free memory, else the CPU.
    #[default]
    Auto,
    Cpu,
    /// A ggml device by name, such as `Vulkan0`.
    Device(String),
}

impl Settings {
    /// Loads the settings, falling back to defaults if the file is missing or unreadable.
    ///
    /// A file that does not parse is renamed to `settings.json.invalid` rather than overwritten
    /// by the next save, so the license acceptance and the glossary in it can be recovered.
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            let aside = path.with_extension("json.invalid");
            let moved = std::fs::rename(&path, &aside).is_ok();
            crate::app_log!(
                "ignoring invalid settings in {}: {e}{}",
                path.display(),
                if moved {
                    format!("; kept as {}", aside.display())
                } else {
                    String::new()
                }
            );
            Self::default()
        })
    }

    /// Saves the settings atomically. Failures are reported but not fatal.
    pub fn save(&self) {
        let Some(path) = settings_path() else {
            return;
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            crate::app_log!("failed to save settings to {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
        let s: Settings = serde_json::from_str(r#"{"model":"x","unknown":1}"#).unwrap();
        assert_eq!(s.model.as_deref(), Some("x"));
        assert!(!s.license_accepted);
        // Settings from older versions with a context length still load.
        let s: Settings =
            serde_json::from_str(r#"{"backend":{"device":{"kind":"cpu"},"n_ctx":4096}}"#).unwrap();
        assert_eq!(s.backend.device, super::DeviceSetting::Cpu);
        assert_eq!(s.ui_language, super::UiLanguage::System);
        let s: Settings = serde_json::from_str(r#"{"ui_language":"japanese"}"#).unwrap();
        assert_eq!(s.ui_language, super::UiLanguage::Japanese);
        assert_eq!(s.theme, super::ThemeSetting::System);
        let s: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(s.theme, super::ThemeSetting::Dark);
        assert_eq!(s.document, super::DocumentSettings::default());
        assert!(s.sync_scroll);
        let s: Settings = serde_json::from_str(r#"{"document":{"chunk_tokens":512}}"#).unwrap();
        assert_eq!(s.document.chunk_tokens, 512);
        assert_eq!(
            s.document.history_tokens,
            super::DocumentSettings::default().history_tokens
        );
    }

    #[test]
    fn device_setting_round_trips() {
        use super::DeviceSetting;
        for d in [
            DeviceSetting::Auto,
            DeviceSetting::Cpu,
            DeviceSetting::Device("Vulkan0".into()),
        ] {
            let json = serde_json::to_string(&d).unwrap();
            assert_eq!(serde_json::from_str::<DeviceSetting>(&json).unwrap(), d);
        }
    }
}
