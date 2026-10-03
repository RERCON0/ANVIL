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
pub fn usable_cwd(saved: Option<&Path>) -> Option<PathBuf> {
    saved.filter(|p| p.is_dir()).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(profile: &str, cwd: Option<&str>) -> PaneState {
        PaneState { profile_id: profile.into(), cwd: cwd.map(PathBuf::from) }
    }

    fn sample() -> SessionState {
        SessionState {
            window: Some(WindowState { x: 10, y: 20, width: 1600, height: 900, maximized: true }),
            active_tab: 1,
            tabs: vec![
                TabState { layout: SavedNode::Pane(pane("git-bash", Some("C:\\work"))), focused: 0, custom_title: None },
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
}
