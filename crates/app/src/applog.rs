//! A small append-only application log for events that change model files or state.
//!
//! Lines go to stderr and to `app.log` in the data directory, so unexpected changes (a model
//! file disappearing, for example) can be traced after the fact. Only a few lines are written
//! per run (start-up, devices, model loads, downloads and deletions; never translated text), and
//! a log over [`MAX_SIZE`] is moved to `app.log.1` at start-up, so at most two files of about
//! that size exist.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Size at which the log is rotated.
const MAX_SIZE: u64 = 1 << 20;

/// Opens (or creates) the log file. Logging still goes to stderr if this fails.
pub fn init(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    rotate(path, MAX_SIZE);
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => *LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file),
        Err(e) => eprintln!("cannot open log file {}: {e}", path.display()),
    }
}

/// Moves `path` to `path.1` (replacing an older one) when it has grown beyond `max_size`.
fn rotate(path: &Path, max_size: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max_size) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(path, old);
    }
}

pub fn write(message: &str) {
    let line = format!(
        "[{}] {message}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    eprintln!("{line}");
    if let Some(file) = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(file, "{line}");
    }
}

#[macro_export]
macro_rules! app_log {
    ($($arg:tt)*) => {
        $crate::applog::write(&format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::rotate;

    #[test]
    fn rotates_only_large_logs() {
        let dir = std::env::temp_dir().join(format!("plamo-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("app.log");
        let old = dir.join("app.log.1");

        std::fs::write(&log, b"small").unwrap();
        rotate(&log, 10);
        assert!(log.exists() && !old.exists());

        std::fs::write(&log, b"more than ten bytes").unwrap();
        rotate(&log, 10);
        assert!(!log.exists());
        assert_eq!(std::fs::read(&old).unwrap(), b"more than ten bytes");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
