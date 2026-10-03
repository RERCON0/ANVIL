//! Process tree queries (is `claude.exe` running under this pane's shell?).

use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
}

/// True when a descendant of `root` (not `root` itself) has executable `name`
/// (case-insensitive). Tolerates parent-pid cycles from reused pids.
pub fn has_descendant_named(procs: &[ProcInfo], root: u32, name: &str) -> bool {
    let mut children: HashMap<u32, Vec<&ProcInfo>> = HashMap::new();
    for p in procs {
        if p.pid != p.ppid {
            children.entry(p.ppid).or_default().push(p);
        }
    }
    let mut seen = HashSet::from([root]);
    let mut queue = VecDeque::from([root]);
    while let Some(pid) = queue.pop_front() {
        for child in children.get(&pid).into_iter().flatten() {
            if !seen.insert(child.pid) {
                continue;
            }
            if child.name.eq_ignore_ascii_case(name) {
                return true;
            }
            queue.push_back(child.pid);
        }
    }
    false
}

/// All processes, via a Toolhelp snapshot. Empty on failure.
#[cfg(windows)]
pub fn snapshot() -> Vec<ProcInfo> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: plain Win32 calls on a zeroed, correctly sized PROCESSENTRY32W.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            out.push(ProcInfo {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                name: String::from_utf16_lossy(&entry.szExeFile[..len]),
            });
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str) -> ProcInfo {
        ProcInfo { pid, ppid, name: name.to_owned() }
    }

    #[test]
    fn finds_nested_descendant() {
        let procs = vec![p(10, 1, "bash.exe"), p(11, 10, "node.exe"), p(12, 11, "Claude.EXE"), p(20, 1, "claude.exe")];
        assert!(has_descendant_named(&procs, 10, "claude.exe"));
        assert!(!has_descendant_named(&procs, 11, "node.exe"), "root itself does not count");
        assert!(!has_descendant_named(&procs, 30, "claude.exe"));
    }

    #[test]
    fn survives_cycles() {
        let procs = vec![p(1, 2, "a.exe"), p(2, 1, "b.exe"), p(3, 3, "self.exe")];
        assert!(!has_descendant_named(&procs, 1, "claude.exe"));
        assert!(!has_descendant_named(&procs, 3, "claude.exe"));
    }
}
