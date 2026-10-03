//! Per-pane workspace panel: git changes with selective and hunk staging, a
//! diff preview, the commit box (with an optional AI-written message), the
//! commit graph and a file browser.
//!
//! Every `git` call happens on a worker thread: the panel only sends requests
//! and consumes responses, so the UI never blocks on the repository.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{Align, Align2, Layout, Rect, RichText, ScrollArea, Sense, Stroke, Vec2};

use crate::git::{self, Change, CommitLog, FileDiff, Status};
use crate::graph;
use crate::strings;
use crate::theme;

const POLL_INTERVAL: Duration = Duration::from_millis(1500);
pub const MIN_WIDTH: f32 = 300.0;
pub const MAX_WIDTH: f32 = 800.0;
const LANE_WIDTH: f32 = 11.0;
/// Room the commit subject keeps even when a row is crowded with ref chips.
const MIN_SUBJECT_WIDTH: f32 = 72.0;

pub enum Request {
    Refresh { cwd: PathBuf },
    Diff { path: String },
    Stage { paths: Vec<String>, staged: bool },
    Commit { message: String },
    AiMessage { command: Option<String> },
    Log,
    CommitDetail { hash: String },
    Files,
    ReadFile { path: String },
    /// `header` is the `@@ … @@` line the user saw; the worker refuses to apply
    /// when the freshly generated diff no longer has it at `index`.
    ApplyHunks { path: String, index: usize, header: String, from_index: bool },
    Fetch,
    Push,
    WritePath { path: String, folder: bool },
    RenamePath { from: String, to: String },
    DeletePath { path: String },
}

pub enum Response {
    /// Status plus the resolved repository root (None: not a repository).
    Status(Status, Option<PathBuf>),
    Diff { path: String, files: Vec<FileDiff>, text: String, staged: bool },
    Refreshed,
    Committed(String),
    AiMessage(String),
    Log(CommitLog),
    CommitDetail { hash: String, detail: git::CommitDetail },
    Files(Vec<String>),
    FileText { path: String, text: String, truncated: bool },
    Applied,
    Fetched(String),
    Pushed(String),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    /// Changes and commits in one column.
    Changes,
    Files,
}

