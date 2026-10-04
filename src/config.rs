//! %APPDATA%\anvil\config.json. Only values that differ from the defaults are
//! written, so the file stays short and future default changes still apply.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::fsutil::atomic_write;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub version: u32,
    pub font: FontConfig,
    pub color_scheme: String,
    pub custom_color_schemes: Vec<SchemeConfig>,
    pub default_profile: String,
    pub profiles: Vec<ProfileConfig>,
    pub terminal: TerminalConfig,
    /// Per-action overrides of the default hotkey table.
    pub hotkeys: BTreeMap<String, Vec<String>>,
    pub claude_status: ClaudeStatusConfig,
    pub restore_session: bool,
    pub workspace: WorkspaceConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceConfig {
    /// CLI that writes the commit message (claude, opencode, codex, gemini,
    /// aider or a full command line). None: detect the CLI running in the pane.
    pub ai_commit_command: Option<String>,
    /// OpenCode `provider/model` for commit messages. None: the CLI default.
    pub ai_commit_model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemeConfig {
    pub name: String,
    pub foreground: String,
    pub background: String,
    pub cursor: String,
    pub colors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileConfig {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShapeConfig {
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CursorConfig {
    pub shape: CursorShapeConfig,
    pub blink: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RightClick {
    Clipboard,
    Paste,
    Menu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bell {
    Off,
    Visual,
}

/// Largest scrollback the settings page offers (lines per pane).
pub const MAX_SCROLLBACK: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TerminalConfig {
    pub scrollback: usize,
    pub cursor: CursorConfig,
    pub right_click: RightClick,
    pub paste_on_middle_click: bool,
    pub copy_on_select: bool,
    /// Allow applications to write the OS clipboard through OSC 52.
    pub allow_osc52: bool,
    pub word_separators: String,
    pub bell: Bell,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClaudeStatusConfig {
    pub enabled: bool,
    /// A foreign statusLine command the user chose to keep.
    pub declined_command: Option<String>,
    /// The statusLine value ANVIL replaced (None: there was none).
    pub previous_status_line: Option<Value>,
    /// Exact helper command installed with consent, including its original path.
    pub installed_command: Option<String>,
    pub installed_settings_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 1,
            font: FontConfig::default(),
            color_scheme: "Hardcore".into(),
            custom_color_schemes: Vec::new(),
            default_profile: "git-bash".into(),
            profiles: Vec::new(),
            terminal: TerminalConfig::default(),
            hotkeys: BTreeMap::new(),
            claude_status: ClaudeStatusConfig::default(),
            restore_session: true,
            workspace: WorkspaceConfig::default(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig { family: "Consolas".into(), size: 15.0 }
    }
}

impl Default for CursorConfig {
    fn default() -> Self {
        CursorConfig { shape: CursorShapeConfig::Block, blink: false }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            scrollback: 25_000,
            cursor: CursorConfig::default(),
            right_click: RightClick::Clipboard,
            paste_on_middle_click: true,
            copy_on_select: false,
            allow_osc52: false,
            word_separators: " ()[]{}'\"".into(),
            bell: Bell::Off,
        }
    }
}

pub struct LoadOutcome {
    pub config: Config,
    /// Shown once to the user (e.g. the file was broken and was set aside).
    pub notice: Option<String>,
}

/// %APPDATA%\anvil
pub fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("anvil")
}

impl Config {
    pub fn path() -> PathBuf {
        app_dir().join("config.json")
    }

    /// Missing file: defaults. Broken file: renamed to
    /// `config.json.broken-<unix seconds>`, defaults, and a notice.
    pub fn load(path: &Path) -> LoadOutcome {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return LoadOutcome { config: Config::default(), notice: None };
            }
            Err(e) => {
                log::warn!("cannot read {}: {e}", path.display());
                return LoadOutcome { config: Config::default(), notice: Some(crate::strings::CONFIG_UNREADABLE.to_owned()) };
            }
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(config) => LoadOutcome { config: config.sanitized(), notice: None },
            Err(e) => {
                log::warn!("broken {}: {e}", path.display());
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let mut aside = path.as_os_str().to_os_string();
                aside.push(format!(".broken-{secs}"));
                let _ = std::fs::rename(path, &aside);
                LoadOutcome { config: Config::default(), notice: Some(crate::strings::CONFIG_BROKEN.to_owned()) }
            }
        }
    }

    /// Reload for the live watcher: a read or parse failure (an editor may be
    /// mid-write) leaves the active configuration alone and never quarantines
    /// the file; only startup treats a broken file as corruption.
    pub fn load_for_reload(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str::<Config>(&text).map(Config::sanitized).map_err(|e| e.to_string())
    }

    /// Values a hand-edited file may hold out of range, brought into the
    /// ranges the settings page allows: the font size goes straight into the
    /// font atlas (0 or 1e9 points is a crash or a giant allocation) and the
    /// scrollback is allocated per pane.
    fn sanitized(mut self) -> Config {
        use crate::term::view::{MAX_FONT_SIZE, MIN_FONT_SIZE};
        self.font.size = if self.font.size.is_finite() {
            self.font.size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
        } else {
            Config::default().font.size
        };
        self.terminal.scrollback = self.terminal.scrollback.min(MAX_SCROLLBACK);
        self
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(&self.to_minimal_json()).map_err(io::Error::other)?;
        atomic_write(path, format!("{text}\n").as_bytes())
    }

    /// `version` plus every value that differs from the default.
    pub fn to_minimal_json(&self) -> Value {
        let current = serde_json::to_value(self).expect("config serializes");
        let defaults = serde_json::to_value(Config::default()).expect("config serializes");
        let mut out = match diff(&current, &defaults) {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        out.shift_remove("version");
        let mut with_version = Map::new();
        with_version.insert("version".into(), Value::from(self.version));
        with_version.extend(out);
        Value::Object(with_version)
    }
}

fn diff(current: &Value, default: &Value) -> Option<Value> {
    match (current, default) {
        (Value::Object(c), Value::Object(d)) => {
            let mut out = Map::new();
            for (k, v) in c {
                match d.get(k) {
                    Some(dv) => {
                        if let Some(changed) = diff(v, dv) {
                            out.insert(k.clone(), changed);
                        }
                    }
                    None => {
                        out.insert(k.clone(), v.clone());
                    }
                }
            }
            (!out.is_empty()).then_some(Value::Object(out))
        }
        _ => (current != default).then(|| current.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let out = Config::load(&dir.path().join("config.json"));
        assert_eq!(out.config, Config::default());
        assert!(out.notice.is_none());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"version":1,"font":{"size":13},"terminal":{"rightClick":"menu"}}"#).unwrap();
        let c = Config::load(&path).config;
        assert_eq!(c.font.size, 13.0);
        assert_eq!(c.font.family, "Consolas");
        assert_eq!(c.terminal.right_click, RightClick::Menu);
        assert_eq!(c.terminal.scrollback, 25_000);
        assert!(!c.claude_status.enabled);
        assert!(!c.terminal.allow_osc52);
    }

    /// A hand-edited size went straight into the font atlas: 0 or 1e9 points
    /// (or a huge scrollback) is a crash or a gigabyte allocation at start.
    #[test]
    fn out_of_range_values_are_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for (size, expected) in [("0", 6.0), ("1e9", 48.0), ("-3", 6.0), ("14", 14.0)] {
            std::fs::write(&path, format!(r#"{{"version":1,"font":{{"size":{size}}}}}"#)).unwrap();
            assert_eq!(Config::load(&path).config.font.size, expected, "size {size}");
            assert_eq!(Config::load_for_reload(&path).unwrap().font.size, expected, "reload, size {size}");
        }
        std::fs::write(&path, r#"{"version":1,"terminal":{"scrollback":999999999999}}"#).unwrap();
        assert_eq!(Config::load(&path).config.terminal.scrollback, MAX_SCROLLBACK);
    }

    #[test]
    fn broken_file_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ not json").unwrap();
        let out = Config::load(&path);
        assert_eq!(out.config, Config::default());
        assert!(out.notice.is_some());
        assert!(!path.exists());
        let aside: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(aside.len(), 1);
        assert!(aside[0].to_string_lossy().starts_with("config.json.broken-"));
    }

    #[test]
    fn wrong_types_count_as_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"font": 5}"#).unwrap();
        assert!(Config::load(&path).notice.is_some());
    }

    #[test]
    fn save_writes_only_differences_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut c = Config::default();
        c.font.size = 13.0;
        c.hotkeys.insert("split-right".into(), vec!["Ctrl-Alt-S".into()]);
        c.save(&path).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written,
            serde_json::json!({"version": 1, "font": {"size": 13.0}, "hotkeys": {"split-right": ["Ctrl-Alt-S"]}})
        );
        assert_eq!(Config::load(&path).config, c);
    }

    #[test]
    fn selected_commit_model_survives_configuration_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = Config::default();
        config.workspace.ai_commit_command = Some("opencode".to_owned());
        config.workspace.ai_commit_model = Some("deepseek/deepseek-flash".to_owned());
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).config.workspace, config.workspace);
    }

    #[test]
    fn default_config_saves_as_version_only() {
        assert_eq!(Config::default().to_minimal_json(), serde_json::json!({"version": 1}));
    }
}
