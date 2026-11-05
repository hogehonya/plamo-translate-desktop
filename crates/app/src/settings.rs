//! Persistent settings and application directories.

use std::path::PathBuf;

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
    /// Context window in tokens.
    pub n_ctx: u32,
}

impl Default for BackendSettings {
    fn default() -> Self {
        Self {
            device: DeviceSetting::Auto,
            n_ctx: 16384,
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
        assert_eq!(s.backend.n_ctx, 16384);
        assert_eq!(s.ui_language, super::UiLanguage::System);
        let s: Settings = serde_json::from_str(r#"{"ui_language":"japanese"}"#).unwrap();
        assert_eq!(s.ui_language, super::UiLanguage::Japanese);
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