impl PanelTab {
    pub fn as_str(self) -> &'static str {
        match self {
            PanelTab::Changes => "changes",
            PanelTab::Files => "files",
        }
    }

    pub fn parse(text: &str) -> PanelTab {
        match text {
            "files" => PanelTab::Files,
            _ => PanelTab::Changes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    NewFile,
    NewFolder,
    Rename(String),
    Delete(String),
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    pub focus: bool,
}

pub struct Workspace {
    pub open: bool,
    pub width: f32,
    pub tab: PanelTab,
    pub root: Option<PathBuf>,
    pub status: Status,
    pub selected: HashSet<String>,
    pub collapsed: HashSet<String>,
    pub diff_path: Option<String>,
    pub diff_text: String,
    pub diff_files: Vec<FileDiff>,
    pub diff_from_index: bool,
    pub commit_message: String,
    pub notice: Option<(String, bool)>,
    pub busy: bool,
    pub last_poll: Instant,
    pub log: CommitLog,
    pub graph: Vec<graph::Row>,
    pub detail: Option<(String, git::CommitDetail)>,
    pub files: Vec<String>,
    pub file_filter: String,
    pub file_preview: Option<(String, String, bool)>,
    pub prompt: Option<Prompt>,
    ai_command: Option<String>,
    tx: Option<Sender<Request>>,
    rx: Option<Receiver<Response>>,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            open: false,
            width: 380.0,
            tab: PanelTab::Changes,
            root: None,
            status: Status::default(),
            selected: HashSet::new(),
            collapsed: HashSet::new(),
            diff_path: None,
            diff_text: String::new(),
            diff_files: Vec::new(),
            diff_from_index: false,
            commit_message: String::new(),
            notice: None,
            busy: false,
            last_poll: Instant::now() - POLL_INTERVAL,
            log: CommitLog::default(),
            graph: Vec::new(),
            detail: None,
            files: Vec::new(),
            file_filter: String::new(),
            file_preview: None,
            prompt: None,
            ai_command: None,
            tx: None,
            rx: None,
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

    /// Applies worker responses; returns true when the UI should repaint.
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
                Response::Status(status, root) => {
                    self.busy = false;
                    // Another repository invalidates everything read from the
                    // previous one: its commits, file browser and open commit
                    // must never be shown under the new header.
                    let root_changed = self.root != root;
                    if root_changed {
                        self.selected.clear();
                        self.diff_path = None;
                        self.diff_text.clear();
                        self.diff_files.clear();
                        self.detail = None;
                        self.files.clear();
                        self.file_preview = None;
                        self.log = CommitLog::default();
                        self.graph.clear();
                    }
                    self.root = root;
                    self.status = status;
                    self.selected.retain(|path| self.status.changes.iter().any(|change| &change.path == path));
                    if let Some(path) = self.diff_path.clone() {
                        if !self.status.changes.iter().any(|change| change.path == path) {
                            self.diff_path = None;
                            self.diff_text.clear();
                            self.diff_files.clear();
                        } else {
                            self.send(Request::Diff { path });
                        }
                    }
                    if root_changed || self.log.commits.is_empty() {
                        self.send(Request::Log);
                    }
                    if root_changed || (self.tab == PanelTab::Files && self.files.is_empty()) {
                        self.send(Request::Files);
                    }
                }
                Response::Diff { path, files, text, staged } => {
                    self.busy = false;
                    if self.diff_path.as_deref() == Some(path.as_str()) {
                        self.diff_text = text;
                        self.diff_files = files;
                        self.diff_from_index = staged;
                    }
                }
                Response::Refreshed | Response::Applied => {
                    self.busy = false;
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                    self.send(Request::Log);
                    if self.tab == PanelTab::Files {
                        self.send(Request::Files);
                    }
                }
                Response::Committed(hash) => {
                    self.busy = false;
                    self.commit_message.clear();
                    self.notice = Some((strings::workspace_committed(&hash), false));
                    self.send(Request::Log);
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                }
                Response::AiMessage(message) => {
                    self.busy = false;
                    self.commit_message = message;
                }
                Response::Log(log) => {
                    self.busy = false;
                    self.graph = graph::compute(&log.commits);
                    self.log = log;
                }
                Response::CommitDetail { hash, detail } => {
                    self.busy = false;
                    self.detail = Some((hash, detail));
                }
                Response::Files(files) => {
                    self.busy = false;
                    self.files = files;
                }
                Response::FileText { path, text, truncated } => {
                    self.busy = false;
                    self.file_preview = Some((path, text, truncated));
                }
                Response::Fetched(what) => {
                    self.busy = false;
                    self.notice = Some((strings::workspace_fetch_done(&what), false));
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                }
                Response::Pushed(branch) => {
                    self.busy = false;
                    self.notice = Some((strings::workspace_pushed(&branch), false));
                    self.last_poll = Instant::now() - POLL_INTERVAL;
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
    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, pane: crate::layout::split_tree::PaneId, ai_command: Option<&str>) -> Vec<WorkspaceAction> {
        self.ai_command = ai_command.map(str::to_owned);
        let mut actions = Vec::new();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::LIFT);
        painter.vline(rect.min.x + 0.5, rect.y_range(), Stroke::new(1.0, theme::LINE));
        let inner = rect.shrink2(Vec2::new(10.0, 8.0));
        // Every pane has its own panel with its own widgets: without the pane
        // in the salt two panels share one id, and egui paints a clash overlay
        // over them (and their scroll state is shared).
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner).id_salt(("workspace-panel", pane)), |ui| {
            // Two modes only: the changes view keeps commits in the same
            // column (one scroll), the file browser is separate.
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (PanelTab::Files, strings::WORKSPACE_TAB_FILES.to_owned()),
                    (PanelTab::Changes, strings::workspace_changes_tab(self.status.changes.len())),
                ] {
                    // Framed like the other panel chips, with the selection dot
                    // of the settings rows inside.
                    let selected = self.tab == tab;
                    let (dot, color) = if selected { ("●", theme::ACCENT) } else { ("○", theme::DIM) };
                    let text = egui::RichText::new(format!("{dot} {label}")).font(theme::field_font(12.5)).color(color);
                    if ui.add(egui::Button::new(text)).clicked() {
                        self.tab = tab;
                        self.refresh_soon();
                        match tab {
                            PanelTab::Changes => {
                                self.send(Request::Log);
                            }
                            PanelTab::Files => self.send(Request::Files),
                        }
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(theme::ghost_button("×")).on_hover_text(strings::WORKSPACE_CLOSE).clicked() {
                        actions.push(WorkspaceAction::Close);
                    }
                    if ui.add(theme::ghost_button("⟳")).on_hover_text(strings::WORKSPACE_REFRESH).clicked() {
                        self.refresh_soon();
                        self.send(Request::Log);
                        self.send(Request::Files);
                    }
                });
            });
            theme::hairline(ui);
            if self.prompt.is_some() {
                self.prompt_row(ui);
            }
            match self.tab {
                PanelTab::Changes => self.changes_mode(ui),
                PanelTab::Files => self.files_tab(ui),
            }
        });
        actions
    }

    /// One column: repo header, commit box, changes, then the commit history.
    fn changes_mode(&mut self, ui: &mut egui::Ui) {
        if self.status.changes.is_empty() && self.status.branch.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(strings::WORKSPACE_NO_REPO_HINT).color(theme::FAINT).font(theme::font(12.0)));
            return;
        }
        if self.detail_view(ui) {
            return;
        }
        ScrollArea::vertical().id_salt("workspace-column").auto_shrink([false, false]).show(ui, |ui| {
            self.repo_row(ui);
            self.commit_box(ui);
            self.change_rows(ui);
            ui.add_space(6.0);
            self.commits_section(ui);
        });
    }

    /// Repo name, branch, counters and the publish/fetch actions.
    fn repo_row(&mut self, ui: &mut egui::Ui) {
        let name = self
            .root
            .as_ref()
            .and_then(|root| root.file_name().map(|name| name.to_string_lossy().into_owned()))
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).color(theme::TEXT).font(theme::font(12.5)));
            if !self.status.branch.is_empty() {
                ui.label(RichText::new(&self.status.branch).color(theme::ACCENT).font(theme::field_font(12.0)));
                if self.status.ahead > 0 {
                    ui.label(RichText::new(format!("↑{}", self.status.ahead)).color(theme::ACCENT).font(theme::field_font(11.5)));
                }
                if self.status.behind > 0 {
                    ui.label(RichText::new(format!("↓{}", self.status.behind)).color(theme::STATUS_YELLOW).font(theme::field_font(11.5)));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.status.branch.is_empty() {
                    if ui.add(theme::accent_button(strings::WORKSPACE_PUBLISH)).on_hover_text(strings::WORKSPACE_PUSH_HINT).clicked() {
                        self.busy = true;
                        self.send(Request::Push);
                    }
                    if ui.add(theme::ghost_button("fetch")).on_hover_text(strings::WORKSPACE_FETCH_HINT).clicked() {
                        self.busy = true;
                        self.send(Request::Fetch);
                    }
                }
            });
        });
        let additions: u32 = self.status.changes.iter().map(|c| c.additions).sum();
        let deletions: u32 = self.status.changes.iter().map(|c| c.deletions).sum();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("+{additions}")).color(theme::STATUS_GREEN).font(theme::field_font(11.5)));
            ui.label(RichText::new(format!("−{deletions}")).color(theme::STATUS_RED).font(theme::field_font(11.5)));
            ui.label(RichText::new(strings::workspace_staged(self.counts().0, self.counts().1)).color(theme::FAINT).font(theme::font(11.0)));
        });
        if let Some((notice, error)) = &self.notice {
            let colour = if *error { theme::STATUS_RED } else { theme::STATUS_GREEN };
            ui.label(RichText::new(notice).color(colour).font(theme::font(11.5)));
        }
        theme::hairline(ui);
    }

    fn commit_box(&mut self, ui: &mut egui::Ui) {
        let commit = ui.add(
            egui::TextEdit::multiline(&mut self.commit_message)
                .font(theme::field_font(12.0))
                .desired_rows(3)
                .hint_text(strings::WORKSPACE_COMMIT_HINT)
                .desired_width(f32::INFINITY),
        );
        let ctrl_enter = commit.has_focus() && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        ui.horizontal_wrapped(|ui| {
            let can_commit = !self.commit_message.trim().is_empty() && self.status.changes.iter().any(Change::staged);
            if ui.add_enabled(can_commit, theme::accent_button(strings::WORKSPACE_COMMIT)).clicked() || (can_commit && ctrl_enter) {
                self.busy = true;
                self.notice = None;
                self.send(Request::Commit { message: self.commit_message.clone() });
            }
            let ai_label = match &self.ai_command {
                Some(command) => strings::workspace_ai(command),
                None => strings::WORKSPACE_AI.to_owned(),
            };
            let ai = self.ai_command.clone();
            if ui.add(theme::ghost_button(ai_label)).clicked() {
                self.busy = true;
                self.notice = None;
                self.send(Request::AiMessage { command: ai });
            }
        });
        ui.add_space(4.0);
    }

    /// `[ КОММИТЫ ]` header plus the graph list, inline in the same column.
    fn commits_section(&mut self, ui: &mut egui::Ui) {
        let ahead = self.status.ahead;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("[ {} ]", strings::WORKSPACE_COMMITS_TITLE)).color(theme::FAINT).font(theme::font(11.5)));
            if ahead > 0 {
                ui.label(RichText::new(format!("↑{ahead}")).color(theme::ACCENT).font(theme::field_font(11.5)));
            }
            if self.log.truncated {
                ui.label(RichText::new(strings::WORKSPACE_TRUNCATED).color(theme::FAINT).font(theme::font(10.5)));
            }
        });
        theme::hairline(ui);
        if self.log.commits.is_empty() {
            ui.label(RichText::new(strings::WORKSPACE_NO_COMMITS).color(theme::FAINT).font(theme::font(11.5)));
            return;
        }
        let mut current_section = None;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        const ROW_HEIGHT: f32 = 22.0;
        for (index, commit) in self.log.commits.clone().iter().enumerate() {
            if current_section != Some(commit.section) {
                current_section = Some(commit.section);
                let label = match commit.section {
                    git::Section::Outgoing => strings::WORKSPACE_SECTION_OUTGOING,
                    git::Section::Incoming => strings::WORKSPACE_SECTION_INCOMING,
                    git::Section::History => strings::WORKSPACE_SECTION_HISTORY,
                };
                ui.add_space(4.0);
                ui.label(RichText::new(label).color(theme::FAINT).font(theme::font(10.5)));
            }
            let row = self.graph.get(index).cloned().unwrap_or(graph::Row { lane: 0, lane_count: 1, segments: Vec::new() });
            let width = ui.available_width();
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
            let painter = ui.painter_at(rect);
            if response.hovered() {
                painter.rect_filled(rect, 0.0, theme::TAB_HOVER_BG);
            }
            let lane_x = |lane: usize| rect.min.x + lane as f32 * LANE_WIDTH + LANE_WIDTH / 2.0;
            let mid = rect.center().y;
            for segment in &row.segments {
                let stroke = Stroke::new(1.5, section_color(commit.section));
                let (from, to) = (lane_x(segment.from_lane), lane_x(segment.to_lane));
                match segment.kind {
                    graph::Kind::Through => {
                        painter.line_segment([egui::Pos2::new(from, rect.min.y), egui::Pos2::new(from, rect.max.y)], stroke);
                    }
                    graph::Kind::Up if from == to => {
                        painter.line_segment([egui::Pos2::new(from, rect.min.y), egui::Pos2::new(from, mid)], stroke);
                    }
                    graph::Kind::Down if from == to => {
                        painter.line_segment([egui::Pos2::new(from, mid), egui::Pos2::new(from, rect.max.y)], stroke);
                    }
                    graph::Kind::Up => {
                        let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                            [
                                egui::Pos2::new(from, rect.min.y),
                                egui::Pos2::new(from, mid),
                                egui::Pos2::new(to, rect.min.y),
                                egui::Pos2::new(to, mid),
                            ],
                            false,
                            egui::Color32::TRANSPARENT,
                            stroke,
                        );
                        painter.add(curve);
                    }
                    graph::Kind::Down => {
                        let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                            [
                                egui::Pos2::new(from, mid),
                                egui::Pos2::new(from, rect.max.y),
                                egui::Pos2::new(to, mid),
                                egui::Pos2::new(to, rect.max.y),
                            ],
                            false,
                            egui::Color32::TRANSPARENT,
                            stroke,
                        );
                        painter.add(curve);
                    }
                }
            }
            painter.circle_filled(egui::Pos2::new(lane_x(row.lane), mid), 3.2, section_color(commit.section));

            // One compact line: subject, refs, then time and hash on the right.
            let graph_width = row.lane_count as f32 * LANE_WIDTH;
            let text_x = rect.min.x + graph_width + 7.0;
            let time_text = crate::strings::relative_time(now, commit.time);
            let time_galley = painter.layout_no_wrap(time_text.clone(), theme::font(10.0), theme::FAINT);
            let hash_galley = painter.layout_no_wrap(commit.short.clone(), theme::field_font(10.0), theme::FAINT);
            let right_width = time_galley.size().x + hash_galley.size().x + 12.0;
            let right_x = rect.max.x - 2.0;
            painter.galley(
                egui::Pos2::new(right_x - time_galley.size().x, mid - time_galley.size().y / 2.0),
                time_galley,
                theme::FAINT,
            );
            painter.galley(
                egui::Pos2::new(right_x - right_width, mid - hash_galley.size().y / 2.0),
                hash_galley,
                theme::FAINT,
            );
            let mut badge_x = text_x;
            for reference in &commit.refs {
                let name = display(reference.strip_prefix("HEAD -> ").unwrap_or(reference), 40);
                let colour = ref_color(&name);
                let galley = painter.layout_no_wrap(name, theme::field_font(10.0), theme::CHROME_BG);
                let badge = Rect::from_min_size(
                    egui::Pos2::new(badge_x, mid - 7.0),
                    Vec2::new(galley.size().x + 10.0, 14.0),
                );
                // A chip is only worth drawing while the subject keeps room to
                // the left of the time and hash column.
                if badge.max.x > rect.max.x - right_width - 10.0 - MIN_SUBJECT_WIDTH {
                    break;
                }
                painter.rect_filled(badge, egui::Rounding::same(3.0), colour);
                painter.galley(egui::Pos2::new(badge.min.x + 5.0, badge.min.y + 1.0), galley, theme::CHROME_BG);
                badge_x = badge.max.x + 4.0;
            }
            let subject_limit = rect.max.x - right_width - 8.0 - badge_x;
            if subject_limit > 16.0 {
                let subject = elide(&painter, &display(&commit.subject, 200), theme::font(12.0), subject_limit);
                painter.text(
                    egui::Pos2::new(badge_x, mid),
                    Align2::LEFT_CENTER,
                    subject,
                    theme::font(12.0),
                    theme::TEXT,
                );
            }
            if response.hovered() {
                let tooltip = format!("{}\n{} · {}\n{}", commit.subject, commit.author, commit.short, time_text);
                let _ = response.clone().on_hover_text(tooltip);
            }
            if response.clicked() {
                self.busy = true;
                self.send(Request::CommitDetail { hash: commit.hash.clone() });
            }
        }
    }

    fn prompt_row(&mut self, ui: &mut egui::Ui) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        let label = match &prompt.kind {
            PromptKind::NewFile => strings::WORKSPACE_NEW_FILE.to_owned(),
            PromptKind::NewFolder => strings::WORKSPACE_NEW_FOLDER.to_owned(),
            PromptKind::Rename(path) => format!("{}: {path}", strings::WORKSPACE_RENAME),
            PromptKind::Delete(path) => format!("{}: {path}", strings::WORKSPACE_DELETE),
        };
        ui.label(RichText::new(label).color(theme::DIM).font(theme::font(11.5)));
        let mut commit = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut prompt.text)
                    .font(theme::field_font(12.0))
                    .desired_width(180.0),
            );
            if prompt.focus {
                field.request_focus();
                prompt.focus = false;
            }
            commit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.add(theme::ghost_button(strings::SETTINGS_SAVE)).clicked() {
                commit = true;
            }
            if ui.add(theme::ghost_button(strings::SETTINGS_CANCEL)).clicked() {
                cancel = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
        if commit {
            let Some(prompt) = self.prompt.take() else { return };
            let text = prompt.text.trim().to_owned();
            match prompt.kind {
                PromptKind::NewFile if !text.is_empty() => {
                    self.busy = true;
                    self.send(Request::WritePath { path: text, folder: false });
                }
                PromptKind::NewFolder if !text.is_empty() => {
                    self.busy = true;
                    self.send(Request::WritePath { path: text, folder: true });
                }
                PromptKind::Rename(from) if !text.is_empty() => {
                    self.busy = true;
                    self.send(Request::RenamePath { from, to: text });
                }
                PromptKind::Delete(path) => {
                    self.busy = true;
                    self.send(Request::DeletePath { path });
                }
                _ => {}
            }
        } else if cancel {
            self.prompt = None;
        }
    }

    // ---- changes ---------------------------------------------------------

    /// Stage buttons, the change tree and (when a file is picked) its diff.
    fn change_rows(&mut self, ui: &mut egui::Ui) {
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
        const ROW_HEIGHT: f32 = 21.0;
        const INDENT: f32 = 12.0;
        let rows = self.rows();
        // With many files the tree gets its own bounded scroll, so the commit
        // section below stays reachable; short lists stay inline.
        let bounded = |ui: &mut egui::Ui, body: &mut dyn FnMut(&mut egui::Ui)| {
            if rows.len() > 12 {
                ScrollArea::vertical().id_salt("workspace-changes").max_height(300.0).auto_shrink([false, false]).show(ui, body);
            } else {
                body(ui);
            }
        };
        {
            bounded(ui, &mut |ui: &mut egui::Ui| {
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
                            let name = display(path.rsplit('/').next().unwrap_or(&path), 40);
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
                            let (icon, icon_color) = crate::file_icons::for_file(change.file_name());
                            painter.text(
                                egui::Pos2::new(box_x + 30.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                icon,
                                theme::icon_font(13.0),
                                icon_color,
                            );
                            painter.text(
                                egui::Pos2::new(box_x + 48.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                display(change.file_name(), 60),
                                theme::font(12.0),
                                theme::TEXT,
                            );
                            let letter = change.letter();
                            let letter_galley = painter.layout_no_wrap(letter.to_string(), theme::field_font(11.5), status_color(letter));
                            let letter_width = letter_galley.size().x;
                            painter.galley(
                                egui::Pos2::new(rect.max.x - 2.0 - letter_width, rect.center().y - letter_galley.size().y / 2.0),
                                letter_galley,
                                status_color(letter),
                            );
                            let mut stat = String::new();
                            if change.additions > 0 {
                                stat.push_str(&format!("+{} ", change.additions));
                            }
                            if change.deletions > 0 {
                                stat.push_str(&format!("−{}", change.deletions));
                            }
                            painter.text(
                                egui::Pos2::new(rect.max.x - letter_width - 8.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                stat,
                                theme::field_font(10.5),
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
                                    self.diff_files.clear();
                                    self.busy = true;
                                    self.send(Request::Diff { path });
                                }
                            }
                        }
                    }
                }
            });
        }

        if self.diff_path.is_some() {
            self.diff_view(ui);
        }
        ui.add_space(4.0);
    }

    fn diff_view(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.diff_path.clone() else { return };
        ui.add_space(4.0);
        theme::hairline(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new(display(&path, 120)).color(theme::DIM).font(theme::field_font(11.5)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::ghost_button(strings::WORKSPACE_DIFF_STAGED)).clicked() {
                    self.diff_text.clear();
                    self.diff_files.clear();
                    self.busy = true;
                    self.send(Request::Diff { path: path.clone() });
                }
            });
        });
        let mut hunks_to_apply: Option<(usize, String, bool)> = None;
        {
            let ui = &mut *ui;
            let mut hunk_index = 0;
            for line in self.diff_text.lines() {
                if line.starts_with("@@") {
                    let label = if self.diff_from_index { "◂" } else { "▸" };
                    let header = line.to_owned();
                    ui.horizontal(|ui| {
                        if ui.add(theme::ghost_button(label)).clicked() {
                            hunks_to_apply = Some((hunk_index, header.clone(), self.diff_from_index));
                        }
                        ui.label(RichText::new(line).color(theme::STATUS_YELLOW).font(theme::field_font(11.0)));
                    });
                    hunk_index += 1;
                    continue;
                }
                let colour = if line.starts_with('+') && !line.starts_with("+++") {
                    theme::STATUS_GREEN
                } else if line.starts_with('-') && !line.starts_with("---") {
                    theme::STATUS_RED
                } else {
                    theme::DIM
                };
                ui.label(RichText::new(line).color(colour).font(theme::field_font(11.0)));
            }
        }
        if let Some((index, header, from_index)) = hunks_to_apply {
            self.busy = true;
            self.send(Request::ApplyHunks { path, index, header, from_index });
        }
    }

    /// Commit detail replaces the column while open.
    fn detail_view(&mut self, ui: &mut egui::Ui) -> bool {
        match self.detail.clone() {
            Some((hash, detail)) => {
                self.commit_detail_view(ui, &hash, &detail);
                true
            }
            None => false,
        }
    }

    fn commit_detail_view(&mut self, ui: &mut egui::Ui, hash: &str, detail: &git::CommitDetail) {
        ui.horizontal(|ui| {
            if ui.add(theme::ghost_button(strings::WORKSPACE_BACK)).clicked() {
                self.detail = None;
            }
            if ui.add(theme::ghost_button(strings::WORKSPACE_COPY_HASH)).clicked() {
                copy_to_clipboard(hash);
            }
            if ui.add(theme::ghost_button(strings::WORKSPACE_COPY_PATCH)).clicked() {
                copy_to_clipboard(&detail.patch);
            }
        });
        let mut lines = detail.header.lines();
        let full_hash = lines.next().unwrap_or("").to_owned();
        let author = lines.next().unwrap_or("").to_owned();
        let date = lines.next().unwrap_or("").to_owned();
        let subject = display(lines.next().unwrap_or(""), 300);
        let body: String = display(&lines.collect::<Vec<_>>().join("\n"), 2000);
        ui.label(RichText::new(subject).color(theme::TEXT).font(theme::font(12.5)));
        ui.label(RichText::new(format!("{} · {}", display(&author, 80), display(&date, 40))).color(theme::FAINT).font(theme::font(10.5)));
        ui.label(RichText::new(full_hash).color(theme::FAINT).font(theme::field_font(10.5)));
        if !body.trim().is_empty() {
            ui.label(RichText::new(body.trim()).color(theme::DIM).font(theme::font(11.0)));
        }
        theme::hairline(ui);
        for (status, path, additions, deletions) in &detail.files {
            ui.horizontal(|ui| {
                ui.label(RichText::new(status.to_string()).color(status_color(*status)).font(theme::field_font(11.5)));
                ui.label(RichText::new(display(path, 120)).color(theme::TEXT).font(theme::font(11.5)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("+{additions} −{deletions}")).color(theme::FAINT).font(theme::field_font(10.5)));
                });
            });
        }
        theme::hairline(ui);
        ScrollArea::vertical().id_salt("workspace-commit-patch").auto_shrink([false, false]).show(ui, |ui| {
            for line in detail.patch.lines() {
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

    // ---- files -----------------------------------------------------------

    fn files_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::ghost_button(strings::WORKSPACE_NEW_FILE)).clicked() {
                self.prompt = Some(Prompt { kind: PromptKind::NewFile, text: String::new(), focus: true });
            }
            if ui.add(theme::ghost_button(strings::WORKSPACE_NEW_FOLDER)).clicked() {
                self.prompt = Some(Prompt { kind: PromptKind::NewFolder, text: String::new(), focus: true });
            }
            if ui.add(theme::ghost_button(strings::WORKSPACE_REFRESH)).clicked() {
                self.busy = true;
                self.send(Request::Files);
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.file_filter)
                .font(theme::field_font(12.0))
                .hint_text(strings::WORKSPACE_FILE_FILTER)
                .desired_width(f32::INFINITY),
        );
        let mut matches: Vec<(i32, &String)> = self
            .files
            .iter()
            .filter_map(|path| crate::profiles::fuzzy_score(&self.file_filter, path).map(|score| (score, path)))
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        const ROW_HEIGHT: f32 = 20.0;
        let list_height = (ui.available_height() * 0.45).clamp(100.0, 320.0);
        let mut open_file: Option<String> = None;
        ScrollArea::vertical().id_salt("workspace-files").max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
            for (_, path) in matches.iter().take(400) {
                let width = ui.available_width();
                let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
                let painter = ui.painter_at(rect);
                if response.hovered() {
                    painter.rect_filled(rect, 0.0, theme::TAB_HOVER_BG);
                }
                let (dir, name) = match path.rfind('/') {
                    Some(index) => (&path[..index + 1], &path[index + 1..]),
                    None => ("", path.as_str()),
                };
                let name = display(name, 80);
                let dir = display(dir, 80);
                let (icon, icon_color) = crate::file_icons::for_file(&name);
                painter.text(
                    egui::Pos2::new(rect.min.x + 4.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    icon,
                    theme::icon_font(13.0),
                    icon_color,
                );
                let mut x = rect.min.x + 24.0;
                if !dir.is_empty() {
                    let galley = painter.layout_no_wrap(dir.to_owned(), theme::font(11.0), theme::FAINT);
                    painter.galley(egui::Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley.clone(), theme::FAINT);
                    x += galley.size().x;
                }
                painter.text(
                    egui::Pos2::new(x, rect.center().y),
                    Align2::LEFT_CENTER,
                    name,
                    theme::font(11.5),
                    theme::TEXT,
                );
                if response.clicked() {
                    open_file = Some(path.to_string());
                }
                let path = (*path).clone();
                response.context_menu(|ui| {
                    if ui.button(strings::WORKSPACE_OPEN_EXTERNAL).clicked() {
                        open_external(&self.root, &path);
                        ui.close_menu();
                    }
                    if ui.button(strings::WORKSPACE_REVEAL).clicked() {
                        reveal_in_explorer(&self.root, &path);
                        ui.close_menu();
                    }
                    if ui.button(strings::WORKSPACE_RENAME).clicked() {
                        self.prompt = Some(Prompt { kind: PromptKind::Rename(path.clone()), text: path.clone(), focus: true });
                        ui.close_menu();
                    }
                    if ui.button(strings::WORKSPACE_DELETE).clicked() {
                        self.prompt = Some(Prompt { kind: PromptKind::Delete(path.clone()), text: String::new(), focus: false });
                        ui.close_menu();
                    }
                });
            }
        });
        if let Some(path) = open_file {
            self.busy = true;
            self.send(Request::ReadFile { path });
        }
        if let Some((path, text, truncated)) = self.file_preview.clone() {
            ui.add_space(2.0);
            theme::hairline(ui);
            ui.label(RichText::new(&path).color(theme::DIM).font(theme::field_font(11.0)));
            let height = (ui.available_height() * 0.5).clamp(80.0, 320.0);
            ScrollArea::vertical().id_salt("workspace-file-preview").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                for (index, line) in text.lines().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{:>4}", index + 1))
                                .color(theme::FAINT)
                                .font(theme::field_font(10.5)),
                        );
                        ui.label(RichText::new(line).color(theme::DIM).font(theme::field_font(11.0)));
                    });
                }
                if truncated {
                    ui.label(RichText::new(strings::WORKSPACE_FILE_TRUNCATED).color(theme::STATUS_YELLOW).font(theme::font(11.0)));
                }
            });
        }
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

