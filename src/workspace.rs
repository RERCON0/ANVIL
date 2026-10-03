//! Per-pane workspace panel: git status grouped by folder, selective staging,
//! a diff preview and the commit box (with an optional AI-written message).
//!
//! Every `git` call happens on a worker thread: the panel only sends requests
//! and consumes responses, so the UI never blocks on the repository.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Align, Align2, Layout, Rect, RichText, ScrollArea, Sense, Stroke, Vec2};

use crate::git::{self, Change, Status};
use crate::strings;
use crate::theme;

const POLL_INTERVAL: Duration = Duration::from_millis(1500);
pub const MIN_WIDTH: f32 = 260.0;
pub const MAX_WIDTH: f32 = 720.0;

pub enum Request {
    Refresh { cwd: PathBuf },
    Diff { path: String },
    Stage { paths: Vec<String>, staged: bool },
    Commit { message: String },
    AiMessage { command: Option<String> },
}

pub enum Response {
    Status(Status),
    Diff { path: String, text: String },
    Refreshed,
    Committed(String),
    AiMessage(String),
    Error(String),
}

pub struct Workspace {
    pub open: bool,
    pub width: f32,
    pub root: Option<PathBuf>,
    pub status: Status,
    pub selected: HashSet<String>,
    pub collapsed: HashSet<String>,
    pub diff_path: Option<String>,
    pub diff_text: String,
    pub commit_message: String,
    pub notice: Option<(String, bool)>,
    pub busy: bool,
    pub last_poll: Instant,
    tx: Option<Sender<Request>>,
    rx: Option<Receiver<Response>>,
    diff_side: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            open: false,
            width: 340.0,
            root: None,
            status: Status::default(),
            selected: HashSet::new(),
            collapsed: HashSet::new(),
            diff_path: None,
            diff_text: String::new(),
            commit_message: String::new(),
            notice: None,
            busy: false,
            last_poll: Instant::now() - POLL_INTERVAL,
            tx: None,
            rx: None,
            diff_side: false,
        }
    }
}

