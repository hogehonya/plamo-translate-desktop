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
    models_dir().parent().map_or_else(
        || PathBuf::from("catalog.json"),
        |dir| dir.join("catalog.json"),
    )
}

/// The application log, next to the models.
pub fn log_path() -> PathBuf {
    models_dir()
        .parent()
        .map_or_else(|| PathBuf::from("app.log"), |dir| dir.join("app.log"))
}

fn settings_path() -> Option<PathBuf> {
    project_dirs().map(|dirs| dirs.config_dir().join("settings.json"))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn options(self) -> DocumentOptions {
        DocumentOptions {
            // A chunk of a few tokens would only split sentences apart.
            max_chunk_tokens: self.chunk_tokens.max(64),
            max_history_tokens: self.history_tokens,
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
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("ignoring invalid settings in {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
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
            eprintln!("failed to save settings to {}: {e}", path.display());
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
