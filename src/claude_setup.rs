//! Wires `anvil-claude-status.exe` into Claude Code's settings.json. Only the
//! `statusLine` key is ever touched; every other key keeps its value and order.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// statusLine already runs our command.
    AlreadyInstalled,
    /// No statusLine (or no file): installation requires confirmation.
    Install,
    /// Our status command from another build: updating requires confirmation.
    Update,
    /// Another command is configured: ask once before replacing it.
    AskReplace { current: String },
    /// The user already chose to keep this exact command.
    Declined,
    /// settings.json is not a JSON object: leave it alone.
    Broken(String),
}

/// True when the command actually runs an ANVIL status binary: the first token
/// (unquoted or quoted) must be a path whose file name is the helper. A
/// substring mention such as `echo anvil-claude-status` is foreign, so it takes
/// the confirm/decline path instead of being silently replaced.
pub fn is_anvil_status_command(command: &str) -> bool {
    let trimmed = command.trim();
    let first_token = match trimmed.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => trimmed.split_whitespace().next().unwrap_or(""),
    };
    let file_name = first_token.rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
    file_name == "anvil-claude-status.exe" || file_name == "anvil-claude-status"
}

/// `"C:/.../anvil-claude-status.exe"` with forward slashes and quotes, so it
/// survives both JSON and the shell Claude Code runs it through.
pub fn status_command(exe_dir: &Path) -> String {
    let exe = exe_dir.join("anvil-claude-status.exe");
    format!("\"{}\"", exe.to_string_lossy().replace('\\', "/"))
}

/// `%CLAUDE_CONFIG_DIR%\settings.json`, else `%USERPROFILE%\.claude\settings.json`.
pub fn settings_path(claude_config_dir: Option<&Path>, user_profile: Option<&Path>) -> Option<PathBuf> {
    match claude_config_dir {
        Some(dir) => Some(dir.join("settings.json")),
        None => user_profile.map(|home| home.join(".claude").join("settings.json")),
    }
}

fn parse_object(text: &str) -> Result<Map<String, Value>, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("settings.json is not a JSON object".into()),
        Err(e) => Err(format!("settings.json is not valid JSON: {e}")),
    }
}

fn command_of(status_line: &Value) -> String {
    match status_line.get("command") {
        Some(Value::String(c)) => c.trim().to_owned(),
        _ => status_line.to_string(),
    }
}

pub fn plan(settings: Option<&str>, ours: &str, declined: Option<&str>) -> Plan {
    let Some(text) = settings else { return Plan::Install };
    let map = match parse_object(text) {
        Ok(m) => m,
        Err(e) => return Plan::Broken(e),
    };
    let Some(current) = map.get("statusLine") else { return Plan::Install };
    let current = command_of(current);
    if current == ours.trim() {
        Plan::AlreadyInstalled
    } else if is_anvil_status_command(&current) {
        Plan::Update
    } else if Some(current.as_str()) == declined.map(str::trim) {
        Plan::Declined
    } else {
        Plan::AskReplace { current }
    }
}

fn render(map: Map<String, Value>) -> String {
    let mut text = serde_json::to_string_pretty(&Value::Object(map)).expect("serializes");
    text.push('\n');
    text
}

/// New settings text with our statusLine, and the statusLine it replaced.
pub fn install(settings: Option<&str>, ours: &str) -> Result<(String, Option<Value>), String> {
    let mut map = match settings {
        Some(text) => parse_object(text)?,
        None => Map::new(),
    };
    let ours_value = serde_json::json!({ "type": "command", "command": ours });
    let previous = map.insert("statusLine".into(), ours_value);
    Ok((render(map), previous))
}

/// Puts back what ANVIL replaced. Returns None (no change) when statusLine is
/// not ours any more: the user changed it, so it is theirs.
pub fn uninstall(settings: &str, ours: &str, previous: Option<&Value>) -> Result<Option<String>, String> {
    let mut map = parse_object(settings)?;
    match map.get("statusLine") {
        Some(v) if command_of(v) == ours.trim() => {}
        _ => return Ok(None),
    }
    match previous {
        Some(v) => {
            map.insert("statusLine".into(), v.clone());
        }
        None => {
            map.shift_remove("statusLine");
        }
    }
    Ok(Some(render(map)))
}

/// Applies consent to the document shown to the user, preserving atomic writes.
/// On Windows the snapshot handle denies in-place writers but allows rename,
/// which is necessary for atomic replacement. Recheck the path before staging;
/// Windows does not offer a filesystem compare-and-swap, so another program's
/// atomic rename after that check remains an unavoidable race.
pub fn write_confirmed(path: &Path, expected: Option<&str>, replacement: &str) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let Some(expected) = expected else {
        return create_confirmed(path, replacement.as_bytes());
    };
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
    }
    let mut file = options.open(path)?;
    let mut current = String::new();
    file.read_to_string(&mut current)?;
    if current != expected {
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "Claude settings changed after confirmation"));
    }
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".anvil-backup");
    match std::fs::OpenOptions::new().write(true).create_new(true).open(PathBuf::from(backup)) {
        Ok(mut backup) => {
            backup.write_all(current.as_bytes())?;
            backup.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    // Detect an editor replacing the pathname while we held the old snapshot.
    if std::fs::read_to_string(path)? != expected {
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "Claude settings changed after confirmation"));
    }
    crate::fsutil::atomic_write(path, replacement.as_bytes())
}

