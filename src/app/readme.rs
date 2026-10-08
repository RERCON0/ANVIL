//! Development capture with real installed CLI processes and a disposable Git workspace.
//! Reads the normal CLI logins and the existing quota snapshot, without changing ANVIL settings.
use super::*;
use crate::layout::split_tree::Node;

/// Owns prewarmed real sessions while the demo introduces them one at a time.
/// It is a development fixture, not a launcher or a persisted user workspace.
pub(crate) struct ReadmeDemo {
    ids: Vec<PaneId>,
    pids: HashMap<PaneId, u32>,
    pending: HashMap<PaneId, PaneEntry>,
    tabs: std::collections::VecDeque<Tab>,
    last_step: Option<usize>,
    drag: Option<(egui::Pos2, egui::Pos2)>,
    pub cursor: Option<egui::Pos2>,
}

impl ReadmeDemo {
    pub fn prepare(app: &mut AnvilApp) -> Self {
        assert!(app.readme_root.is_some(), "demo must use an isolated README scene");
        let tabs = app.tabs.drain(1..).collect();
        let tab = &mut app.tabs[0];
        let ids = tab.tree.panes();
        assert_eq!(ids.len(), 4);
        let mut pids = HashMap::new();
        for id in &ids {
            let entry = &mut tab.panes.get_mut(id).unwrap();
            let pane = entry.live().expect("the demo requires real CLI processes");
            assert!(!entry.exited);
            pids.insert(*id, pane.shell_pid);
            let text: String = pane.term.lock().renderable_content().display_iter.map(|c| c.cell.c).collect();
            assert!(
                !text.contains("No, exit") && !text.contains("Trust and continue"),
                "CLI trust must be accepted first"
            );
            entry.workspace.open = false;
        }
        let pending = ids.iter().skip(1).map(|id| (*id, tab.panes.remove(id).unwrap())).collect();
        tab.tree = SplitTree::new(ids[0]);
        tab.set_focus(ids[0]);
        app.active = 0;
        Self { ids, pids, pending, tabs, last_step: None, drag: None, cursor: None }
    }

    pub fn step(&mut self, app: &mut AnvilApp, input: &mut egui::RawInput, ctx: &egui::Context, frame: usize) {
        let fresh = self.last_step != Some(frame);
        if fresh {
            self.last_step = Some(frame);
            match frame {
                15 | 25 => {
                    app.tabs.push(self.tabs.pop_front().unwrap());
                    app.active = app.tabs.len() - 1;
                }
                35 => app.active = 0,
                40 | 50 | 60 => {
                    let index = (frame - 30) / 10;
                    let id = self.ids[index];
                    let entry = self.pending.remove(&id).unwrap();
                    let tab = &mut app.tabs[0];
                    let (target, dir) = match index {
                        1 => (self.ids[0], Dir::Column),
                        2 => (self.ids[0], Dir::Row),
                        _ => (self.ids[2], Dir::Column),
                    };
                    assert!(tab.tree.insert(target, id, dir, true));
                    if index == 2 {
                        assert!(tab.tree.relocate(id, None, Dir::Row, true));
                    }
                    tab.panes.insert(id, entry);
                    tab.set_focus(id);
                }
                70 => {
                    app.focus_pane(self.ids[1]);
                    app.tabs[0].panes.get_mut(&self.ids[1]).unwrap().workspace.open = true;
                }
                _ => {}
            }
        }
        input.focused = true;
        // Program-owned input targets only this disposable egui instance.
        input.events.retain(|e| {
            !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone)
        });
        input.modifiers = if (80..=104).contains(&frame) {
            egui::Modifiers { ctrl: true, shift: true, command: true, ..Default::default() }
        } else {
            egui::Modifiers::default()
        };
        if frame == 80 && self.drag.is_none() {
            let area = app.last_tab_area;
            let rects = app.tabs[0].tree.layout(
                crate::layout::split_tree::Rect::new(area.min.x, area.min.y, area.width(), area.height()),
                theme::DIVIDER_WIDTH,
            );
            let source = rects.iter().find(|(id, _)| *id == self.ids[2]).unwrap().1;
            let target = rects.iter().find(|(id, _)| *id == self.ids[3]).unwrap().1;
            self.drag = Some((
                egui::pos2(source.x + source.w / 2.0, source.y + source.h / 2.0),
                egui::pos2(target.x + target.w / 2.0, target.bottom() - 60.0),
            ));
        }
        if let Some((from, to)) = self.drag.filter(|_| (80..=104).contains(&frame)) {
            let share = ((frame.saturating_sub(82)) as f32 / 18.0).clamp(0.0, 1.0);
            let point = from + (to - from) * share;
            self.cursor = Some(point);
            input.events.push(egui::Event::PointerMoved(point));
            if fresh && (frame == 81 || frame == 104) {
                input.events.push(egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: frame == 81,
                    modifiers: input.modifiers,
                });
            }
        } else {
            self.cursor = None;
            input.events.push(egui::Event::PointerGone);
        }
        ctx.request_repaint();
    }

    pub fn verify(&self, app: &AnvilApp) {
        assert_eq!(
            app.tabs[0].tree.panes(),
            vec![self.ids[0], self.ids[1], self.ids[3], self.ids[2]],
            "the real drag must reorder the panes"
        );
        assert!(self.pending.is_empty() && self.tabs.is_empty());
        for (id, pid) in &self.pids {
            let entry = &app.tabs[0].panes[id];
            assert!(
                !entry.exited && entry.live().unwrap().shell_pid == *pid,
                "moving a pane must keep its live process"
            );
        }
        println!("Verified real drag-and-drop and unchanged CLI process IDs");
    }
}

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
