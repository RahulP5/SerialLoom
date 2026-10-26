//! Log file shared by the app and its elevated helper:
//! `%LOCALAPPDATA%\SerialLoom\logs\serialloom.log`.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// Start a fresh file once it grows past this size.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub fn log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("SerialLoom").join("logs").join("serialloom.log")
}

/// Sends `tracing` output to the log file (and stderr). `who` tags each
/// process, e.g. "app" or "admin".
pub fn init(who: &'static str) {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let result = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_thread_names(true)
            .with_writer(Mutex::new(file))
            .try_init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).try_init(),
    };
    if result.is_ok() {
        tracing::info!(
            "{who} started, version {} (pid {})",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
    }
}

/// The last `lines` lines of the log, for the diagnostics report.
pub fn tail(lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(log_path()) else {
        return "(no log file yet)".into();
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}