/// Missing-file consent publishes a complete synced file without overwriting
/// anything created while the dialog was open. No empty target is reserved.
fn create_confirmed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path.file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "settings path has no filename"))?
        .to_os_string();
    name.push(format!(".anvil-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let staged = path.with_file_name(name);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&staged)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
            let source: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
            let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // No REPLACE_EXISTING: a concurrently created target wins.
            // SAFETY: both buffers are valid NUL-terminated paths.
            if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(not(windows))]
        std::fs::hard_link(&staged, path)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&staged);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "\"C:/Tools/ANVIL/anvil-claude-status.exe\"";
    const OWNER: &str = r#"{
  "hooks": {"Stop": []},
  "statusLine": {"type": "command", "command": "node \"C:/Tools/cc/statusline.mjs\""},
  "model": "opus"
}"#;

    #[test]
    fn command_and_paths() {
        assert_eq!(status_command(Path::new("C:\\Tools\\ANVIL")), OURS);
        assert_eq!(
            settings_path(None, Some(Path::new("C:\\Users\\me"))),
            Some(PathBuf::from("C:\\Users\\me\\.claude\\settings.json"))
        );
        assert_eq!(
            settings_path(Some(Path::new("D:\\cc")), Some(Path::new("C:\\Users\\me"))),
            Some(PathBuf::from("D:\\cc\\settings.json"))
        );
    }

    #[test]
    fn planning() {
        assert_eq!(plan(None, OURS, None), Plan::Install);
        assert_eq!(plan(Some(r#"{"model":"opus"}"#), OURS, None), Plan::Install);
        assert_eq!(
            plan(Some(OWNER), OURS, None),
            Plan::AskReplace { current: "node \"C:/Tools/cc/statusline.mjs\"".into() }
        );
        assert_eq!(plan(Some(OWNER), OURS, Some("node \"C:/Tools/cc/statusline.mjs\"")), Plan::Declined);
        let stale = r#"{"statusLine":{"type":"command","command":"\"C:/build/anvil-claude-status.exe\""}}"#;
        assert_eq!(plan(Some(stale), OURS, None), Plan::Update, "a moved helper requires explicit update consent");
        let (installed, _) = install(Some(OWNER), OURS).unwrap();
        assert_eq!(plan(Some(&installed), OURS, None), Plan::AlreadyInstalled);
        assert!(matches!(plan(Some("{ broken"), OURS, None), Plan::Broken(_)));
        assert!(matches!(plan(Some("[1,2]"), OURS, None), Plan::Broken(_)));
    }

    #[test]
    fn install_keeps_other_keys_and_order() {
        let (text, previous) = install(Some(OWNER), OURS).unwrap();
        let keys: Vec<String> = parse_object(&text).unwrap().keys().cloned().collect();
        assert_eq!(keys, vec!["hooks", "statusLine", "model"]);
        assert_eq!(parse_object(&text).unwrap()["statusLine"]["command"], OURS);
        assert_eq!(previous.unwrap()["command"], "node \"C:/Tools/cc/statusline.mjs\"");
        let (fresh, none) = install(None, OURS).unwrap();
        let fresh = parse_object(&fresh).unwrap();
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh["statusLine"], serde_json::json!({ "type": "command", "command": OURS }));
        assert!(none.is_none());
    }

    #[test]
    fn uninstall_restores_or_removes_and_respects_foreign_values() {
        let (installed, previous) = install(Some(OWNER), OURS).unwrap();
        let restored = uninstall(&installed, OURS, previous.as_ref()).unwrap().unwrap();
        assert_eq!(parse_object(&restored).unwrap(), parse_object(OWNER).unwrap());

        let (installed, previous) = install(Some(r#"{"model":"opus"}"#), OURS).unwrap();
        let removed = uninstall(&installed, OURS, previous.as_ref()).unwrap().unwrap();
        assert_eq!(parse_object(&removed).unwrap(), parse_object(r#"{"model":"opus"}"#).unwrap());

        assert_eq!(uninstall(OWNER, OURS, None).unwrap(), None, "not ours: untouched");
        assert!(uninstall("{ broken", OURS, None).is_err());
    }

    #[test]
    fn consent_rejects_concurrent_changes_and_keeps_first_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, OWNER).unwrap();
        let (installed, _) = install(Some(OWNER), OURS).unwrap();
        write_confirmed(&path, Some(OWNER), &installed).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), installed);
        let changed = r#"{"model":"user-change"}"#;
        std::fs::write(&path, changed).unwrap();
        assert!(write_confirmed(&path, Some(&installed), OWNER).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), changed);
        write_confirmed(&path, Some(changed), OWNER).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("settings.json.anvil-backup")).unwrap(), OWNER);
        assert!(write_confirmed(&path, None, "{}").is_err(), "missing-file consent cannot overwrite a newly created file");
        let missing = dir.path().join("new/settings.json");
        write_confirmed(&missing, None, &installed).unwrap();
        assert_eq!(std::fs::read_to_string(missing).unwrap(), installed);
    }
}
