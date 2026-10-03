//! Minimal file logger: %APPDATA%\anvil\anvil.log, rotated to anvil.log.1.
//! Never log terminal input or output.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

pub struct FileLogger {
    path: PathBuf,
    level: log::LevelFilter,
    max_bytes: u64,
    file: Mutex<Option<File>>,
}

impl FileLogger {
    pub fn new(path: PathBuf, level: log::LevelFilter, max_bytes: u64) -> FileLogger {
        FileLogger { path, level, max_bytes, file: Mutex::new(None) }
    }

    pub fn write_line(&self, line: &str) -> io::Result<()> {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if rotate_if_needed(&self.path, self.max_bytes)? {
            *guard = None;
        }
        if guard.is_none() {
            if let Some(dir) = self.path.parent() {
                fs::create_dir_all(dir)?;
            }
            *guard = Some(OpenOptions::new().create(true).append(true).open(&self.path)?);
        }
        let file = guard.as_mut().expect("opened above");
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")
    }
}

/// Moves `path` to `path.1` once it reaches `max_bytes`. Returns whether it did.
pub fn rotate_if_needed(path: &Path, max_bytes: u64) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(meta) if meta.len() >= max_bytes => {
            let mut old = path.as_os_str().to_os_string();
            old.push(".1");
            let _ = fs::remove_file(&old);
            fs::rename(path, &old)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // The terminal stack logs raw terminal bytes (vte's unhandled OSC
        // payloads) and profile environment values; neither may ever reach the
        // log file.
        let target = record.target();
        if target.starts_with("vte") || target.starts_with("alacritty_terminal") {
            return;
        }
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let line = format!("{ms} {:<5} {}: {}", record.level(), record.target(), record.args());
        let _ = self.write_line(&line);
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.flush();
        }
    }
}

/// Installs the logger once for the process.
pub fn init(path: PathBuf, level: log::LevelFilter) -> Result<(), log::SetLoggerError> {
    // A static OnceLock hands out a `&'static FileLogger` without leaking.
    static LOGGER: std::sync::OnceLock<FileLogger> = std::sync::OnceLock::new();
    let logger = LOGGER.get_or_init(|| FileLogger::new(path, level, MAX_LOG_BYTES));
    log::set_logger(logger)?;
    log::set_max_level(level);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_when_full() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anvil.log");
        let logger = FileLogger::new(path.clone(), log::LevelFilter::Info, 64);
        for i in 0..10 {
            logger.write_line(&format!("line number {i} with some padding")).unwrap();
        }
        let old = dir.path().join("anvil.log.1");
        assert!(old.exists());
        assert!(fs::metadata(&path).unwrap().len() < 128);
        assert!(fs::metadata(&old).unwrap().len() >= 64);
    }
}
