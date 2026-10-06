//! Process tree queries (is `claude.exe` running under this pane's shell?).

use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// Queried only for Node processes; unavailable/protected processes remain
    /// unidentified instead of being mistaken for Claude Code.
    pub command_line: Option<String>,
}

/// True when a descendant of `root` (not `root` itself) has executable `name`
/// (case-insensitive). Tolerates parent-pid cycles from reused pids.
pub fn has_descendant_named(procs: &[ProcInfo], root: u32, name: &str) -> bool {
    descendant_names(procs, root).contains(&name.to_ascii_lowercase())
}

/// Lower-case executable names of every descendant of `root` (not `root`
/// itself): one walk of the process list answers any number of names.
pub fn descendant_names(procs: &[ProcInfo], root: u32) -> HashSet<String> {
    let mut names = HashSet::new();
    walk_descendants(procs, root, |process| {
        names.insert(process.name.to_ascii_lowercase());
    });
    names
}

fn walk_descendants(procs: &[ProcInfo], root: u32, mut visit: impl FnMut(&ProcInfo)) {
    let mut children: HashMap<u32, Vec<&ProcInfo>> = HashMap::new();
    for process in procs {
        if process.pid != process.ppid {
            children.entry(process.ppid).or_default().push(process);
        }
    }
    let mut seen = HashSet::from([root]);
    let mut queue = VecDeque::from([root]);
    while let Some(pid) = queue.pop_front() {
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(child.pid) {
                visit(child);
                queue.push_back(child.pid);
            }
        }
    }
}

/// Canonical CLI names detected under a shell. A Node executable counts only
/// when its script argument is a known CLI entrypoint, never from its name or
/// from a package name mentioned inside `node -e`.
pub fn detected_cli_names(procs: &[ProcInfo], root: u32) -> HashSet<String> {
    let mut names = HashSet::new();
    walk_descendants(procs, root, |process| {
        if let Some(cli) = process_cli(process) {
            names.insert(cli.to_owned());
        }
    });
    names
}

pub fn runs_claude(procs: &[ProcInfo], shell: u32) -> bool {
    let mut found = false;
    walk_descendants(procs, shell, |process| {
        found |= process_cli(process) == Some("claude");
    });
    found
}

fn process_cli(process: &ProcInfo) -> Option<&'static str> {
    let native = [("claude", "claude.exe"), ("opencode", "opencode.exe"), ("codex", "codex.exe"), ("gemini", "gemini.exe"), ("aider", "aider.exe")]
        .into_iter()
        .find_map(|(cli, executable)| (process.name.eq_ignore_ascii_case(cli) || process.name.eq_ignore_ascii_case(executable)).then_some(cli));
    native.or_else(|| {
        process.name.eq_ignore_ascii_case("node.exe")
            .then(|| process.command_line.as_deref().and_then(node_entrypoint_cli))
            .flatten()
    })
}

fn node_entrypoint_cli(command_line: &str) -> Option<&'static str> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
    let wide: Vec<u16> = command_line.encode_utf16().chain(std::iter::once(0)).collect();
    let mut count = 0;
    // SAFETY: a NUL-terminated command line; the Shell owns this argument array
    // until LocalFree. Only argv[1], the npm shim's actual script, is inspected.
    unsafe {
        let args = CommandLineToArgvW(wide.as_ptr(), &mut count);
        if args.is_null() {
            return None;
        }
        let result = if count >= 2 {
            let script = *args.add(1);
            let mut length = 0;
            while *script.add(length) != 0 {
                length += 1;
            }
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(script, length)).replace('\\', "/").to_ascii_lowercase();
            [
                ("/node_modules/@anthropic-ai/claude-code/cli.js", "claude"),
                ("/node_modules/@openai/codex/bin/codex.js", "codex"),
                ("/node_modules/@google/gemini-cli/dist/index.js", "gemini"),
                ("/node_modules/opencode-ai/bin/opencode", "opencode"),
            ]
            .into_iter()
            .find_map(|(suffix, cli)| path.ends_with(suffix).then_some(cli))
        } else {
            None
        };
        LocalFree(args.cast());
        result
    }
}

