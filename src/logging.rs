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
        // Fullness is read from the open handle, not from the path: another
        // window may have rotated the file this handle names, and a rotated
        // file is full by definition. A handle that is full (or unreadable) is
        // closed, which also covers this process's own rotation below.
        if guard.as_ref().is_some_and(|file| !file.metadata().is_ok_and(|meta| meta.len() < self.max_bytes)) {
            *guard = None;
        }
        if guard.is_none() {
            // A rotation that fails (another window holds the old file) must not
            // cost this line: keep appending to the current file and rotate on
            // the next reopen.
            let _ = rotate_if_needed(&self.path, self.max_bytes);
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
        Ok(meta) if meta.len() >= max_bytes => rotate(path),
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// One rename replaces an older `path.1` atomically (the standard library
/// replaces the destination on Windows), so no window sees the old file gone
/// and the new one not yet in place. Every window is its own process and they
/// share the log: when another one rotated between the size check and this
/// rename the source is gone, which is the rotation already done, not a failure.
fn rotate(path: &Path) -> io::Result<bool> {
    let mut old = path.as_os_str().to_os_string();
    old.push(".1");
    match fs::rename(path, &old) {
        Ok(()) => Ok(true),
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

    #[test]
    fn a_rotation_another_window_already_did_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anvil.log");
        // The other window got there first: the log is already `.1`.
        fs::write(dir.path().join("anvil.log.1"), b"older\n").unwrap();
        assert!(!rotate(&path).unwrap());
        assert_eq!(fs::read(dir.path().join("anvil.log.1")).unwrap(), b"older\n");
    }

    #[test]
    fn rotation_replaces_the_previous_rotated_file_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anvil.log");
        fs::write(&path, b"newer\n").unwrap();
        fs::write(dir.path().join("anvil.log.1"), b"older\n").unwrap();
        assert!(rotate(&path).unwrap());
        assert!(!path.exists());
        assert_eq!(fs::read(dir.path().join("anvil.log.1")).unwrap(), b"newer\n");
    }

    /// Every ANVIL window is its own process and they share one log file. A
    /// window that did not do the rotation kept its handle on the file that is
    /// now `anvil.log.1`, judged fullness by the new (small) `anvil.log`, and
    /// so wrote the rest of its life into the rotated file: its lines left the
    /// live log and the rotated file outgrew the cap.
    #[test]
    fn a_window_that_did_not_rotate_follows_the_live_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("anvil.log");
        let first = FileLogger::new(path.clone(), log::LevelFilter::Info, 64);
        let second = FileLogger::new(path.clone(), log::LevelFilter::Info, 64);
        first.write_line("first window starts").unwrap();
        for i in 0..3 {
            second.write_line(&format!("second window line {i} with padding")).unwrap();
        }
        let old = dir.path().join("anvil.log.1");
        assert!(old.exists(), "the second window rotated the shared log");
        let rotated = fs::metadata(&old).unwrap().len();
        first.write_line("first window writes again").unwrap();
        assert!(
            fs::read_to_string(&path).unwrap().contains("first window writes again"),
            "the line must reach the live log, not the rotated file"
        );
        assert_eq!(fs::metadata(&old).unwrap().len(), rotated, "the rotated file is closed for writing");
    }
}
