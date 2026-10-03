//! Wires `anvil-claude-status.exe` into Claude Code's settings.json. Only the
//! `statusLine` key is ever touched; every other key keeps its value and order.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// statusLine already runs our command.
    AlreadyInstalled,
    /// No statusLine (or no file): install without asking.
    Install,
    /// Our own status command from another build (debug/release path):
    /// point it at this binary silently, keeping the saved original.
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

/// Copies settings.json to settings.json.anvil-backup once. The backup is
/// created exclusively, so two ANVIL windows cannot overwrite the first copy
/// (and with it the recovery of the user's original command).
pub fn backup_once(settings_path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut backup = settings_path.as_os_str().to_os_string();
    backup.push(".anvil-backup");
    let backup = PathBuf::from(backup);
    if !settings_path.exists() {
        return Ok(());
    }
    let bytes = match std::fs::read(settings_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&backup) {
        Ok(mut file) => file.write_all(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
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
        assert_eq!(plan(Some(stale), OURS, None), Plan::Update, "another ANVIL build updates silently");
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
        assert_eq!(fresh, format!("{{\n  \"statusLine\": {{\n    \"type\": \"command\",\n    \"command\": {}\n  }}\n}}\n", serde_json::to_string(OURS).unwrap()));
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
    fn backup_is_made_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "first").unwrap();
        backup_once(&path).unwrap();
        std::fs::write(&path, "second").unwrap();
        backup_once(&path).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("settings.json.anvil-backup")).unwrap(), "first");
    }
}