/// ProcessCommandLineInformation returns an owned snapshot without following
/// a remote process's PEB pointers. Unknown versions/access failures fail closed.
#[cfg(windows)]
fn process_command_line(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(process: HANDLE, class: u32, information: *mut std::ffi::c_void, length: u32, returned: *mut u32) -> i32;
    }
    // SAFETY: the queried class writes only to our bounded aligned allocation.
    // Validate the returned string lies wholly within it before reading.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut bytes = 0;
        NtQueryInformationProcess(process, 60, std::ptr::null_mut(), 0, &mut bytes);
        if !(std::mem::size_of::<UnicodeString>() as u32..=128 * 1024).contains(&bytes) {
            CloseHandle(process);
            return None;
        }
        let words = (bytes as usize).div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; words];
        let capacity = storage.len() * std::mem::size_of::<usize>();
        let status = NtQueryInformationProcess(process, 60, storage.as_mut_ptr().cast(), capacity as u32, &mut bytes);
        CloseHandle(process);
        if status < 0 {
            return None;
        }
        let string = &*storage.as_ptr().cast::<UnicodeString>();
        let start = storage.as_ptr() as usize;
        let pointer = string.buffer as usize;
        let end = pointer.checked_add(string.length as usize)?;
        if !string.length.is_multiple_of(2) || !pointer.is_multiple_of(2) || pointer < start || end > start + capacity {
            return None;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(string.buffer, string.length as usize / 2)))
    }
}

/// All processes, via a Toolhelp snapshot. `None` when the snapshot itself
/// failed — an empty list and "could not enumerate" are different answers, and
/// callers that decide what is dead must not read the second as the first.
#[cfg(windows)]
pub fn snapshot() -> Option<Vec<ProcInfo>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: plain Win32 calls on a zeroed, correctly sized PROCESSENTRY32W.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            let command_line = name.eq_ignore_ascii_case("node.exe").then(|| process_command_line(entry.th32ProcessID)).flatten();
            out.push(ProcInfo {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                name,
                command_line,
            });
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str) -> ProcInfo {
        ProcInfo { pid, ppid, name: name.to_owned(), command_line: None }
    }

    #[test]
    fn finds_nested_descendant() {
        let procs = vec![p(10, 1, "bash.exe"), p(11, 10, "node.exe"), p(12, 11, "Claude.EXE"), p(20, 1, "claude.exe")];
        assert!(has_descendant_named(&procs, 10, "claude.exe"));
        assert!(!has_descendant_named(&procs, 11, "node.exe"), "root itself does not count");
        assert!(!has_descendant_named(&procs, 30, "claude.exe"));
    }

    #[test]
    fn claude_requires_native_executable_or_known_node_entrypoint() {
        let shell = p(10, 1, "pwsh.exe");
        assert!(runs_claude(&[shell.clone(), p(11, 10, "claude.exe")], 10));
        let mut npm = p(12, 11, "node.exe");
        npm.command_line = Some(r#""C:\Program Files\nodejs\node.exe" "C:\Users\Jane Doe\npm\node_modules\@anthropic-ai\claude-code\cli.js" --debug"#.to_owned());
        assert!(runs_claude(&[shell.clone(), p(11, 10, "cmd.exe"), npm.clone()], 10));
        npm.command_line = Some(r#"node.exe -e "console.log('/node_modules/@anthropic-ai/claude-code/cli.js')" "#.to_owned());
        assert!(!runs_claude(&[shell.clone(), p(11, 10, "cmd.exe"), npm], 10));
        assert!(!runs_claude(&[shell.clone(), p(11, 10, "node.exe")], 10));
        assert!(!runs_claude(&[shell, p(11, 10, "git.exe")], 10));
    }

    #[test]
    fn names_of_all_descendants() {
        let procs = vec![p(10, 1, "bash.exe"), p(11, 10, "Node.exe"), p(12, 11, "claude.exe"), p(20, 1, "other.exe")];
        let names = descendant_names(&procs, 10);
        assert_eq!(names, HashSet::from(["node.exe".to_owned(), "claude.exe".to_owned()]));
    }

    #[test]
    fn survives_cycles() {
        let procs = vec![p(1, 2, "a.exe"), p(2, 1, "b.exe"), p(3, 3, "self.exe")];
        assert!(!has_descendant_named(&procs, 1, "claude.exe"));
        assert!(!has_descendant_named(&procs, 3, "claude.exe"));
    }
}
