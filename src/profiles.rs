//! Shell profiles: detection of installed shells, user profiles from the
//! config, and the environment each pane starts with.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileKind {
    GitBash,
    PowerShell,
    Cmd,
    Wsl,
    Custom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub kind: ProfileKind,
}

/// The Git Bash `PROMPT_COMMAND` that reports the working directory to ANVIL
/// (the same command Helm used), placed before any existing command.
pub fn prompt_command(existing: Option<&str>) -> String {
    const REPORT: &str = r#"printf '\033]1337;CurrentDir=%s\007' "$(cygpath -w "$PWD")""#;
    match existing.map(str::trim).filter(|s| !s.is_empty()) {
        Some(prev) => format!("{REPORT}; {prev}"),
        None => REPORT.to_owned(),
    }
}

/// Extra environment for a pane (the rest is inherited from ANVIL).
pub fn pane_env(
    profile: &Profile,
    pane_id: u64,
    status_dir: Option<&Path>,
    version: &str,
    inherited_prompt_command: Option<&str>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "ANVIL".into());
    env.insert("TERM_PROGRAM_VERSION".into(), version.into());
    env.insert("WT_SESSION".into(), "0".into());
    env.insert("ANVIL_PANE_ID".into(), pane_id.to_string());
    if let Some(dir) = status_dir {
        env.insert("ANVIL_STATUS_DIR".into(), dir.to_string_lossy().into_owned());
    }
    if profile.kind == ProfileKind::GitBash {
        env.insert("PROMPT_COMMAND".into(), prompt_command(inherited_prompt_command));
    }
    for (k, v) in &profile.env {
        env.insert(k.clone(), v.clone());
    }
    env
}

/// Names printed by `wsl.exe -l -q` (UTF-16LE, possibly with a BOM).
pub fn parse_wsl_list(bytes: &[u8]) -> Vec<String> {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
        .trim_start_matches('\u{feff}')
        .lines()
        .map(|l| l.trim().trim_matches('\0').to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Subsequence match, case-insensitive. Higher is better; None = no match.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let c: Vec<char> = candidate.to_lowercase().chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (i, ch) in c.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            score += 10;
            if prev == Some(i.wrapping_sub(1)) {
                score += 20;
            }
            if i == 0 || !c[i - 1].is_alphanumeric() {
                score += 20;
            }
            if let Some(p) = prev {
                score -= 5 * (i - p - 1) as i32;
            }
            prev = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score)
}

