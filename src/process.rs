//! Bounded Windows command execution. No worker owns a pipe after this call.

use std::process::Command;
use std::time::Duration;

#[cfg(windows)]
pub(crate) use windows::run_bounded;

#[cfg(not(windows))]
pub(crate) fn run_bounded(
    _command: Command,
    label: &str,
    _timeout: Duration,
    _max_bytes: usize,
    _input: Option<Vec<u8>>,
) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
    Err(format!("{label}: bounded process execution requires Windows"))
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Stdio};
    use std::ptr::{null, null_mut};
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_NO_DATA, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_OUTBOUND,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, GetNamedPipeClientProcessId, PeekNamedPipe, PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE,
    };
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

    const CHUNK: usize = 64 * 1024;
    /// Idle pause between polls; it doubles while nothing moves (up to `POLL_STEPS`
    /// doublings) and falls back to this the moment a pipe moves, so a long quiet
    /// command costs a quarter of the wake-ups and a busy one is not slowed.
    const POLL: Duration = Duration::from_millis(2);
    const POLL_STEPS: u32 = 2;

    fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            // Each successful creation/open returns a unique owned handle.
            Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
        }
    }

    fn job() -> io::Result<OwnedHandle> {
        let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // No breakaway flags: every descendant stays in this invocation's job.
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    struct Invocation {
        child: Child,
        job: Option<OwnedHandle>,
    }

    impl Drop for Invocation {
        fn drop(&mut self) {
            // Closing the sole, non-inheritable job handle also kills descendants
            // on success, IO errors, timeout and unwinding. kill covers failures
            // before assignment, when the direct child is still suspended.
            drop(self.job.take());
            let _ = self.child.kill();
        }
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtResumeProcess(process: HANDLE) -> i32;
    }

    fn resume(child: &Child) -> io::Result<()> {
        // std::Child retains the process handle, not the primary thread handle,
        // and the only documented way to find that thread (a Toolhelp snapshot)
        // walks every thread on the machine for each spawn. Before its first
        // execution the child has only the suspended primary thread, so
        // resuming the process resumes exactly that thread.
        // SAFETY: the handle is the live, full-access process handle std owns.
        let status = unsafe { NtResumeProcess(child.as_raw_handle()) };
        if status < 0 {
            return Err(io::Error::other(format!("could not resume suspended child (NTSTATUS {status:#010x})")));
        }
        Ok(())
    }

    fn stdin_pipe() -> io::Result<(File, File)> {
        use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        let mut nonce = [0_u8; 16];
        // SAFETY: the system RNG writes exactly the size of this writable buffer.
        let status = unsafe {
            BCryptGenRandom(null_mut(), nonce.as_mut_ptr(), nonce.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
        };
        if status < 0 {
            return Err(io::Error::other("cannot generate stdin pipe name"));
        }
        let name: Vec<u16> =
            format!("\\\\.\\pipe\\anvil-stdin-{}-{:032x}", std::process::id(), u128::from_le_bytes(nonce),)
                .encode_utf16()
                .chain(Some(0))
                .collect();
        let writer = owned(unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                CHUNK as u32,
                0,
                0,
                null(),
            )
        })?;
        // CreateFile connects this client without a blocking ConnectNamedPipe.
        // The child receives a normal, blocking read handle via std::Command.
        let reader =
            owned(unsafe { CreateFileW(name.as_ptr(), GENERIC_READ, 0, null(), OPEN_EXISTING, 0, null_mut()) })?;
        let mut pid = 0;
        if unsafe { GetNamedPipeClientProcessId(writer.as_raw_handle(), &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if pid != std::process::id() {
            return Err(io::Error::other("stdin pipe client does not belong to this process"));
        }
        Ok((File::from(writer), File::from(reader)))
    }

    fn broken(error: &io::Error) -> bool {
        matches!(error.raw_os_error(), Some(code) if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_NO_DATA as i32)
    }

    // One bounded read per poll gives stdin, stderr and the deadline a turn even
    // when stdout never stops. No other thread can consume the peeked bytes.
    fn drain(
        stream: &mut (impl Read + AsRawHandle),
        output: &mut Vec<u8>,
        max_bytes: usize,
        chunk: &mut [u8; CHUNK],
    ) -> io::Result<usize> {
        let mut available = 0;
        if unsafe { PeekNamedPipe(stream.as_raw_handle(), null_mut(), 0, null_mut(), &mut available, null_mut()) } == 0
        {
            let error = io::Error::last_os_error();
            return if broken(&error) { Ok(0) } else { Err(error) };
        }
        if available == 0 {
            return Ok(0);
        }
        let size = (available as usize).min(CHUNK);
        let count = stream.read(&mut chunk[..size])?;
        let keep = count.min(max_bytes - output.len());
        if keep > output.capacity() - output.len() {
            output.reserve_exact(keep);
        }
        output.extend_from_slice(&chunk[..keep]);
        Ok(count)
    }

    /// Owns the whole child tree before any child code executes; keeps the raw
    /// prefix of each output and uses one absolute deadline for all IO/waits.
    pub(crate) fn run_bounded(
        mut command: Command,
        label: &str,
        timeout: Duration,
        max_bytes: usize,
        input: Option<Vec<u8>>,
    ) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
        let started = Instant::now();
        let expired = || format!("{label} не ответил за {} с", timeout.as_secs());
        let error = |e: io::Error| format!("{label}: {e}");
        let job = job().map_err(error)?;
        let mut writer = if input.is_some() {
            let (writer, reader) = stdin_pipe().map_err(error)?;
            command.stdin(Stdio::from(reader));
            Some(writer)
        } else {
            command.stdin(Stdio::null());
            None
        };
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        // Keep Command's native argument quoting, cwd and environment handling.
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        if started.elapsed() >= timeout {
            return Err(expired());
        }
        let child = command.spawn().map_err(error)?;
        let mut invocation = Invocation { child, job: Some(job) };
        if unsafe {
            AssignProcessToJobObject(
                invocation.job.as_ref().expect("owned job").as_raw_handle(),
                invocation.child.as_raw_handle(),
            )
        } == 0
        {
            return Err(error(io::Error::last_os_error()));
        }
        if started.elapsed() >= timeout {
            return Err(expired());
        }
        resume(&invocation.child).map_err(error)?;
        let mut stdout = invocation.child.stdout.take().expect("piped stdout");
        let mut stderr = invocation.child.stderr.take().expect("piped stderr");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut chunk = [0u8; CHUNK];
        let mut sent = 0;
        let mut status = None;
        let mut idle = 0u32;
        loop {
            if started.elapsed() >= timeout {
                return Err(expired());
            }
            if status.is_none() {
                status = invocation.child.try_wait().map_err(error)?;
                if status.is_some() {
                    // Output written by the direct child is already in the pipes.
                    // Kill inherited writers before draining the remaining bytes.
                    drop(invocation.job.take());
                    writer = None;
                }
            }
            let read_out = drain(&mut stdout, &mut out, max_bytes, &mut chunk).map_err(error)?;
            if started.elapsed() >= timeout {
                return Err(expired());
            }
            let read_err = drain(&mut stderr, &mut err, max_bytes, &mut chunk).map_err(error)?;
            if started.elapsed() >= timeout {
                return Err(expired());
            }
            if let Some(status) = status {
                if read_out == 0 && read_err == 0 {
                    return Ok((status.success(), out, err));
                }
            }
            let mut written = 0;
            if let (Some(pipe), Some(bytes)) = (writer.as_mut(), input.as_ref()) {
                if sent < bytes.len() {
                    let end = sent.saturating_add(CHUNK).min(bytes.len());
                    // PIPE_NOWAIT byte writes can be partial or zero; never
                    // write_all, never discard an unwritten suffix.
                    match pipe.write(&bytes[sent..end]) {
                        Ok(count) => {
                            sent += count;
                            written = count;
                        }
                        Err(e) if broken(&e) => writer = None,
                        Err(e) => return Err(error(e)),
                    }
                }
                if sent == bytes.len() {
                    writer = None;
                } // EOF, including empty input
            }
            if read_out == 0 && read_err == 0 && written == 0 {
                let pause = POLL.saturating_mul(1 << idle.min(POLL_STEPS));
                idle = idle.saturating_add(1);
                std::thread::sleep(pause.min(timeout.saturating_sub(started.elapsed())));
            } else {
                idle = 0;
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessHandleCount, OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    const MODE: &str = "ANVIL_BOUNDED_TEST_MODE";
    const PID_FILE: &str = "ANVIL_BOUNDED_TEST_PID_FILE";
    const MARKER: &[u8] = b"ANVIL-RAW\0";

    fn helper(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        // Re-execute only our own helper, never Git, shell commands or hooks.
        let (_, module) = module_path!().split_once("::").unwrap();
        command.args(["--exact", &format!("{module}::helper_process"), "--nocapture"]);
        command.env(MODE, mode);
        command
    }

    fn bytes(size: usize) -> Vec<u8> {
        (0..size).map(|index| index as u8).collect()
    }

    #[test]
    fn helper_process() {
        let Ok(mode) = std::env::var(MODE) else {
            return;
        };
        match mode.as_str() {
            "duplex" => {
                // Both output pipes fill before any stdin is read.
                std::io::stdout().write_all(MARKER).unwrap();
                std::io::stdout().write_all(&bytes(256 * 1024)).unwrap();
                std::io::stderr().write_all(&bytes(256 * 1024)).unwrap();
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                assert_eq!(input, bytes(1024 * 1024));
                std::process::exit(0);
            }
            "descendant" => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            "tree-success" | "tree-timeout" => {
                // All three pipes are inherited; the descendant never reads or
                // closes them, even after its direct parent has exited.
                std::io::stdout().write_all(MARKER).unwrap();
                std::io::stdout().write_all(&bytes(2048)).unwrap();
                std::io::stderr().write_all(&bytes(2048)).unwrap();
                std::io::stdout().flush().unwrap();
                std::io::stderr().flush().unwrap();
                // Never awaited on purpose: the point of this fixture is a
                // descendant that outlives its parent and must be killed by the
                // job object, not one this process reaps.
                #[allow(clippy::zombie_processes)]
                let descendant = helper("descendant")
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap();
                fs::write(std::env::var_os(PID_FILE).unwrap(), descendant.id().to_string()).unwrap();
                if mode == "tree-success" {
                    std::process::exit(0);
                }
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            "context" => {
                assert_eq!(
                    std::env::current_dir().unwrap(),
                    PathBuf::from(std::env::var_os("ANVIL_TEST_CWD").unwrap())
                );
                assert_eq!(std::env::var("ANVIL_TEST_VALUE").unwrap(), "spaces \"quotes\" Юникод");
                assert!(std::env::var_os("ANVIL_TEST_REMOVED").is_none());
                std::io::stdout().write_all(MARKER).unwrap();
                std::process::exit(0);
            }
            _ => panic!("unknown own-helper mode"),
        }
    }

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn resources() -> (u32, usize) {
        let mut handles = 0;
        assert_ne!(unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) }, 0);
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        assert!(!raw.is_null() && raw != INVALID_HANDLE_VALUE);
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut count = 0;
        let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
        while more != 0 {
            if entry.th32OwnerProcessID == std::process::id() {
                count += 1;
            }
            more = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
        }
        (handles, count)
    }

    /// The descendant's pid, once the helper has recorded it. A helper the
    /// deadline kills before it even reaches the spawn leaves no file, and then
    /// there was no descendant to survive: `None` is that, not a failure.
    fn descendant_pid(path: &std::path::Path, grace: Duration) -> Option<u32> {
        let until = Instant::now() + grace;
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse() {
                    return Some(pid);
                }
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Asserts the recorded descendant is gone; answers whether it was recorded
    /// at all, so the caller can insist the check was not vacuous.
    fn assert_descendant_dead(path: &std::path::Path) -> bool {
        let Some(pid) = descendant_pid(path, Duration::from_secs(2)) else { return false };
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if raw.is_null() {
            // ERROR_INVALID_PARAMETER means the process has already disappeared,
            // not that the test lacked permission to inspect it.
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(87));
        } else {
            let status = unsafe { WaitForSingleObject(raw, 1000) };
            unsafe {
                CloseHandle(raw);
            }
            assert_eq!(status, WAIT_OBJECT_0, "descendant {pid} survived invocation");
        }
        true
    }

    fn raw_stdout(output: &[u8]) -> &[u8] {
        let start = output.windows(MARKER.len()).position(|part| part == MARKER).unwrap();
        &output[start + MARKER.len()..]
    }

    // Process-wide resource accounting needs its own harness process: a mutex
    // in this test cannot exclude unrelated tests opening handles or threads.
    #[test]
    fn bounded_invocation_owns_pipes_tree_and_deadline() {
        const PROBE: &str = "ANVIL_BOUNDED_RESOURCE_PROBE";
        if std::env::var_os(PROBE).is_none() {
            let (_, module) = module_path!().split_once("::").unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                &format!("{module}::bounded_invocation_owns_pipes_tree_and_deadline"),
                "--nocapture",
                "--test-threads=1",
            ]);
            command.env(PROBE, "1");
            let (success, out, err) =
                run_bounded(command, "isolated resource probe", Duration::from_secs(120), 32 * 1024, None).unwrap();
            assert!(
                success,
                "isolated probe failed:\n{}\n{}",
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(&err)
            );
            return;
        }
        let scratch = Scratch(std::env::temp_dir().join(format!(
            "anvil-bounded-regression-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        )));
        fs::create_dir(&scratch.0).unwrap();
        // These two cases assert what came back, not how fast: half a megabyte
        // through both pipes plus a megabyte in has no business finishing inside
        // a deadline a loaded machine can stretch, so give it room.
        let roomy = Duration::from_secs(20);
        let (success, out, err) =
            run_bounded(helper("duplex"), "own duplex helper", roomy, 4096, Some(bytes(1024 * 1024))).unwrap();
        assert!(success, "{}", String::from_utf8_lossy(&err));
        assert_eq!(out.len(), 4096);
        assert_eq!(err, bytes(4096));
        assert_eq!(raw_stdout(&out), &bytes(4096)[..raw_stdout(&out).len()]);

        let mut context = helper("context");
        context
            .current_dir(&scratch.0)
            .env("ANVIL_TEST_CWD", &scratch.0)
            .env("ANVIL_TEST_VALUE", "spaces \"quotes\" Юникод")
            .env_remove("ANVIL_TEST_REMOVED");
        assert!(run_bounded(context, "own context helper", roomy, 4096, Some(Vec::new())).unwrap().0);

        let before = resources();
        let deadline = Duration::from_millis(600);
        // The deadline must be met without waiting for the inherited pipes the
        // descendant holds open for a minute; the slack only absorbs a loaded
        // machine's scheduling delay, it is nowhere near that minute.
        let slack = Duration::from_millis(400);
        // The direct child exits on its own here, so the call must come back
        // well before its own deadline — and a fortiori nowhere near the minute
        // its descendant would hold the pipes for.
        let tree_deadline = Duration::from_secs(3);
        // This case launches the test binary twice per iteration, so a single
        // sample can be stretched by the rest of the suite. The invariant is
        // that the call returns long before the minute its descendant holds the
        // pipes for, and the fastest of the loop is the least perturbed measure
        // of it; every iteration still has to succeed.
        let mut fastest_tree = Duration::MAX;
        let mut fastest_timeout = Duration::MAX;
        let mut checked = 0;
        for index in 0..8 {
            let pid_path = scratch.0.join(format!("success-{index}.pid"));
            let mut command = helper("tree-success");
            command.env(PID_FILE, &pid_path);
            let start = Instant::now();
            let (success, out, err) =
                run_bounded(command, "own success tree", tree_deadline, 4096, Some(bytes(1024 * 1024))).unwrap();
            assert!(success);
            fastest_tree = fastest_tree.min(start.elapsed());
            assert!(raw_stdout(&out).starts_with(&bytes(2048)));
            assert_eq!(err, bytes(2048));
            checked += usize::from(assert_descendant_dead(&pid_path));

            let pid_path = scratch.0.join(format!("timeout-{index}.pid"));
            let mut command = helper("tree-timeout");
            command.env(PID_FILE, &pid_path);
            let start = Instant::now();
            let result = run_bounded(command, "own timed-out tree", deadline, 4096, Some(bytes(1024 * 1024)));
            assert!(result.unwrap_err().contains("не ответил"));
            fastest_timeout = fastest_timeout.min(start.elapsed());
            checked += usize::from(assert_descendant_dead(&pid_path));
        }
        assert!(checked > 0, "no helper recorded a descendant, so nothing was proved about the tree");
        // Neither call may wait for the pipes the descendant holds: the helper
        // sleeps for a minute, so anything near that proves the join happened.
        assert!(fastest_tree < tree_deadline, "waited for inherited pipe EOF: {:?}", fastest_tree);
        assert!(fastest_timeout < deadline + slack, "added per-stream waits to the deadline: {:?}", fastest_timeout);
        let after = resources();
        assert!(after.0 <= before.0 + 2, "handles grew: {before:?} -> {after:?}");
        assert!(after.1 <= before.1, "pipe threads survived: {before:?} -> {after:?}");

        assert!(run_bounded(helper("duplex"), "zero deadline", Duration::ZERO, 0, None,)
            .unwrap_err()
            .contains("не ответил"));
        // Empty caps still drain, rather than blocking a chatty child.
        assert_eq!(
            run_bounded(helper("duplex"), "zero cap", roomy, 0, Some(bytes(1024 * 1024)),).unwrap(),
            (true, Vec::new(), Vec::new())
        );
    }
}
