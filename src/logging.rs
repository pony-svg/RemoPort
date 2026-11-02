//! Activity log: one timestamped line per thing the user does and per
//! problem met, so a session can be retraced afterwards. It records actions,
//! PC names and addresses, never passwords or other secrets.
//!
//! Written to `remoport.log` in `$REMOPORT_CONFIG_DIR` when that is set,
//! else in `$XDG_STATE_HOME/remoport` or `~/.local/state/remoport`, and
//! echoed to standard error for whoever runs Remoport from a terminal.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// A log grown past this size is moved to `remoport.log.old` at startup.
const MAX_BYTES: u64 = 1024 * 1024;

static LOG: OnceLock<Option<Mutex<File>>> = OnceLock::new();

pub fn log_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("REMOPORT_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("remoport.log"));
    }
    let state = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
    };
    Some(state.join("remoport").join("remoport.log"))
}

fn open() -> Option<Mutex<File>> {
    // Tests exercise error paths that log; keep them out of the user's log.
    if cfg!(test) {
        return None;
    }
    let path = log_path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
        .map(Mutex::new)
}

/// Records one event. Logging never fails the caller: a log that cannot be
/// written is skipped.
pub fn event(message: impl std::fmt::Display) {
    let line = format!(
        "{} {message}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    eprint!("{line}");
    if let Some(file) = LOG.get_or_init(open) {
        if let Ok(mut file) = file.lock() {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
