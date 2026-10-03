//! File helpers shared by config, session and Claude status writers.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `bytes` to `path` via a uniquely named sibling temp file and a
/// rename, so readers never see a half-written file and two ANVIL processes
/// (or two writers) cannot interleave on the same temp name. Creates the
/// parent directory. A symlinked target is resolved first, so the link itself
/// survives the write.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve_link(path);
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tag = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(".{}.{tag}.tmp", std::process::id()));
    let tmp = target.with_file_name(tmp_name);
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Follows an existing symlink (one level, as dotfile setups use) so an
/// atomic replace does not turn the link into a regular file.
fn resolve_link(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::read_link(path).map(|target| {
            if target.is_absolute() {
                target
            } else {
                path.parent().map(|dir| dir.join(&target)).unwrap_or(target)
            }
        }).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("a.json");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files are cleaned up: {leftovers:?}");
    }

    #[test]
    fn concurrent_writers_do_not_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut handles = Vec::new();
        for writer in 0..8u8 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                for round in 0..20 {
                    let payload = format!("{{\"writer\":{writer},\"round\":{round}}}");
                    atomic_write(&path, payload.as_bytes()).unwrap();
                    let read = fs::read_to_string(&path).unwrap();
                    assert!(read.starts_with('{') && read.ends_with('}'), "torn write: {read:?}");
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let final_text = fs::read_to_string(&path).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&final_text).is_ok(), "final file is valid JSON");
    }
}
