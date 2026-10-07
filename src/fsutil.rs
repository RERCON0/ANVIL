//! File helpers shared by writers and workspace file operations.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Limit the read itself, not just a metadata check that can race a writer.
pub fn read_limited(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    let file = retry_file_sharing(|| fs::File::open(path))?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((cap as u64).saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() > cap {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file exceeds size limit"));
    }
    Ok(bytes)
}

#[cfg(test)]
#[test]
fn bounded_reads_accept_the_exact_cap_and_reject_more() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bounded");
    fs::write(&path, b"1234").unwrap();
    assert_eq!(read_limited(&path, 4).unwrap(), b"1234");
    assert!(read_limited(&path, 3).is_err());
    assert!(read_limited(dir.path(), 4).is_err());
}

/// Writes `bytes` to `path` via a uniquely named sibling temp file and a
/// rename, so readers never see a half-written file and two ANVIL processes
/// (or two writers) cannot interleave on the same temp name. Creates the
/// parent directory. A symlinked target is resolved first, so the link itself
/// survives the write.
///
/// Atomic for readers, not durable across a power cut: the temp file is
/// flushed, but the rename itself is not written through (a write-through
/// `MoveFileExW` cannot replace a file other readers hold open, which the
/// standard rename can). The replacement is a new file, so it takes the ACL and
/// attributes the directory gives new files, not those of the file it replaces.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve_link(path);
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let name =
        target.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tag = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(".{}.{tag}.tmp", std::process::id()));
    let tmp = target.with_file_name(tmp_name);
    let result = (|| -> io::Result<()> {
        // create_new: never write through something already at the temp name
        // (a leftover or a planted link), as docs/DECISIONS.md records.
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        retry_file_sharing(|| fs::rename(&tmp, &target))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// NTFS can briefly deny an open/replace while another atomic replacement is
/// finishing or a reader/scanner holds the destination. Retry only the Windows
/// access/sharing/lock errors, with one short deadline; never delete the target
/// as a fallback or retry writes that could have partially modified a file.
fn retry_file_sharing<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    #[cfg(windows)]
    {
        use std::time::{Duration, Instant};
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match operation() {
                Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }
    #[cfg(not(windows))]
    operation()
}

/// Follows an existing symlink (one level, as dotfile setups use) so an
/// atomic replace does not turn the link into a regular file.
fn resolve_link(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::read_link(path)
            .map(|target| {
                if target.is_absolute() {
                    target
                } else {
                    path.parent().map(|dir| dir.join(&target)).unwrap_or(target)
                }
            })
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// Moves a complete file or directory to the Windows Recycle Bin.
///
/// Recycling failure or cancellation is an error; permanent deletion is never
/// a fallback. Other platforms are explicitly unsupported.
pub fn recycle_path(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        recycling::recycle(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Recycle Bin operations are only supported on Windows".to_owned())
    }
}

#[cfg(windows)]
mod recycling {
    use std::cell::Cell;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Path, Prefix};
    use windows::core::{implement, ComObject, IUnknownImpl, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{E_ABORT, E_NOTIMPL, S_OK};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
        SHCreateItemFromParsingName, FOFX_ADDUNDORECORD, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FOF_NOCONFIRMATION,
        FOF_NOERRORUI, FOF_NO_CONNECTED_ELEMENTS, FOF_SILENT, TSF_DELETE_RECYCLE_IF_POSSIBLE,
    };

    struct Apartment;

    impl Drop for Apartment {
        fn drop(&mut self) {
            // This guard stays on the initialized thread and outlives every COM object.
            unsafe { CoUninitialize() };
        }
    }

    /// NUL-terminated path for the Shell, which parses only `\` separators: a
    /// `/` in the string makes `SHCreateItemFromParsingName` fail with
    /// E_INVALIDARG. Windows file names cannot contain `/`, so the mapping is
    /// lossless and callers may keep using either separator.
    pub(super) fn shell_path(path: &Path) -> Vec<u16> {
        let mut wide: Vec<u16> =
            path.as_os_str().encode_wide().map(|unit| if unit == b'/' as u16 { b'\\' as u16 } else { unit }).collect();
        wide.push(0);
        wide
    }

    /// Whether a delete callback means "the item is in the Recycle Bin".
    ///
    /// The Shell reports recycling with copy-engine success codes rather than
    /// S_OK (`COPYENGINE_S_*`), so any successful HRESULT counts; a permanent
    /// deletion arrives as success without a Recycle Bin item and must fail.
    pub(super) fn delete_outcome(result: HRESULT, recycled: bool) -> HRESULT {
        if result.is_ok() && recycled {
            S_OK
        } else if result.is_err() {
            result
        } else {
            E_ABORT
        }
    }

    pub(super) fn recycle(path: &Path) -> Result<(), String> {
        // Reject embedded NULs before passing a path to a null-terminated API.
        if path.as_os_str().encode_wide().any(|unit| unit == 0) {
            return Err("Cannot recycle a path containing NUL".to_owned());
        }
        let path = std::path::absolute(path).map_err(|e| e.to_string())?;
        let local_disk = matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        );
        let stream = path.components().any(|part| match part {
            Component::Normal(name) => name.encode_wide().any(|unit| unit == b':' as u16),
            _ => false,
        });
        if !local_disk || path.file_name().is_none() || stream {
            return Err("Recycle Bin supports local files and directories, not roots, network/device paths or streams"
                .to_owned());
        }
        // Only accept existing filesystem items, not arbitrary Shell namespace names.
        std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let wide = shell_path(&path);

        // IFileOperation requires STA. A preexisting incompatible apartment is
        // an error, not grounds to switch to a less safe deletion API. S_FALSE
        // is success too and must be balanced by CoUninitialize.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|e| format!("Cannot initialize Recycle Bin operation in an STA: {e}"))?;
        let _apartment = Apartment;

        let result = (|| -> windows::core::Result<()> {
            // SAFETY: COM is initialized on this thread, wide is NUL-terminated
            // and lives through parsing; generated bindings own interface lifetimes.
            let operation: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER) }?;
            unsafe {
                operation.SetOperationFlags(
                    FOFX_RECYCLEONDELETE
                        | FOFX_ADDUNDORECORD
                        | FOF_NOERRORUI
                        | FOF_NOCONFIRMATION
                        | FOFX_EARLYFAILURE
                        | FOF_SILENT
                        | FOF_NO_CONNECTED_ELEMENTS,
                )?;
            }
            let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }?;
            let guard = ComObject::new(RecycleGuard { refused: Cell::new(false), deleted: Cell::new(None) });
            let sink: IFileOperationProgressSink = guard.to_interface();
            // Advise covers every item, including any Shell fallback operation.
            let cookie = unsafe { operation.Advise(&sink) }?;
            let performed = (|| -> windows::core::Result<()> {
                unsafe {
                    operation.DeleteItem(&item, None)?;
                    operation.PerformOperations()?;
                    if operation.GetAnyOperationsAborted()?.as_bool() {
                        return Err(E_ABORT.into());
                    }
                }
                match guard.deleted.get() {
                    Some(S_OK) if !guard.refused.get() => Ok(()),
                    Some(hr) if hr.is_err() => Err(hr.into()),
                    _ => Err(E_ABORT.into()),
                }
            })();
            let unadvised = unsafe { operation.Unadvise(cookie) };
            performed.and(unadvised)
        })();
        result.map_err(|e| format!("Could not move {} to the Recycle Bin: {e}", path.display()))
    }

    #[implement(IFileOperationProgressSink)]
    struct RecycleGuard {
        refused: Cell<bool>,
        deleted: Cell<Option<HRESULT>>,
    }

    #[allow(non_snake_case)]
    impl IFileOperationProgressSink_Impl for RecycleGuard_Impl {
        fn StartOperations(&self) -> windows::core::Result<()> {
            Ok(())
        }

        fn FinishOperations(&self, result: HRESULT) -> windows::core::Result<()> {
            result.ok()
        }

        fn PreDeleteItem(&self, flags: u32, _: Option<&IShellItem>) -> windows::core::Result<()> {
            // FOF_ALLOWUNDO alone promises undo only "if possible". Veto the
            // Shell's permanent-delete path before it can touch the item.
            // https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-predeleteitem
            if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
                self.get_impl().refused.set(true);
                Err(E_ABORT.into())
            } else {
                Ok(())
            }
        }

        fn PostDeleteItem(
            &self,
            _: u32,
            _: Option<&IShellItem>,
            result: HRESULT,
            recycled: Option<&IShellItem>,
        ) -> windows::core::Result<()> {
            // A successful PerformOperations can still mean skipped/cancelled.
            // PostDeleteItem supplies the actual item HRESULT and a non-null
            // Recycle Bin item only for recycling, not permanent deletion.
            let result = delete_outcome(result, recycled.is_some());
            let state = self.get_impl();
            if !matches!(state.deleted.get(), Some(hr) if hr.is_err()) {
                state.deleted.set(Some(result));
            }
            result.ok()
        }

        // No rename/copy/move/new operations are queued. Reject unexpected work
        // rather than allowing the Shell to substitute a different operation.
        fn PreRenameItem(&self, _: u32, _: Option<&IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn PostRenameItem(
            &self,
            _: u32,
            _: Option<&IShellItem>,
            _: &PCWSTR,
            _: HRESULT,
            _: Option<&IShellItem>,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn PreMoveItem(
            &self,
            _: u32,
            _: Option<&IShellItem>,
            _: Option<&IShellItem>,
            _: &PCWSTR,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn PostMoveItem(
            &self,
            _: u32,
            _: Option<&IShellItem>,
            _: Option<&IShellItem>,
            _: &PCWSTR,
            _: HRESULT,
            _: Option<&IShellItem>,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn PreCopyItem(
            &self,
            _: u32,
            _: Option<&IShellItem>,
            _: Option<&IShellItem>,
            _: &PCWSTR,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn PostCopyItem(
            &self,
            _: u32,
            _: Option<&IShellItem>,
            _: Option<&IShellItem>,
            _: &PCWSTR,
            _: HRESULT,
            _: Option<&IShellItem>,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn PreNewItem(&self, _: u32, _: Option<&IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn PostNewItem(
            &self,
            _: u32,
            _: Option<&IShellItem>,
            _: &PCWSTR,
            _: &PCWSTR,
            _: u32,
            _: HRESULT,
            _: Option<&IShellItem>,
        ) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
            Ok(())
        }
        fn ResetTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn PauseTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn ResumeTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn delete_outcome_demands_a_recycle_bin_item() {
        use windows::Win32::Foundation::E_ABORT;
        use windows::Win32::UI::Shell::TSF_DELETE_RECYCLE_IF_POSSIBLE;
        // The SDK puts the recycle flag in bit 7; the sink vetoes without it.
        assert_eq!(TSF_DELETE_RECYCLE_IF_POSSIBLE.0, 0x80);
        // Recycling comes back as a copy-engine success code, not S_OK.
        assert_eq!(
            super::recycling::delete_outcome(windows::core::HRESULT(0x0027_0008), true),
            windows::core::HRESULT(0)
        );
        // A permanent delete reports success without a Recycle Bin item.
        assert_eq!(super::recycling::delete_outcome(windows::core::HRESULT(0), false), E_ABORT);
        // A real failure keeps its own code.
        assert_eq!(
            super::recycling::delete_outcome(windows::core::HRESULT(0x8027_0027u32 as i32), false),
            windows::core::HRESULT(0x8027_0027u32 as i32)
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_paths_turn_slashes_into_backslashes() {
        let wide = super::recycling::shell_path(Path::new("C:/repo/sub/file.txt"));
        let text = String::from_utf16(&wide[..wide.len() - 1]).unwrap();
        assert_eq!(text, "C:\\repo\\sub\\file.txt");
        assert_eq!(*wide.last().unwrap(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn recycle_rejects_nul_and_stream_paths_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keep.txt");
        fs::write(&path, b"must survive").unwrap();
        for suffix in ["\0ignored", ":stream"] {
            let mut unsupported = path.as_os_str().to_os_string();
            unsupported.push(suffix);
            assert!(recycle_path(Path::new(&unsupported)).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"must survive");
        }
    }

    #[cfg(windows)]
    #[test]
    fn recycle_fails_closed_in_an_incompatible_com_apartment() {
        // A fresh MTA thread deterministically fails before invoking the Shell,
        // independently of the user's Recycle Bin settings.
        std::thread::spawn(|| {
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
            struct Apartment;
            impl Drop for Apartment {
                fn drop(&mut self) {
                    unsafe { CoUninitialize() };
                }
            }
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().unwrap();
            let _apartment = Apartment;
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("keep");
            fs::create_dir(&folder).unwrap();
            let file = folder.join(".ignored");
            fs::write(&file, b"complete contents").unwrap();
            assert!(recycle_path(&folder).is_err());
            assert_eq!(fs::read(file).unwrap(), b"complete contents");
        })
        .join()
        .unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn recycle_is_unsupported_and_preserves_files_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".ignored");
        fs::write(&file, b"complete contents").unwrap();
        assert!(recycle_path(&file).is_err());
        assert!(recycle_path(dir.path()).is_err());
        assert_eq!(fs::read(file).unwrap(), b"complete contents");
    }

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
                    let read = String::from_utf8(read_limited(&path, 128).unwrap()).unwrap();
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

    #[cfg(windows)]
    #[test]
    fn atomic_replace_waits_for_a_temporary_reader_lock() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::sync::mpsc;
        use std::time::Duration;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        atomic_write(&path, b"old complete payload").unwrap();
        // A real handle which permits reads/writes but denies replacement.
        let reader =
            fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).open(&path).unwrap();
        let (sender, receiver) = mpsc::channel();
        let output = path.clone();
        let writer = std::thread::spawn(move || sender.send(atomic_write(&output, b"new complete payload")).unwrap());
        assert!(matches!(receiver.recv_timeout(Duration::from_millis(50)), Err(mpsc::RecvTimeoutError::Timeout)));
        assert_eq!(read_limited(&path, 128).unwrap(), b"old complete payload");
        drop(reader);
        receiver.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        writer.join().unwrap();
        assert_eq!(read_limited(&path, 128).unwrap(), b"new complete payload");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "no temporary file remains");
    }
}
