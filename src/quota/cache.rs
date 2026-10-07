//! Files shared by all ANVIL windows in `%LOCALAPPDATA%\anvil`: `quota.json`
//! (the snapshot; no secrets), `quota.lock` (held open exclusively by the one
//! window that polls providers) and `quota.refresh` (touched to ask that
//! window for an immediate cycle).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::model::Snapshot;
use crate::fsutil::atomic_write;

const MAX_CACHE: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Paths {
    pub snapshot: PathBuf,
    pub lock: PathBuf,
    pub refresh: PathBuf,
    pub schedule: PathBuf,
}

impl Paths {
    pub fn in_dir(dir: &Path) -> Paths {
        Paths {
            snapshot: dir.join("quota.json"),
            lock: dir.join("quota.lock"),
            refresh: dir.join("quota.refresh"),
            schedule: dir.join("quota-schedule.json"),
        }
    }

    /// `%LOCALAPPDATA%\anvil`, the parent of the per-process `run` folders.
    pub fn default_dir() -> PathBuf {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("anvil")
    }
}

pub fn read(path: &Path) -> Option<Snapshot> {
    // Another window's rename or a scanner can hold the file for a moment, and
    // an observer that gives up then keeps the stale snapshot until the next
    // cycle: the read bounds itself on the handle and retries those errors.
    let snapshot: Snapshot = serde_json::from_slice(&crate::fsutil::read_limited(path, MAX_CACHE).ok()?).ok()?;
    (snapshot.version == Snapshot::VERSION).then_some(snapshot)
}

pub fn write(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let text = serde_json::to_vec_pretty(snapshot).map_err(io::Error::other)?;
    atomic_write(path, &text)
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Held by the polling window. Windows releases the share lock when the
/// handle closes — on drop, or when the process ends however it ends.
pub struct Leader {
    _file: File,
}

pub fn try_lead(path: &Path) -> Option<Leader> {
    use std::os::windows::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .ok()
        .map(|file| Leader { _file: file })
}

/// Any window asks the leader for a cycle by rewriting this file.
pub fn request_refresh(path: &Path) -> io::Result<()> {
    let stamp = super::time::now_unix().to_string();
    atomic_write(path, stamp.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::model::{ProviderId, ProviderSnapshot, ProviderState};

    #[test]
    fn only_one_holder_leads_until_it_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let first = try_lead(&paths.lock).expect("first window leads");
        assert!(try_lead(&paths.lock).is_none(), "second window must observe");
        drop(first);
        assert!(try_lead(&paths.lock).is_some(), "the lock passes on");
    }

    #[test]
    fn snapshot_round_trip_and_rejections() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        assert!(read(&paths.snapshot).is_none());
        let snapshot = Snapshot {
            version: Snapshot::VERSION,
            providers: vec![ProviderSnapshot {
                id: ProviderId::Claude,
                plan: Some("Max".into()),
                source: "Claude Code".into(),
                state: ProviderState::Ok,
                windows: Vec::new(),
                balances: Vec::new(),
                fetched_at: Some(1),
                checked_at: 1,
            }],
        };
        write(&paths.snapshot, &snapshot).unwrap();
        assert_eq!(read(&paths.snapshot), Some(snapshot));
        std::fs::write(&paths.snapshot, r#"{"version": 99, "providers": []}"#).unwrap();
        assert!(read(&paths.snapshot).is_none(), "unknown versions are ignored");
        std::fs::write(&paths.snapshot, "garbage").unwrap();
        assert!(read(&paths.snapshot).is_none());
    }

    #[test]
    fn a_snapshot_held_for_a_moment_is_still_read() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        write(&paths.snapshot, &Snapshot::default()).unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&paths.snapshot).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(held);
        });
        assert_eq!(read(&paths.snapshot), Some(Snapshot::default()));
        release.join().unwrap();
    }

    #[test]
    fn refresh_requests_change_the_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        assert!(mtime(&paths.refresh).is_none());
        request_refresh(&paths.refresh).unwrap();
        assert!(mtime(&paths.refresh).is_some());
    }
}
