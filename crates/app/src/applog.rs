//! A small append-only application log for events that change model files or state.
//!
//! Lines go to stderr and to `app.log` in the data directory, so unexpected changes (a model
//! file disappearing, for example) can be traced after the fact.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Opens (or creates) the log file. Logging still goes to stderr if this fails.
pub fn init(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => *LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file),
        Err(e) => eprintln!("cannot open log file {}: {e}", path.display()),
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