impl Workspace {
    /// Kicks off the worker thread (once) and sends a refresh.
    pub fn poll(&mut self, cwd: PathBuf) {
        if self.tx.is_none() {
            let (tx, rx) = spawn_worker();
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        if self.busy || self.last_poll.elapsed() < POLL_INTERVAL {
            return;
        }
        self.last_poll = Instant::now();
        self.busy = true;
        self.send(Request::Refresh { cwd });
    }

    /// A refresh is due right now (after stage/commit, or on open).
    pub fn refresh_soon(&mut self) {
        self.last_poll = Instant::now() - POLL_INTERVAL;
    }

    fn send(&self, request: Request) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(request);
        }
    }

    /// Applies one worker response; returns true when the UI should repaint.
    pub fn absorb(&mut self) -> bool {
        let mut repaint = false;
        let mut responses = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(response) = rx.try_recv() {
                responses.push(response);
            }
        }
        for response in responses {
            repaint = true;
            match response {
                Response::Status(status) => {
                    self.busy = false;
                    self.status = status;
                    self.selected.retain(|path| self.status.changes.iter().any(|change| &change.path == path));
                    if let Some(path) = self.diff_path.clone() {
                        if !self.status.changes.iter().any(|change| change.path == path) {
                            self.diff_path = None;
                            self.diff_text.clear();
                        } else {
                            self.send(Request::Diff { path });
                        }
                    }
                }
                Response::Diff { path, text } => {
                    self.busy = false;
                    if self.diff_path.as_deref() == Some(path.as_str()) {
                        self.diff_text = text;
                    }
                }
                Response::Refreshed => {
                    self.busy = false;
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                }
                Response::Committed(hash) => {
                    self.busy = false;
                    self.commit_message.clear();
                    self.notice = Some((strings::workspace_committed(&hash), false));
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                }
                Response::AiMessage(message) => {
                    self.busy = false;
                    self.commit_message = message;
                }
                Response::Error(message) => {
                    self.busy = false;
                    self.notice = Some((message, true));
                }
            }
        }
        repaint
    }

    /// Draws the panel; returns actions the app must handle.
    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, ai_command: Option<&str>) -> Vec<WorkspaceAction> {
        let mut actions = Vec::new();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::LIFT);
        painter.vline(rect.min.x + 0.5, rect.y_range(), Stroke::new(1.0, theme::LINE));
        let inner = rect.shrink2(Vec2::new(10.0, 8.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("git").color(theme::FAINT).font(theme::font(11.5)));
                let (branch, colour) = if self.status.branch.is_empty() || self.status.branch == "HEAD" {
                    (strings::WORKSPACE_NO_REPO.to_owned(), theme::DIM)
                } else {
                    (self.status.branch.clone(), theme::TEXT)
                };
                ui.label(RichText::new(branch).color(colour).font(theme::field_font(12.5)));
                if !self.status.branch.is_empty() {
                    if self.status.ahead > 0 {
                        ui.label(RichText::new(format!("↑{}", self.status.ahead)).color(theme::ACCENT).font(theme::field_font(12.0)));
                    }
                    if self.status.behind > 0 {
                        ui.label(RichText::new(format!("↓{}", self.status.behind)).color(theme::STATUS_YELLOW).font(theme::field_font(12.0)));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(theme::ghost_button("×")).on_hover_text(strings::WORKSPACE_CLOSE).clicked() {
                        actions.push(WorkspaceAction::Close);
                    }
                    if ui.add(theme::ghost_button("⟳")).on_hover_text(strings::WORKSPACE_REFRESH).clicked() {
                        self.last_poll = Instant::now() - POLL_INTERVAL;
                    }
                });
            });
            theme::hairline(ui);
            if let Some((notice, error)) = &self.notice {
                let colour = if *error { theme::STATUS_RED } else { theme::STATUS_GREEN };
                ui.label(RichText::new(notice).color(colour).font(theme::font(11.5)));
            }
            if self.status.changes.is_empty() && self.status.branch.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(strings::WORKSPACE_NO_REPO_HINT).color(theme::FAINT).font(theme::font(12.0)));
                return;
            }

            let (staged, unstaged) = self.counts();
            ui.label(RichText::new(strings::workspace_staged(staged, unstaged)).color(theme::DIM).font(theme::font(11.5)));
            let selected: Vec<String> = self.selected.iter().cloned().collect();
            let has_selection = !selected.is_empty();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(has_selection, theme::ghost_button(strings::WORKSPACE_STAGE)).clicked() {
                    self.busy = true;
                    self.send(Request::Stage { paths: selected.clone(), staged: true });
                }
                if ui.add_enabled(has_selection, theme::ghost_button(strings::WORKSPACE_UNSTAGE)).clicked() {
                    self.busy = true;
                    self.send(Request::Stage { paths: selected, staged: false });
                }
                if ui.add(theme::ghost_button(strings::WORKSPACE_STAGE_ALL)).clicked() {
                    let paths: Vec<String> = self.status.changes.iter().filter(|c| c.unstaged()).map(|c| c.path.clone()).collect();
                    self.busy = true;
                    self.send(Request::Stage { paths, staged: true });
                }
                if ui.add(theme::ghost_button(strings::WORKSPACE_UNSTAGE_ALL)).clicked() {
                    let paths: Vec<String> = self.status.changes.iter().filter(|c| c.staged()).map(|c| c.path.clone()).collect();
                    self.busy = true;
                    self.send(Request::Stage { paths, staged: false });
                }
            });

            // Rows are painted on a fixed grid: mixed widget heights made the
            // checkbox, badge and text sit on different baselines.
            const ROW_HEIGHT: f32 = 21.0;
            const INDENT: f32 = 12.0;
            let list_height = (ui.available_height() * 0.42).clamp(120.0, 320.0);
            ScrollArea::vertical().id_salt("workspace-changes").max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
                for row in self.rows() {
                    let width = ui.available_width();
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
                    let painter = ui.painter_at(rect);
                    if response.hovered() {
                        painter.rect_filled(rect, 0.0, theme::TAB_HOVER_BG);
                    }
                    match row {
                        Row::Folder { path, depth, count } => {
                            let collapsed = self.collapsed.contains(&path);
                            let name = path.rsplit('/').next().unwrap_or(&path);
                            let caret = if collapsed { "▸" } else { "▾" };
                            painter.text(
                                egui::Pos2::new(rect.min.x + depth as f32 * INDENT, rect.center().y),
                                Align2::LEFT_CENTER,
                                format!("{caret} {name}/"),
                                theme::font(12.0),
                                theme::DIM,
                            );
                            painter.text(
                                egui::Pos2::new(rect.max.x, rect.center().y),
                                Align2::RIGHT_CENTER,
                                count.to_string(),
                                theme::field_font(11.0),
                                theme::FAINT,
                            );
                            if response.clicked() {
                                if collapsed {
                                    self.collapsed.remove(&path);
                                } else {
                                    self.collapsed.insert(path.clone());
                                }
                            }
                        }
                        Row::File { change } => {
                            let path = change.path.clone();
                            let depth = change.path.matches('/').count();
                            let selected = self.selected.contains(&path);
                            let box_x = rect.min.x + depth as f32 * INDENT;
                            painter.text(
                                egui::Pos2::new(box_x, rect.center().y),
                                Align2::LEFT_CENTER,
                                if selected { "[×]" } else { "[ ]" },
                                theme::field_font(12.0),
                                if selected { theme::ACCENT } else { theme::FAINT },
                            );
                            painter.text(
                                egui::Pos2::new(box_x + 32.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                change.letter().to_string(),
                                theme::field_font(12.0),
                                status_color(change.letter()),
                            );
                            painter.text(
                                egui::Pos2::new(box_x + 48.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                change.file_name(),
                                theme::font(12.0),
                                theme::TEXT,
                            );
                            let mut stat = String::new();
                            if change.additions > 0 {
                                stat.push_str(&format!("+{}", change.additions));
                            }
                            if change.deletions > 0 {
                                stat.push_str(&format!(" −{}", change.deletions));
                            }
                            painter.text(
                                egui::Pos2::new(rect.max.x, rect.center().y),
                                Align2::RIGHT_CENTER,
                                stat,
                                theme::field_font(11.0),
                                theme::FAINT,
                            );
                            if response.clicked() {
                                let checkbox_hit = response.interact_pointer_pos().is_some_and(|pos| pos.x < box_x + 28.0);
                                if checkbox_hit {
                                    if selected {
                                        self.selected.remove(&path);
                                    } else {
                                        self.selected.insert(path.clone());
                                    }
                                } else {
                                    self.diff_path = Some(path.clone());
                                    self.diff_text.clear();
                                    self.busy = true;
                                    self.send(Request::Diff { path });
                                }
                            }
                        }
                    }
                }
            });

            if let Some(path) = self.diff_path.clone() {
                ui.add_space(4.0);
                theme::hairline(ui);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&path).color(theme::DIM).font(theme::field_font(11.5)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(theme::ghost_button(strings::WORKSPACE_DIFF_STAGED)).clicked() {
                            self.diff_side = !self.diff_side;
                            self.diff_text.clear();
                            self.busy = true;
                            self.send(Request::Diff { path: path.clone() });
                        }
                    });
                });
                let height = (ui.available_height() * 0.45).clamp(100.0, 300.0);
                ScrollArea::vertical().id_salt("workspace-diff").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                    for line in self.diff_text.lines() {
                        let colour = if line.starts_with('+') && !line.starts_with("+++") {
                            theme::STATUS_GREEN
                        } else if line.starts_with('-') && !line.starts_with("---") {
                            theme::STATUS_RED
                        } else if line.starts_with("@@") {
                            theme::STATUS_YELLOW
                        } else {
                            theme::DIM
                        };
                        ui.label(RichText::new(line).color(colour).font(theme::field_font(11.0)));
                    }
                });
            }

            ui.add_space(4.0);
            theme::hairline(ui);
            let commit = ui.add(
                egui::TextEdit::multiline(&mut self.commit_message)
                    .font(theme::field_font(12.0))
                    .desired_rows(3)
                    .hint_text(strings::WORKSPACE_COMMIT_HINT)
                    .desired_width(f32::INFINITY),
            );
            let ctrl_enter = commit.has_focus() && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                let can_commit = !self.commit_message.trim().is_empty() && self.status.changes.iter().any(Change::staged);
                if ui.add_enabled(can_commit, theme::accent_button(strings::WORKSPACE_COMMIT)).clicked() || (can_commit && ctrl_enter) {
                    self.busy = true;
                    self.notice = None;
                    self.send(Request::Commit { message: self.commit_message.clone() });
                }
                let ai_label = match ai_command {
                    Some(command) => strings::workspace_ai(command),
                    None => strings::WORKSPACE_AI.to_owned(),
                };
                if ui.add(theme::ghost_button(ai_label)).clicked() {
                    self.busy = true;
                    self.notice = None;
                    self.send(Request::AiMessage { command: ai_command.map(str::to_owned) });
                }
            });
        });
        actions
    }

    fn counts(&self) -> (usize, usize) {
        let staged = self.status.changes.iter().filter(|c| c.staged()).count();
        let unstaged = self.status.changes.iter().filter(|c| c.unstaged()).count();
        (staged, unstaged)
    }

    /// Flattened rows: folder headers followed by their files, honoring
    /// `collapsed` (changes are sorted by path, so folders come in order).
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut index = 0;
        while index < self.status.changes.len() {
            let dir = self.status.changes[index].directory().to_owned();
            let mut end = index;
            while end < self.status.changes.len() && self.status.changes[end].directory() == dir {
                end += 1;
            }
            if !dir.is_empty() {
                let depth = dir.matches('/').count();
                rows.push(Row::Folder { path: dir.clone(), depth, count: end - index });
                let collapsed = (0..=depth).any(|level| {
                    let prefix = dir.split('/').take(level + 1).collect::<Vec<_>>().join("/");
                    self.collapsed.contains(&prefix)
                });
                if collapsed {
                    index = end;
                    continue;
                }
            }
            for change in &self.status.changes[index..end] {
                rows.push(Row::File { change: change.clone() });
            }
            index = end;
        }
        rows
    }
}

