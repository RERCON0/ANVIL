//! Development capture with real installed CLI processes and a disposable Git workspace.
//! Reads the normal CLI logins and the existing quota snapshot, without changing ANVIL settings.
use super::*;
use crate::layout::split_tree::Node;

fn installed(name: &str) -> String {
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let file = dir.join(format!("{name}.exe"));
        if file.is_file() {
            return file.to_string_lossy().into_owned();
        }
    }
    if name == "codex" {
        let file = PathBuf::from(std::env::var_os("APPDATA").expect("APPDATA"))
            .join("npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
        if file.is_file() {
            return file.to_string_lossy().into_owned();
        }
    }
    panic!("Install {name} before capturing the README");
}

impl AnvilApp {
    pub fn readme_scene(root: PathBuf) -> Self {
        let config = Config {
            language: strings::Language::English,
            font: crate::config::FontConfig { family: "Consolas".into(), size: 14.0 },
            quota: crate::config::QuotaConfig { enabled: true, ..Default::default() },
            ..Default::default()
        };
        let session = SessionState {
            window: Some(WindowState { x: 0, y: 0, width: 1920, height: 1540, maximized: false }),
            ..Default::default()
        };
        let mut app = Self::from_state(config, root.join("config.json"), None, session, true, root.join("status"));
        app.readme_root = Some(root);
        app
    }

    pub(super) fn start_readme_scene(&mut self) {
        let root = self.readme_root.clone().expect("capture root");
        let snapshot =
            crate::quota::cache::read(&crate::quota::Paths::in_dir(&crate::quota::Paths::default_dir()).snapshot)
                .expect("An existing quota snapshot is required; never invent usage figures");
        self.quota = Some(crate::quota::QuotaHandle::read_only(snapshot, &root));
        let agents = [
            ("Codex", "codex", vec!["--no-daemon", "--no-alt-screen"]),
            ("Claude Code", "claude", vec![]),
            ("OpenCode", "opencode", vec!["--standalone"]),
            ("OMP", "omp", vec!["--no-session", "--no-title", "--no-extensions"]),
        ];
        for (project, color) in
            [("ANVIL", theme::TabColor::Green), ("BEAT", theme::TabColor::Blue), ("SNATCH", theme::TabColor::Purple)]
        {
            let mut panes = HashMap::new();
            let mut ids = Vec::new();
            for (index, (agent, executable, args)) in agents.iter().enumerate() {
                if project != "ANVIL" && index > 0 {
                    break;
                }
                let id = self.alloc_pane_id();
                let profile = Profile {
                    id: format!("capture-{executable}"),
                    name: (*agent).into(),
                    command: if project == "ANVIL" {
                        installed(executable)
                    } else {
                        format!(
                            "{}\\System32\\cmd.exe",
                            std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())
                        )
                    },
                    args: if project == "ANVIL" {
                        args.iter().map(|s| (*s).into()).collect()
                    } else {
                        vec!["/d".into()]
                    },
                    cwd: Some(root.join(project)),
                    env: if *executable == "opencode" {
                        std::collections::BTreeMap::from([
                            ("XDG_CONFIG_HOME".into(), root.join("config").to_string_lossy().into_owned()),
                            ("OPENCODE_CONFIG_DIR".into(), root.join("config/opencode").to_string_lossy().into_owned()),
                        ])
                    } else {
                        Default::default()
                    },
                    kind: profiles::ProfileKind::Custom,
                };
                let mut entry = self.spawn_entry(id, &profile, Some(root.join(project)));
                entry.title = (*agent).into();
                if index == 1 {
                    entry.workspace.open = true;
                    entry.workspace.width = 380.0;
                    entry.workspace.commit_message = "Polish workspace controls and harden release packaging".into();
                }
                ids.push(id);
                panes.insert(id, entry);
            }
            let tree = if ids.len() == 4 {
                SplitTree::from_root(Node::Split {
                    dir: Dir::Row,
                    children: vec![
                        (
                            0.60,
                            Node::Split {
                                dir: Dir::Column,
                                children: vec![(0.28, Node::Leaf(ids[0])), (0.72, Node::Leaf(ids[1]))],
                            },
                        ),
                        (
                            0.40,
                            Node::Split {
                                dir: Dir::Column,
                                children: vec![(0.50, Node::Leaf(ids[2])), (0.50, Node::Leaf(ids[3]))],
                            },
                        ),
                    ],
                })
            } else {
                SplitTree::new(ids[0])
            };
            let mut tab = Tab::new(tree, panes, ids[0]);
            tab.custom_title = Some(project.into());
            tab.color = Some(color);
            self.tabs.push(tab);
        }
        self.active = 0;
    }
    /// Only dismiss the update offer and accept the empty repository made by this example.
    /// No model prompt, tool request or update is ever submitted.
    pub(super) fn readme_startup_keys(&mut self) {
        if self.readme_root.is_none() {
            return;
        }
        for tab in &self.tabs {
            for (id, entry) in &tab.panes {
                let Some(pane) = entry.live() else { continue };
                let text: String = pane.term.lock().renderable_content().display_iter.map(|c| c.cell.c).collect();
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let now = Instant::now();
                let phase = if entry.profile_id == "capture-codex" && text.contains("Update now (runs") {
                    Some(0)
                } else if entry.profile_id == "capture-codex" && text.contains("Trust and continue") {
                    Some(1)
                } else if entry.profile_id == "capture-claude" && text.contains("No, exit") {
                    Some(2)
                } else {
                    None
                };
                if let Some(phase) = phase {
                    let first = *self.readme_keys.entry((*id, phase)).or_insert(now);
                    if now.duration_since(first) < Duration::from_secs(1) {
                        continue;
                    }
                    if !self.readme_keys.contains_key(&(*id, phase + 10)) {
                        self.readme_keys.insert((*id, phase + 10), now);
                        match phase {
                            0 => pane.write(vec![0x1b]),
                            1 => pane.write(vec![b'\r']),
                            _ => {
                                use crate::term::input::{encode, InputModes, KeyPress};
                                let modes = InputModes {
                                    app_cursor: pane
                                        .term
                                        .lock()
                                        .mode()
                                        .contains(alacritty_terminal::term::TermMode::APP_CURSOR),
                                };
                                let press = KeyPress { key: Some(crate::hotkeys::KeyName::Down), ..Default::default() };
                                pane.write(encode(&press, modes).expect("down key"));
                            }
                        }
                    } else if phase == 2 {
                        let selected = self.readme_keys[&(*id, phase + 10)];
                        if now.duration_since(selected) > Duration::from_secs(1)
                            && !self.readme_keys.contains_key(&(*id, 22))
                        {
                            self.readme_keys.insert((*id, 22), now);
                            pane.write(vec![b'\r']);
                        }
                    }
                }
            }
        }
    }
}