#[cfg(windows)]
fn registry_string(root: windows_sys::Win32::System::Registry::HKEY, subkey: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: buffer and size describe a valid writable region.
    let rc = unsafe {
        RegGetValueW(
            root,
            wide(subkey).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Shells installed on this machine, in menu order.
#[cfg(windows)]
pub fn detect_builtin() -> Vec<Profile> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let env_path = |var: &str| std::env::var_os(var).map(PathBuf::from);
    let mut out = Vec::new();

    let git_root = registry_string(HKEY_LOCAL_MACHINE, "Software\\GitForWindows", "InstallPath")
        .or_else(|| registry_string(HKEY_CURRENT_USER, "Software\\GitForWindows", "InstallPath"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Program Files\\Git"));
    let bash = git_root.join("bin").join("bash.exe");
    if bash.is_file() {
        out.push(Profile {
            id: "git-bash".into(),
            name: "Git Bash".into(),
            command: bash.to_string_lossy().into_owned(),
            args: vec!["--login".into(), "-i".into()],
            cwd: None,
            env: BTreeMap::new(),
            kind: ProfileKind::GitBash,
        });
    }

    let mut pwsh_candidates = Vec::new();
    if let Some(local) = env_path("LOCALAPPDATA") {
        pwsh_candidates.push(local.join("Microsoft\\WindowsApps\\pwsh.exe"));
    }
    if let Some(pf) = env_path("ProgramFiles") {
        pwsh_candidates.push(pf.join("PowerShell\\7\\pwsh.exe"));
    }
    if let Some(root) = env_path("SystemRoot") {
        pwsh_candidates.push(root.join("System32\\WindowsPowerShell\\v1.0\\powershell.exe"));
    }
    if let Some(ps) = pwsh_candidates.into_iter().find(|p| p.is_file()) {
        out.push(Profile {
            id: "powershell".into(),
            name: "PowerShell".into(),
            command: ps.to_string_lossy().into_owned(),
            args: vec!["-nologo".into()],
            cwd: None,
            env: BTreeMap::new(),
            kind: ProfileKind::PowerShell,
        });
    }

    let cmd = env_path("SystemRoot").map(|r| r.join("System32\\cmd.exe")).unwrap_or_else(|| PathBuf::from("cmd.exe"));
    out.push(Profile {
        id: "cmd".into(),
        name: "cmd".into(),
        command: cmd.to_string_lossy().into_owned(),
        args: vec![],
        cwd: None,
        env: BTreeMap::new(),
        kind: ProfileKind::Cmd,
    });

    let wsl = std::process::Command::new("wsl.exe")
        .args(["-l", "-q"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(output) = wsl {
        if output.status.success() {
            for distro in parse_wsl_list(&output.stdout) {
                out.push(Profile {
                    id: format!("wsl-{distro}"),
                    name: format!("WSL: {distro}"),
                    command: "wsl.exe".into(),
                    args: vec!["-d".into(), distro.clone()],
                    cwd: None,
                    env: BTreeMap::new(),
                    kind: ProfileKind::Wsl,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(kind: ProfileKind) -> Profile {
        Profile {
            id: "x".into(),
            name: "x".into(),
            command: "x.exe".into(),
            args: vec![],
            cwd: None,
            env: BTreeMap::from([("FOO".to_owned(), "bar".to_owned()), ("TERM".to_owned(), "dumb".to_owned())]),
            kind,
        }
    }

    #[test]
    fn prompt_command_prepends_report() {
        let bare = prompt_command(None);
        assert_eq!(bare, r#"printf '\033]1337;CurrentDir=%s\007' "$(cygpath -w "$PWD")""#);
        assert_eq!(prompt_command(Some("history -a")), format!("{bare}; history -a"));
        assert_eq!(prompt_command(Some("  ")), bare);
    }

    #[test]
    fn pane_env_contents() {
        let env = pane_env(&profile(ProfileKind::GitBash), 7, Some(Path::new("C:\\run\\1")), "0.1.0", None);
        assert_eq!(env["TERM_PROGRAM"], "ANVIL");
        assert_eq!(env["ANVIL_PANE_ID"], "7");
        assert_eq!(env["ANVIL_STATUS_DIR"], "C:\\run\\1");
        assert_eq!(env["WT_SESSION"], "0");
        assert_eq!(env["FOO"], "bar");
        assert_eq!(env["TERM"], "dumb", "profile env wins");
        assert!(env.contains_key("PROMPT_COMMAND"));
        let cmd = pane_env(&profile(ProfileKind::Cmd), 1, None, "0.1.0", None);
        assert!(!cmd.contains_key("PROMPT_COMMAND"));
        assert!(!cmd.contains_key("ANVIL_STATUS_DIR"));
    }

    #[test]
    fn wsl_list_utf16() {
        let text = "\u{feff}Ubuntu\r\n\r\nDebian\r\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(parse_wsl_list(&bytes), vec!["Ubuntu".to_owned(), "Debian".to_owned()]);
        assert!(parse_wsl_list(&[]).is_empty());
    }

    #[test]
    fn fuzzy_ranking() {
        assert_eq!(fuzzy_score("", "Git Bash"), Some(0));
        assert!(fuzzy_score("gb", "Git Bash").is_some());
        assert!(fuzzy_score("xyz", "Git Bash").is_none());
        let bash = fuzzy_score("bash", "Git Bash").unwrap();
        let scattered = fuzzy_score("bash", "b-a-s-h").unwrap();
        assert!(bash > scattered);
        assert!(fuzzy_score("ПОВЕР", "поверка").is_some(), "Unicode case folding");
    }
}
