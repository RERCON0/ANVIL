//! %APPDATA%\anvil\session.json: window geometry and tabs to restore at start.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::atomic_write;
use crate::layout::split_tree::{Dir, Node, PaneId};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionState {
    pub window: Option<WindowState>,
    pub active_tab: usize,
    pub tabs: Vec<TabState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabState {
    pub layout: SavedNode,
    /// Index of the focused pane in reading order.
    #[serde(default)]
    pub focused: usize,
    #[serde(default)]
    pub custom_title: Option<String>,
    /// The tab's colour mark; absent in sessions saved before it existed.
    #[serde(default)]
    pub color: Option<crate::theme::TabColor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SavedNode {
    Pane(PaneState),
    Split { dir: Dir, children: Vec<(f32, SavedNode)> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneState {
    pub profile_id: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Workspace (git) panel of this pane.
    #[serde(default)]
    pub workspace_open: bool,
    #[serde(default)]
    pub workspace_width: Option<f32>,
    /// Active workspace tab: "changes" or "files".
    #[serde(default)]
    pub workspace_tab: Option<String>,
}

impl SessionState {
    pub fn path() -> PathBuf {
        crate::config::app_dir().join("session.json")
    }

    /// Missing or broken file: empty state (the caller opens one default tab).
    pub fn load(path: &Path) -> SessionState {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring broken {}: {e}", path.display());
                SessionState::default()
            }),
            Err(_) => SessionState::default(),
        }
    }

    /// What a window opened with Ctrl+Shift+N starts from: the size of the
    /// saved window, moved down and right so the two do not cover each other,
    /// and no tabs. Restoring the tabs cloned the first window's whole
    /// session (and started a second copy of every shell).
    pub fn for_extra_window(self) -> SessionState {
        const CASCADE: i32 = 32;
        let window = self.window.map(|w| WindowState { x: w.x + CASCADE, y: w.y + CASCADE, maximized: false, ..w });
        SessionState { window, active_tab: 0, tabs: Vec::new() }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        atomic_write(path, text.as_bytes())
    }
}

impl SavedNode {
    /// Builds a layout tree, creating a pane for every saved pane.
    pub fn to_node(&self, make_pane: &mut dyn FnMut(&PaneState) -> PaneId) -> Node {
        match self {
            SavedNode::Pane(p) => Node::Leaf(make_pane(p)),
            SavedNode::Split { dir, children } => Node::Split {
                dir: *dir,
                children: children.iter().map(|(f, c)| (*f, c.to_node(make_pane))).collect(),
            },
        }
    }

    pub fn from_node(node: &Node, describe: &dyn Fn(PaneId) -> PaneState) -> SavedNode {
        match node {
            Node::Leaf(id) => SavedNode::Pane(describe(*id)),
            Node::Split { dir, children } => SavedNode::Split {
                dir: *dir,
                children: children.iter().map(|(f, c)| (*f, SavedNode::from_node(c, describe))).collect(),
            },
        }
    }
}

/// The saved directory if it still exists, else None (use the profile's).
/// Only a path on a local drive counts: a UNC or device path restored from a
/// session file would make the pane's ConPTY dial that host — and authenticate
/// to it — while starting the shell, so it is rejected before any filesystem
/// call (`is_dir` on a UNC path is itself the connection).
pub fn usable_cwd(saved: Option<&Path>) -> Option<PathBuf> {
    saved.filter(|p| is_local_dir(p)).map(Path::to_path_buf)
}

fn is_local_dir(path: &Path) -> bool {
    let mut components = path.components();
    let local = matches!(
        components.next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))
    );
    local && components.next().is_some() && path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extra_window_starts_empty_and_offset() {
        let session = SessionState {
            window: Some(WindowState { x: 100, y: 50, width: 1200, height: 800, maximized: true }),
            active_tab: 2,
            tabs: vec![TabState {
                layout: SavedNode::Pane(PaneState {
                    profile_id: "pwsh".into(),
                    cwd: None,
                    workspace_open: false,
                    workspace_width: None,
                    workspace_tab: None,
                }),
                focused: 0,
                custom_title: None,
                color: None,
            }],
        };
        let extra = session.for_extra_window();
        assert!(extra.tabs.is_empty() && extra.active_tab == 0);
        assert_eq!(extra.window, Some(WindowState { x: 132, y: 82, width: 1200, height: 800, maximized: false }));
    }

    fn pane(profile: &str, cwd: Option<&str>) -> PaneState {
        PaneState {
            profile_id: profile.into(),
            cwd: cwd.map(PathBuf::from),
            workspace_open: false,
            workspace_width: None,
            workspace_tab: None,
        }
    }

    fn sample() -> SessionState {
        SessionState {
            window: Some(WindowState { x: 10, y: 20, width: 1600, height: 900, maximized: true }),
            active_tab: 1,
            tabs: vec![
                TabState {
                    layout: SavedNode::Pane(pane("git-bash", Some("C:\\work"))),
                    focused: 0,
                    custom_title: None,
                    color: Some(crate::theme::TabColor::Green),
                },
                TabState {
                    layout: SavedNode::Split {
                        dir: Dir::Row,
                        children: vec![
                            (0.5, SavedNode::Pane(pane("git-bash", None))),
                            (0.5, SavedNode::Pane(pane("powershell", Some("C:\\")))),
                        ],
                    },
                    focused: 1,
                    custom_title: Some("сервер".into()),
                    color: None,
                },
            ],
        }
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        sample().save(&path).unwrap();
        assert_eq!(SessionState::load(&path), sample());
    }

    /// A tab colour is written by name, and a session saved before the field
    /// existed loads as uncoloured instead of failing the whole file.
    #[test]
    fn a_tab_colour_survives_and_an_older_session_has_none() {
        let text = serde_json::to_string(&sample()).unwrap();
        assert!(text.contains("\"color\":\"green\""), "colour is written by name: {text}");
        let older = r#"{"tabs":[{"layout":{"pane":{"profileId":"pwsh"}}}]}"#;
        let loaded: SessionState = serde_json::from_str(older).unwrap();
        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.tabs[0].color, None, "an older session must still load");
    }

    #[test]
    fn missing_or_broken_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        assert_eq!(SessionState::load(&path), SessionState::default());
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(SessionState::load(&path), SessionState::default());
    }

    #[test]
    fn converts_to_and_from_layout_nodes() {
        let saved = sample().tabs[1].layout.clone();
        let mut next = 100;
        let mut made = Vec::new();
        let node = saved.to_node(&mut |p| {
            next += 1;
            made.push((next, p.clone()));
            next
        });
        assert_eq!(
            node,
            Node::Split { dir: Dir::Row, children: vec![(0.5, Node::Leaf(101)), (0.5, Node::Leaf(102))] }
        );
        let back = SavedNode::from_node(&node, &|id| made.iter().find(|(i, _)| *i == id).unwrap().1.clone());
        assert_eq!(back, saved);
    }

    #[test]
    fn cwd_falls_back_when_gone() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(usable_cwd(Some(dir.path())), Some(dir.path().to_path_buf()));
        assert_eq!(usable_cwd(Some(Path::new("C:\\definitely\\not\\here"))), None);
        assert_eq!(usable_cwd(None), None);
    }

    /// A crafted session must not send the pane's shell to a remote host: the
    /// rejection is lexical, before `is_dir`, which would itself open the
    /// connection and offer the user's credentials.
    #[test]
    fn a_remote_path_is_rejected_before_it_is_touched() {
        for path in ["\\\\attacker\\share", "//attacker/share", "\\\\?\\UNC\\attacker\\share"] {
            assert_eq!(usable_cwd(Some(Path::new(path))), None, "{path}");
        }
    }
}
