//! Hook trust watches bytes and directory membership, never just timestamps.
use super::{config_refusal, local_path, RepositoryPaths, RepositoryStamp};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_ENTRIES: usize = 2048;
const MAX_FILE: usize = 256 * 1024;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 8;

#[derive(Default)]
pub(super) struct Budget {
    entries: usize,
    bytes: usize,
}

#[derive(Clone)]
pub(super) struct HookSource {
    repository: RepositoryPaths,
    path: PathBuf,
    // Exact bounded bytes avoid hash collisions and stay out of diagnostics.
    snapshot: Arc<Vec<u8>>,
    files: Arc<Vec<String>>,
}

impl HookSource {
    pub(super) fn read(repository: &RepositoryPaths, path: &Path, budget: &mut Budget) -> Result<Self, String> {
        let path = std::path::absolute(path).map_err(|_| config_refusal())?;
        let (snapshot, files) = snapshot(&path, budget)?;
        Ok(Self { repository: repository.clone(), path, snapshot: Arc::new(snapshot), files: Arc::new(files) })
    }
}

/// Check before following any entry. The normal metadata resolver may accept
/// local links, but hook directories and their contents must be plain entries.
fn plain_local(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    local_path(path)?; // network/device/ADS targets are refused before access
    let mut prefix = PathBuf::new();
    let mut last = None;
    for component in path.components() {
        prefix.push(component.as_os_str());
        let metadata = match std::fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(config_refusal()),
        };
        #[cfg(windows)]
        let link = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let link = metadata.file_type().is_symlink();
        if link {
            return Err(config_refusal());
        }
        last = Some(metadata);
    }
    Ok(last)
}

fn part(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    output.extend_from_slice(bytes);
}

fn snapshot(path: &Path, budget: &mut Budget) -> Result<(Vec<u8>, Vec<String>), String> {
    let mut output = Vec::new();
    let mut files = Vec::new();
    if let Some(meta) = plain_local(path)? {
        if !meta.is_dir() {
            return Err(config_refusal());
        }
        walk(path, path, 0, budget, &mut output, &mut files)?;
    }
    Ok((output, files))
}

fn walk(
    root: &Path,
    directory: &Path,
    depth: usize,
    budget: &mut Budget,
    output: &mut Vec<u8>,
    files: &mut Vec<String>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(config_refusal());
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|_| config_refusal())? {
        budget.entries += 1;
        if budget.entries > MAX_ENTRIES {
            return Err(config_refusal());
        }
        entries.push(entry.map_err(|_| config_refusal())?.path());
    }
    entries.sort();
    for path in entries {
        // Refuse redirects even when their names look like samples.
        let meta = plain_local(&path)?.ok_or_else(config_refusal)?;
        if meta.is_dir() {
            walk(root, &path, depth + 1, budget, output, files)?;
        } else if meta.is_file() {
            if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("sample")) {
                continue;
            }
            if meta.len() > MAX_FILE as u64 {
                return Err(config_refusal());
            }
            let bytes = crate::fsutil::read_limited(&path, MAX_FILE).map_err(|_| config_refusal())?;
            budget.bytes += bytes.len();
            if budget.bytes > MAX_BYTES {
                return Err(config_refusal());
            }
            let relative = path.strip_prefix(root).map_err(|_| config_refusal())?;
            part(output, relative.as_os_str().as_encoded_bytes());
            part(output, &bytes);
            files.push(format!("hooks/{}", relative.display()));
        } else {
            return Err(config_refusal());
        }
    }
    Ok(())
}

pub(super) fn all_current(sources: &[HookSource]) -> bool {
    let mut budget = Budget::default();
    sources.iter().all(|source| snapshot(&source.path, &mut budget).is_ok_and(|(bytes, _)| bytes == *source.snapshot))
}

pub(super) fn merge(stamp: &mut RepositoryStamp, sources: &Arc<Vec<HookSource>>) {
    // Discovery order can differ across runs; approval must not.
    let mut sorted: Vec<_> = sources.iter().filter(|source| !source.files.is_empty()).collect();
    sorted.sort_by(|a, b| {
        (&a.repository.root, &a.repository.git_dir, &a.repository.common_dir, &a.path).cmp(&(
            &b.repository.root,
            &b.repository.git_dir,
            &b.repository.common_dir,
            &b.path,
        ))
    });
    for source in sorted {
        let digest = Arc::make_mut(&mut stamp.digest);
        part(digest, b"ANVIL-hook-trust-v1");
        for path in [&source.repository.root, &source.repository.git_dir, &source.repository.common_dir, &source.path] {
            part(digest, path.as_os_str().as_encoded_bytes());
        }
        part(digest, &source.snapshot);
        Arc::make_mut(&mut stamp.hazards).extend(source.files.iter().cloned());
    }
    let hazards = Arc::make_mut(&mut stamp.hazards);
    hazards.sort();
    hazards.dedup();
    stamp.hooks = sources.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, FileTimes};

    fn repo(root: &Path) -> RepositoryPaths {
        RepositoryPaths { root: root.to_owned(), git_dir: root.join(".git"), common_dir: root.join(".git") }
    }

    #[test]
    fn bytes_and_membership_revoke_trust_even_when_timestamps_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("pre-commit.sample"), b"sample").unwrap();
        let empty = HookSource::read(&repo(dir.path()), &path, &mut Budget::default()).unwrap();
        assert!(empty.files.is_empty());
        let directory_time = fs::metadata(&path).unwrap().modified().unwrap();
        let file = path.join("pre-commit");
        fs::write(&file, [0xff, 0, 1]).unwrap(); // binary hooks also participate
        assert!(!all_current(&[empty]));
        let original = HookSource::read(&repo(dir.path()), &path, &mut Budget::default()).unwrap();
        let time = fs::metadata(&file).unwrap().modified().unwrap();
        fs::write(&file, [0xff, 0, 2]).unwrap();
        File::options().write(true).open(&file).unwrap().set_times(FileTimes::new().set_modified(time)).unwrap();
        assert!(!all_current(std::slice::from_ref(&original)));
        fs::write(&file, [0xff, 0, 1]).unwrap();
        assert!(all_current(std::slice::from_ref(&original)));
        fs::remove_file(file).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            File::options()
                .write(true)
                .custom_flags(0x02000000)
                .open(&path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(directory_time))
                .unwrap();
        }
        assert!(!all_current(&[original]));
    }

    #[test]
    fn absent_paths_and_nested_custom_hooks_are_watched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".husky");
        let absent = HookSource::read(&repo(dir.path()), &path, &mut Budget::default()).unwrap();
        fs::create_dir_all(path.join("_")).unwrap();
        fs::write(path.join("_/h"), b"hook helper").unwrap();
        assert!(!all_current(&[absent]));
        let current = HookSource::read(&repo(dir.path()), &path, &mut Budget::default()).unwrap();
        assert_eq!(current.files.len(), 1);
        assert!(all_current(&[current]));
    }

    #[test]
    fn oversized_and_redirected_hooks_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks");
        fs::create_dir(&path).unwrap();
        let file = path.join("pre-commit");
        File::create(&file).unwrap().set_len(MAX_FILE as u64 + 1).unwrap();
        assert!(HookSource::read(&repo(dir.path()), &path, &mut Budget::default()).is_err());
        fs::remove_file(&file).unwrap();
        fs::write(dir.path().join("outside"), b"outside").unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(dir.path().join("outside"), &file).is_ok() {
            assert!(HookSource::read(&repo(dir.path()), &path, &mut Budget::default()).is_err());
        }
    }
}