pub enum WorkspaceAction {
    Close,
}

enum Row {
    Folder { path: String, depth: usize, count: usize },
    File { change: Change },
}

fn status_color(letter: char) -> egui::Color32 {
    match letter {
        'A' | '?' => theme::STATUS_GREEN,
        'D' => theme::STATUS_RED,
        'M' | 'T' => theme::STATUS_YELLOW,
        'R' | 'C' => theme::ACCENT,
        'U' => theme::STATUS_RED,
        _ => theme::DIM,
    }
}

fn spawn_worker() -> (Sender<Request>, Receiver<Response>) {
    let (request_tx, request_rx) = mpsc::channel::<Request>();
    let (response_tx, response_rx) = mpsc::channel::<Response>();
    std::thread::spawn(move || {
        let mut root: Option<PathBuf> = None;
        let mut cwd: Option<PathBuf> = None;
        while let Ok(request) = request_rx.recv() {
            match request {
                Request::Refresh { cwd: new_cwd } => {
                    if cwd.as_ref() != Some(&new_cwd) || root.is_none() {
                        root = git::find_root(&new_cwd);
                        cwd = Some(new_cwd);
                    }
                    match &root {
                        Some(root) => match git::status(root) {
                            Ok(status) => {
                                let _ = response_tx.send(Response::Status(status));
                            }
                            Err(e) => {
                                let _ = response_tx.send(Response::Error(e));
                            }
                        },
                        None => {
                            let _ = response_tx.send(Response::Status(Status::default()));
                        }
                    }
                }
                Request::Diff { path } => {
                    if let Some(root) = &root {
                        let mut text = git::diff(root, &path, true);
                        if text.trim().is_empty() {
                            text = git::diff(root, &path, false);
                        }
                        let _ = response_tx.send(Response::Diff { path, text });
                    }
                }
                Request::Stage { paths, staged } => {
                    let result = root.as_ref().map(|root| git::stage(root, &paths, staged));
                    let _ = response_tx.send(match result {
                        Some(Ok(())) => Response::Refreshed,
                        Some(Err(e)) => Response::Error(e),
                        None => Response::Error(strings::WORKSPACE_NO_REPO.to_owned()),
                    });
                }
                Request::Commit { message } => {
                    let result = root.as_ref().map(|root| git::commit(root, &message));
                    let _ = response_tx.send(match result {
                        Some(Ok(hash)) => Response::Committed(hash),
                        Some(Err(e)) => Response::Error(e),
                        None => Response::Error(strings::WORKSPACE_NO_REPO.to_owned()),
                    });
                }
                Request::AiMessage { command } => {
                    let result = root.as_ref().map(|root| {
                        let staged = git::run_git(root, &["diff", "--cached", "--no-color", "--unified=1"]).unwrap_or_default();
                        let unstaged = git::run_git(root, &["diff", "--no-color", "--unified=1"]).unwrap_or_default();
                        let status = git::run_git(root, &["status", "--porcelain"]).unwrap_or_default();
                        let recent = git::recent_subjects(root, 8);
                        let diff = format!("{staged}\n{unstaged}");
                        let prompt = git::ai_prompt(&status, diff.trim(), &recent);
                        git::ai_commit_message(root, command.as_deref(), &prompt)
                    });
                    let _ = response_tx.send(match result {
                        Some(Ok(message)) => Response::AiMessage(message),
                        Some(Err(e)) => Response::Error(e),
                        None => Response::Error(strings::WORKSPACE_NO_REPO.to_owned()),
                    });
                }
            }
        }
    });
    (request_tx, response_rx)
}

/// Whether the panel wants the keyboard (the commit box is focused).
pub fn clamp_width(width: f32) -> f32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}
