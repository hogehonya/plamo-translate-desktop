//! Registration of the browser host with Chromium-based browsers (native messaging).
//!
//! The extension in `extension/` talks to `plamo-translate-host`, which ships next to the app.
//! A browser starts the host only if a manifest registered under [`HOST_NAME`] names it and
//! allows the extension's ID. On Windows the manifest is a file in the app's data directory,
//! referenced from the registry (Chrome and Edge); on Linux a copy goes into each browser's
//! configuration directory.

use std::io;
use std::path::{Path, PathBuf};

/// Name the extension connects to.
pub const HOST_NAME: &str = "com.github.tanakh.plamo_translate";

/// Extensions allowed to start the host. The ID follows from the `key` in
/// `extension/manifest.json`, so the unpacked extension always has it.
pub const EXTENSION_IDS: &[&str] = &["bcagbgackmnblhlofmpcmjfkfhhogpmh"];

#[cfg(windows)]
const HOST_FILE: &str = "plamo-translate-host.exe";
#[cfg(not(windows))]
const HOST_FILE: &str = "plamo-translate-host";

/// The host executable next to the app, if it is there.
pub fn host_executable() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.with_file_name(HOST_FILE);
    path.is_file().then_some(path)
}

/// The extension's folder: `browser-extension` next to the app in a package, or the source
/// tree's `extension` in a development build.
pub fn extension_dir() -> Option<PathBuf> {
    let packaged = std::env::current_exe()
        .ok()?
        .with_file_name("browser-extension");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extension");
    [packaged, source]
        .into_iter()
        .find(|dir| dir.join("manifest.json").is_file())
}

/// Whether the host is registered, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    NotRegistered,
    /// Registered with this app's host.
    Registered,
    /// Registered with a host elsewhere, such as an older copy of the app.
    Elsewhere(PathBuf),
}

/// The registration as the first browser that has one sees it.
pub fn registration() -> Registration {
    let registered = manifest_paths()
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .find_map(|json| json["path"].as_str().map(PathBuf::from));
    match registered {
        None => Registration::NotRegistered,
        Some(path) if Some(&path) == host_executable().as_ref() => Registration::Registered,
        Some(path) => Registration::Elsewhere(path),
    }
}

fn manifest(host: &Path) -> String {
    let origins: Vec<String> = EXTENSION_IDS
        .iter()
        .map(|id| format!("chrome-extension://{id}/"))
        .collect();
    let json = serde_json::json!({
        "name": HOST_NAME,
        "description": "PLaMo Translate Desktop",
        "path": host,
        "type": "stdio",
        "allowed_origins": origins,
    });
    serde_json::to_string_pretty(&json).unwrap_or_default()
}

/// Registers this app's host. Returns the browsers it was registered with.
pub fn register() -> io::Result<Vec<&'static str>> {
    let host = host_executable().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{HOST_FILE} is not next to the app"),
        )
    })?;
    platform::register(&manifest(&host))
}

pub fn unregister() -> io::Result<()> {
    platform::unregister()
}

/// Manifest files of the current registrations.
fn manifest_paths() -> Vec<PathBuf> {
    platform::manifest_paths()
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::path::PathBuf;

    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use super::HOST_NAME;

    /// Registry locations, per browser, where a key named after the host points to its manifest.
    const BROWSERS: &[(&str, &str)] = &[
        ("Chrome", r"Software\Google\Chrome\NativeMessagingHosts"),
        ("Edge", r"Software\Microsoft\Edge\NativeMessagingHosts"),
    ];

    fn manifest_file() -> PathBuf {
        appdata::settings::data_file("native-messaging-host.json")
    }

    pub fn register(manifest: &str) -> io::Result<Vec<&'static str>> {
        let path = manifest_file();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, manifest)?;
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let mut registered = Vec::new();
        for (browser, key) in BROWSERS {
            let (host_key, _) = user.create_subkey(format!(r"{key}\{HOST_NAME}"))?;
            host_key.set_value("", &path.as_os_str())?;
            registered.push(*browser);
        }
        Ok(registered)
    }

    pub fn unregister() -> io::Result<()> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        for (_, key) in BROWSERS {
            match user.delete_subkey(format!(r"{key}\{HOST_NAME}")) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        match std::fs::remove_file(manifest_file()) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn manifest_paths() -> Vec<PathBuf> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        BROWSERS
            .iter()
            .filter_map(|(_, key)| user.open_subkey(format!(r"{key}\{HOST_NAME}")).ok())
            .filter_map(|host_key| host_key.get_value::<String, _>("").ok())
            .map(PathBuf::from)
            .collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;
    use std::path::PathBuf;

    use super::HOST_NAME;

    /// Configuration directories (under `$XDG_CONFIG_HOME`) of the browsers, per browser.
    const BROWSERS: &[(&str, &str)] = &[
        ("Chrome", "google-chrome"),
        ("Chromium", "chromium"),
        ("Edge", "microsoft-edge"),
        ("Brave", "BraveSoftware/Brave-Browser"),
    ];

    fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }

    fn manifest_file(browser_dir: &str) -> Option<PathBuf> {
        Some(
            config_dir()?
                .join(browser_dir)
                .join("NativeMessagingHosts")
                .join(format!("{HOST_NAME}.json")),
        )
    }

    pub fn register(manifest: &str) -> io::Result<Vec<&'static str>> {
        let config = config_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
        // Browsers that have been run; Chrome's directory is created if none has.
        let installed: Vec<_> = BROWSERS
            .iter()
            .filter(|(_, dir)| config.join(dir).is_dir())
            .collect();
        let targets = if installed.is_empty() {
            vec![&BROWSERS[0]]
        } else {
            installed
        };
        let mut registered = Vec::new();
        for (browser, dir) in targets {
            if let Some(path) = manifest_file(dir) {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, manifest)?;
                registered.push(*browser);
            }
        }
        Ok(registered)
    }

    pub fn unregister() -> io::Result<()> {
        for path in manifest_paths() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn manifest_paths() -> Vec<PathBuf> {
        BROWSERS
            .iter()
            .filter_map(|(_, dir)| manifest_file(dir))
            .filter(|path| path.is_file())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{HOST_NAME, manifest};

    #[test]
    fn manifest_names_the_host_and_the_extension() {
        let json: serde_json::Value =
            serde_json::from_str(&manifest(Path::new("/opt/app/plamo-translate-host"))).unwrap();
        assert_eq!(json["name"], HOST_NAME);
        assert_eq!(json["path"], "/opt/app/plamo-translate-host");
        assert_eq!(json["type"], "stdio");
        assert_eq!(
            json["allowed_origins"][0],
            "chrome-extension://bcagbgackmnblhlofmpcmjfkfhhogpmh/"
        );
    }
}