/// Graph colours by commit section: not-yet-pushed commits are VS Code blue,
/// already-pushed history the owner's pale pink (#D488B4), incoming purple.
fn section_color(section: git::Section) -> egui::Color32 {
    match section {
        git::Section::Outgoing => egui::Color32::from_rgb(0x59, 0xA4, 0xF9),
        git::Section::Incoming => egui::Color32::from_rgb(0xB1, 0x80, 0xD7),
        git::Section::History => egui::Color32::from_rgb(0xD4, 0x88, 0xB4),
    }
}

/// Untrusted names, subjects and bodies may contain control or bidirectional
/// characters: they would break the fixed row grid or spoof a file extension,
/// so they are stripped and bounded before painting.
fn display(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max_chars * 4));
    let mut count = 0;
    for ch in text.chars() {
        if ch.is_control() || is_format_control(ch) {
            continue;
        }
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

fn is_format_control(ch: char) -> bool {
    matches!(ch, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// Truncates text to `max_width` with an ellipsis.
fn elide(painter: &egui::Painter, text: &str, font: egui::FontId, max_width: f32) -> String {
    let measure = |value: &str| painter.layout_no_wrap(value.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
    if measure(text) <= max_width {
        return text.to_owned();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let mut candidate = out.clone();
        candidate.push(ch);
        candidate.push('…');
        if measure(&candidate) > max_width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// VS Code's ref colours: local branch (charts.blue), remote (charts.purple).
fn ref_color(reference: &str) -> egui::Color32 {
    if reference.contains("remotes/") || reference.starts_with("origin/") {
        egui::Color32::from_rgb(0xB1, 0x80, 0xD7)
    } else if reference.starts_with("tag:") {
        egui::Color32::from_rgb(0xCC, 0xA7, 0x00)
    } else {
        egui::Color32::from_rgb(0x59, 0xA4, 0xF9)
    }
}

fn status_color(letter: char) -> egui::Color32 {
    match letter {
        'A' | '?' => theme::STATUS_GREEN,
        'D' | 'U' => theme::STATUS_RED,
        'M' | 'T' => theme::STATUS_YELLOW,
        'R' | 'C' => theme::ACCENT,
        _ => theme::DIM,
    }
}

fn copy_to_clipboard(text: &str) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text.to_owned());
    }
}

fn open_external(root: &Option<PathBuf>, path: &str) {
    if let Some(root) = root {
        crate::settings_ui::open_path(&root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR)));
    }
}

fn reveal_in_explorer(root: &Option<PathBuf>, path: &str) {
    let Some(root) = root else { return };
    let full = root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let mut command = std::process::Command::new("explorer.exe");
    command.arg(format!("/select,{}", full.display()));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let _ = command.spawn();
}

fn spawn_worker() -> (Sender<Request>, Receiver<Response>) {
    let (request_tx, request_rx) = mpsc::channel::<Request>();
    let (response_tx, response_rx) = mpsc::channel::<Response>();
    std::thread::spawn(move || {
        let mut root: Option<PathBuf> = None;
        let mut cwd: Option<PathBuf> = None;
        while let Ok(request) = request_rx.recv() {
            let send = |response: Response| {
                let _ = response_tx.send(response);
            };
            match request {
                Request::Refresh { cwd: new_cwd } => {
                    if cwd.as_ref() != Some(&new_cwd) || root.is_none() {
                        root = git::find_root(&new_cwd);
                        cwd = Some(new_cwd);
                    }
                    match &root {
                        Some(root) => match git::status(root) {
                            Ok(status) => send(Response::Status(status, Some(root.clone()))),
                            Err(e) => send(Response::Error(e)),
                        },
                        None => send(Response::Status(Status::default(), None)),
                    }
                }
                Request::Diff { path } => match &root {
                    Some(root) => {
                        let staged = git::diff(root, &path, true);
                        let (text, from_index) = if staged.trim().is_empty() {
                            (git::diff(root, &path, false), false)
                        } else {
                            (staged, true)
                        };
                        let files = git::parse_diff(&text, from_index);
                        send(Response::Diff { path, files, text, staged: from_index });
                    }
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::Stage { paths, staged } => match &root {
                    Some(root) => match git::stage(root, &paths, staged) {
                        Ok(()) => send(Response::Refreshed),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::Commit { message } => match &root {
                    Some(root) => match git::commit(root, &message) {
                        Ok(hash) => send(Response::Committed(hash)),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
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
                    match result {
                        Some(Ok(message)) => send(Response::AiMessage(message)),
                        Some(Err(e)) => send(Response::Error(e)),
                        None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                    }
                }
                Request::Log => match &root {
                    Some(root) => match git::log(root) {
                        Ok(log) => send(Response::Log(log)),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Log(CommitLog::default())),
                },
                Request::CommitDetail { hash } => match &root {
                    Some(root) => match git::commit_detail(root, &hash) {
                        Ok(detail) => send(Response::CommitDetail { hash, detail }),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::Files => match &root {
                    Some(root) => match git::ls_files(root) {
                        Ok(files) => send(Response::Files(files)),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Files(Vec::new())),
                },
                Request::ReadFile { path } => match &root {
                    Some(root) => match git::read_file(root, &path, 512 * 1024) {
                        Ok((text, truncated)) => send(Response::FileText { path, text, truncated }),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::ApplyHunks { path, index, header, from_index } => match &root {
                    Some(root) => {
                        let text = git::diff(root, &path, from_index);
                        let files = git::parse_diff(&text, from_index);
                        let result = files
                            .iter()
                            .find(|file| file.path == path)
                            .ok_or_else(|| strings::WORKSPACE_NO_CHANGES_FOR_FILE.to_owned())
                            .and_then(|file| match file.hunks.get(index) {
                                // The file may have changed since it was shown:
                                // apply only the hunk the user actually saw.
                                Some(hunk) if hunk.header == header => git::apply_hunks(root, file, &[index], from_index),
                                Some(_) => Err(strings::WORKSPACE_DIFF_STALE.to_owned()),
                                None => Err(strings::WORKSPACE_DIFF_STALE.to_owned()),
                            });
                        match result {
                            Ok(()) => send(Response::Applied),
                            Err(e) => send(Response::Error(e)),
                        }
                    }
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::Fetch => match &root {
                    Some(root) => match git::fetch(root) {
                        Ok(what) => send(Response::Fetched(what)),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::Push => match &root {
                    Some(root) => match git::push(root) {
                        Ok(branch) => send(Response::Pushed(branch)),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::WritePath { path, folder } => match &root {
                    Some(root) => match write_path(root, &path, folder) {
                        Ok(()) => send(Response::Applied),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::RenamePath { from, to } => match &root {
                    Some(root) => match rename_path(root, &from, &to) {
                        Ok(()) => send(Response::Applied),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::DeletePath { path } => match &root {
                    Some(root) => match delete_path(root, &path) {
                        Ok(()) => send(Response::Applied),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
            }
        }
    });
    (request_tx, response_rx)
}

fn write_path(root: &Path, path: &str, folder: bool) -> Result<(), String> {
    let full = git::resolve_path(root, path)?;
    if folder {
        std::fs::create_dir_all(&full).map_err(|e| e.to_string())
    } else {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if full.exists() {
            return Err(strings::WORKSPACE_FILE_EXISTS.to_owned());
        }
        std::fs::write(&full, "").map_err(|e| e.to_string())
    }
}

fn rename_path(root: &Path, from: &str, to: &str) -> Result<(), String> {
    let from = git::resolve_path(root, from)?;
    let to = git::resolve_path(root, to)?;
    if !from.exists() {
        return Err(strings::WORKSPACE_NO_SUCH_FILE.to_owned());
    }
    std::fs::rename(&from, &to).map_err(|e| e.to_string())
}

fn delete_path(root: &Path, path: &str) -> Result<(), String> {
    let full = git::resolve_path(root, path)?;
    if full.is_dir() {
        std::fs::remove_dir_all(&full).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(&full).map_err(|e| e.to_string())
    }
}

/// Width clamped to the panel bounds.
pub fn clamp_width(width: f32) -> f32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, section: git::Section) -> git::Commit {
        git::Commit {
            hash: hash.to_owned(),
            short: hash[..7].to_owned(),
            parents: Vec::new(),
            author: "tester".to_owned(),
            subject: format!("commit {hash}"),
            refs: Vec::new(),
            time: 0,
            section,
        }
    }

    /// A status from another repository must not leave the previous
    /// repository's commits, files or open commit on screen.
    #[test]
    fn switching_repositories_drops_the_previous_view() {
        let mut workspace = Workspace {
            root: Some(PathBuf::from("C:/old")),
            log: CommitLog { commits: vec![commit("0123456789", git::Section::History)], upstream: None, truncated: false },
            files: vec!["old.txt".to_owned()],
            file_preview: Some(("old.txt".to_owned(), "text".to_owned(), false)),
            detail: Some(("0123456789".to_owned(), git::CommitDetail { files: Vec::new(), patch: String::new(), header: String::new() })),
            ..Default::default()
        };
        workspace.selected.insert("old.txt".to_owned());

        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        workspace.tx = Some(request_tx);
        workspace.rx = Some(response_rx);
        response_tx.send(Response::Status(Status::default(), Some(PathBuf::from("C:/new")))).unwrap();
        let _ = request_rx.try_iter().count();

        assert!(workspace.absorb(), "a response must ask for a repaint");
        assert!(workspace.log.commits.is_empty(), "the other repository's commits must be dropped");
        assert!(workspace.graph.is_empty() && workspace.files.is_empty() && workspace.detail.is_none());
        assert!(workspace.selected.is_empty() && workspace.file_preview.is_none());
        let requests: Vec<Request> = request_rx.try_iter().collect();
        assert!(requests.iter().any(|request| matches!(request, Request::Log)), "the new repository must be read again");
    }

    fn changed_file(path: &str) -> git::Change {
        git::Change {
            path: path.to_owned(),
            original_path: None,
            index: '.',
            worktree: 'M',
            untracked: false,
            unmerged: false,
            additions: 3,
            deletions: 1,
        }
    }

    fn shape_text(shape: &egui::Shape) -> Option<String> {
        match shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(shape_text),
            _ => None,
        }
    }

    fn panel_frame(workspace: &mut Workspace, tab: PanelTab, pane: crate::layout::split_tree::PaneId, rect: egui::Rect) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        workspace.tab = tab;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 700.0))),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = workspace.show(ui, rect, pane, None);
            });
        });
        output.shapes.into_iter().map(|clipped| clipped.shape).collect()
    }

    /// Two widgets sharing one id make egui paint a red clash overlay over the
    /// panel (and share the widget state behind it).
    #[test]
    fn the_panel_does_not_clash_widget_ids() {
        let commits = vec![commit("0123456789", git::Section::Outgoing), commit("9876543210", git::Section::History)];
        let mut workspace = Workspace {
            status: Status {
                branch: "main".to_owned(),
                upstream: Some("origin/main".to_owned()),
                ahead: 1,
                behind: 0,
                changes: vec![changed_file("src/lib.rs"), changed_file("README.md")],
                additions: 6,
                deletions: 2,
            },
            graph: graph::compute(&commits),
            log: CommitLog { commits, upstream: Some("origin/main".to_owned()), truncated: false },
            ..Default::default()
        };
        for tab in [PanelTab::Changes, PanelTab::Files] {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 700.0));
            let shapes = panel_frame(&mut workspace, tab, 1, rect);
            let clashes: Vec<String> = shapes.iter().filter_map(shape_text).filter(|text| text.contains("of ScrollArea")).collect();
            assert!(clashes.is_empty(), "egui reported an id clash in the {tab:?} tab: {clashes:?}");
        }

    }

    /// Two panes live under one ui, each with its own panel: without the pane
    /// in the id salt both panels share every widget id, egui paints a red
    /// clash overlay over them and their scroll state is shared.
    #[test]
    fn two_panes_do_not_clash_widget_ids() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 700.0))),
            ..Default::default()
        };
        let panel = |path: &str| Workspace {
            status: Status { branch: "main".to_owned(), changes: vec![changed_file(path)], additions: 3, deletions: 1, ..Default::default() },
            ..Default::default()
        };
        let mut left = panel("src/lib.rs");
        let mut right = panel("README.md");
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let left_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(400.0, 700.0));
                let right_rect = egui::Rect::from_min_size(egui::Pos2::new(500.0, 0.0), egui::Vec2::new(380.0, 700.0));
                let _ = left.show(ui, left_rect, 1, None);
                let _ = right.show(ui, right_rect, 2, None);
            });
        });
        let clashes: Vec<String> = output
            .shapes
            .into_iter()
            .filter_map(|clipped| shape_text(&clipped.shape))
            .filter(|text| text.contains("of ScrollArea"))
            .collect();
        assert!(clashes.is_empty(), "two panes shared widget ids: {clashes:?}");
    }
}
