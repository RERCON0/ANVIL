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
/// Files larger than this are skipped by "Посчитать строки".
const MAX_COUNTED_FILE: u64 = 4 * 1024 * 1024;
/// Height of one file-browser row; the list height is a multiple of it.
const FILE_ROW_HEIGHT: f32 = 20.0;
/// Width of the line-number gutter of the diff views.
const GUTTER: f32 = 36.0;
/// The Seti folder glyph (U+E032, "folder" in the font) in the folder blue of
/// the reference file tree.
const FOLDER_ICON: (char, egui::Color32) = ('\u{e032}', egui::Color32::from_rgb(0x7B, 0xB3, 0xD9));

pub enum Request {
    Refresh { cwd: PathBuf },
    /// `side`: Some(true) the index, Some(false) the worktree, None the
    /// index when it has changes for the file, else the worktree.
    Diff { path: String, side: Option<bool> },
    Stage { paths: Vec<String>, staged: bool },
    Commit { message: String },
    AiMessage { command: Option<String> },
    Log,
    CommitDetail { hash: String },
    Files,
    /// Lines of every tracked file of the repository.
    CountLines,
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

impl Request {
    /// Acts on the repository: refused when the worker has moved on to
    /// another repository since the panel showed the one the user meant.
    fn acts_on_repository(&self) -> bool {
        matches!(
            self,
            Request::Stage { .. }
                | Request::Commit { .. }
                | Request::AiMessage { .. }
                | Request::ApplyHunks { .. }
                | Request::Fetch
                | Request::Push
                | Request::WritePath { .. }
                | Request::RenamePath { .. }
                | Request::DeletePath { .. }
        )
    }
}

/// A request plus the repository root the panel showed when it was made.
type Envelope = (Option<PathBuf>, Request);

pub enum Response {
    /// Status plus the resolved repository root (None: not a repository).
    Status(Status, Option<PathBuf>),
    Diff { path: String, files: Vec<FileDiff>, text: String, staged: bool },
    Refreshed,
    Committed(String),
    AiMessage(Result<String, String>),
    Log(CommitLog),
    CommitDetail { hash: String, detail: git::CommitDetail },
    Files(Vec<String>),
    LineCount { files: usize, lines: u64 },
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
    /// The side the user picked with the index/worktree button (None: auto).
    pub diff_side: Option<bool>,
    pub commit_message: String,
    pub notice: Option<(String, bool)>,
    pub busy: bool,
    pub last_poll: Instant,
    pub log: CommitLog,
    pub graph: Vec<graph::Row>,
    pub detail: Option<(String, git::CommitDetail)>,
    pub files: Vec<String>,
    /// Folders of the file tree that the user opened; the tree starts folded.
    pub file_expanded: HashSet<String>,
    /// Files and total lines of the last "Посчитать строки" run.
    pub line_count: Option<(usize, u64)>,
    pub file_filter: String,
    pub file_preview: Option<(String, String, bool)>,
    pub prompt: Option<Prompt>,
    ai_command: Option<String>,
    ai_generating: bool,
    tx: Option<Sender<Envelope>>,
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
            diff_side: None,
            commit_message: String::new(),
            notice: None,
            busy: false,
            last_poll: Instant::now() - POLL_INTERVAL,
            log: CommitLog::default(),
            graph: Vec::new(),
            detail: None,
            files: Vec::new(),
            file_expanded: HashSet::new(),
            line_count: None,
            file_filter: String::new(),
            file_preview: None,
            prompt: None,
            ai_command: None,
            ai_generating: false,
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
        if self.busy || self.ai_generating || self.last_poll.elapsed() < POLL_INTERVAL {
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
            let _ = tx.send((self.root.clone(), request));
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
                        self.line_count = None;
                        self.file_preview = None;
                        self.log = CommitLog::default();
                        self.graph.clear();
                        // A delete/rename prompt names a path of the old one.
                        self.prompt = None;
                        self.diff_side = None;
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
                            self.send(Request::Diff { path, side: self.diff_side });
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
                Response::AiMessage(result) => {
                    self.busy = false;
                    self.ai_generating = false;
                    match result {
                        Ok(message) => self.commit_message = message,
                        Err(message) => self.notice = Some((message, true)),
                    }
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
                Response::LineCount { files, lines } => {
                    self.busy = false;
                    self.line_count = Some((files, lines));
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
        self.busy |= self.ai_generating;
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
            // Reserve the scroll-bar strip instead of letting it float over the
            // rows: file names and hashes were running underneath it.
            ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
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
        // Repo row, commit box and changes stay put; the commit history fills
        // the rest of the column and scrolls on its own (Helm's layout), so
        // scrolling the history never carries the header away.
        self.repo_row(ui);
        self.commit_box(ui);
        self.change_rows(ui);
        ui.add_space(6.0);
        self.commits_section(ui);
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
        let editor = egui::Frame::none()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(4.0, 2.0))
            .show(ui, |ui| {
                ui.style_mut().spacing.scroll.foreground_color = true;
                ScrollArea::vertical()
                    .id_salt("workspace-commit-message")
                    .max_height(120.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_enabled(
                            !self.ai_generating,
                            egui::TextEdit::multiline(&mut self.commit_message)
                                .frame(false)
                                .margin(egui::Margin::same(0.0))
                                .font(theme::field_font(12.0))
                                .desired_rows(6)
                                .hint_text(strings::WORKSPACE_COMMIT_HINT)
                                .desired_width(f32::INFINITY),
                        )
                    })
                    .inner
            });
        let commit = editor.inner;
        let visuals = ui.style().interact(&commit);
        let stroke = if commit.has_focus() { ui.visuals().selection.stroke } else { visuals.bg_stroke };
        ui.painter().rect_stroke(editor.response.rect, visuals.rounding, stroke);
        let ctrl_enter = commit.has_focus() && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        ui.horizontal_wrapped(|ui| {
            let can_commit = !self.ai_generating && !self.commit_message.trim().is_empty() && self.status.changes.iter().any(Change::staged);
            if ui.add_enabled(can_commit, theme::accent_button(strings::WORKSPACE_COMMIT)).clicked() || (can_commit && ctrl_enter) {
                self.busy = true;
                self.notice = None;
                self.send(Request::Commit { message: self.commit_message.clone() });
            }
            let ai_label = match &self.ai_command {
                Some(command) => strings::workspace_ai(command.split_whitespace().next().unwrap_or(command)),
                None => strings::WORKSPACE_AI.to_owned(),
            };
            let ai = ui
                .add_enabled(!self.ai_generating, theme::ghost_button(ai_label))
                .on_hover_text(self.ai_command.as_deref().unwrap_or(strings::WORKSPACE_NO_AI_COMMAND));
            if ai.clicked() {
                self.ai_generating = true;
                self.busy = true;
                self.notice = None;
                self.send(Request::AiMessage { command: self.ai_command.clone() });
                ui.ctx().request_repaint();
            }
        });
        if self.ai_generating {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(theme::ACCENT));
                ui.label(RichText::new(strings::WORKSPACE_AI_GENERATING).color(theme::ACCENT).font(theme::font(11.5)));
            });
        }
        ui.add_space(4.0);
    }

    /// `[ КОММИТЫ ]` header plus the graph list, which fills the rest of the
    /// column and scrolls under the header.
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
        ScrollArea::vertical()
            .id_salt("workspace-commits")
            .auto_shrink([false, false])
            .show(ui, |ui| self.commit_rows(ui));
    }

    fn commit_rows(&mut self, ui: &mut egui::Ui) {
        let mut current_section = None;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        const ROW_HEIGHT: f32 = 22.0;
        // Rows sit edge to edge: each draws its lane segments only inside its
        // own rect, so any item spacing between rows is a gap in every lane.
        // The section labels keep the spacing they had around them.
        let gap = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        // Read in place (the log is not cloned every frame); a click is
        // handled after the loop.
        let mut opened: Option<String> = None;
        for (index, commit) in self.log.commits.iter().enumerate() {
            if current_section != Some(commit.section) {
                current_section = Some(commit.section);
                let label = match commit.section {
                    git::Section::Outgoing => strings::WORKSPACE_SECTION_OUTGOING,
                    git::Section::Incoming => strings::WORKSPACE_SECTION_INCOMING,
                    git::Section::History => strings::WORKSPACE_SECTION_HISTORY,
                };
                ui.add_space(if index > 0 { gap + 4.0 } else { 4.0 });
                ui.label(RichText::new(label).color(theme::FAINT).font(theme::font(10.5)));
                ui.add_space(gap);
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
                opened = Some(commit.hash.clone());
            }
        }
        if let Some(hash) = opened {
            self.busy = true;
            self.send(Request::CommitDetail { hash });
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
        // Built once per frame and handed to the drawing closure.
        let mut rows = Some(self.rows());
        let count = rows.as_ref().map_or(0, Vec::len);
        // A long tree gets its own bounded scroll, short lists stay inline. The
        // bound is a share of the column, so the commit history below always
        // keeps room for its own scroll.
        let limit = (ui.available_height() * 0.45).clamp(ROW_HEIGHT * 3.0, 300.0);
        let bounded = |ui: &mut egui::Ui, body: &mut dyn FnMut(&mut egui::Ui)| {
            if count as f32 * (ROW_HEIGHT + ui.spacing().item_spacing.y) > limit {
                ScrollArea::vertical().id_salt("workspace-changes").max_height(limit).auto_shrink([false, false]).show(ui, body);
            } else {
                body(ui);
            }
        };
        {
            bounded(ui, &mut |ui: &mut egui::Ui| {
                for row in rows.take().unwrap_or_default() {
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
                                    self.diff_side = None;
                                    self.busy = true;
                                    self.send(Request::Diff { path, side: None });
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
                    self.diff_side = Some(!self.diff_from_index);
                    self.busy = true;
                    self.send(Request::Diff { path: path.clone(), side: self.diff_side });
                }
            });
        });
        let mut hunks_to_apply: Option<(usize, String, bool)> = None;
        {
            let ui = &mut *ui;
            let mut hunk_index = 0;
            let mut numbers = PatchNumbers::default();
            for line in self.diff_text.lines() {
                let number = numbers.line(line);
                if line.starts_with("@@") {
                    let label = if self.diff_from_index { "◂" } else { "▸" };
                    let header = line.to_owned();
                    ui.horizontal(|ui| {
                        if ui.add(theme::ghost_button(label)).clicked() {
                            hunks_to_apply = Some((hunk_index, header.clone(), self.diff_from_index));
                        }
                        ui.label(RichText::new(line).color(theme::DIFF_HUNK).font(theme::field_font(11.0)));
                    });
                    hunk_index += 1;
                    continue;
                }
                patch_line(ui, line, number);
            }
        }
        if let Some((index, header, from_index)) = hunks_to_apply {
            self.busy = true;
            self.send(Request::ApplyHunks { path, index, header, from_index });
        }
    }

    /// Commit detail replaces the column while open.
    fn detail_view(&mut self, ui: &mut egui::Ui) -> bool {
        // Taken out and put back rather than cloned: the patch can be 8 MB.
        let Some((hash, detail)) = self.detail.take() else { return false };
        let back = commit_detail_view(ui, &hash, &detail);
        if !back {
            self.detail = Some((hash, detail));
        }
        true
    }
}

/// The open commit; true when "back" was pressed.
fn commit_detail_view(ui: &mut egui::Ui, hash: &str, detail: &git::CommitDetail) -> bool {
    let mut back = false;
    {
        ui.horizontal(|ui| {
            if ui.add(theme::ghost_button(strings::WORKSPACE_BACK)).clicked() {
                back = true;
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
        let body: String = display_multiline(&lines.collect::<Vec<_>>().join("\n"), 2000);
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
            let mut numbers = PatchNumbers::default();
            for line in detail.patch.lines() {
                let number = numbers.line(line);
                patch_line(ui, line, number);
            }
        });
    }
    back
}

impl Workspace {
    // ---- files -----------------------------------------------------------

    /// Rows of the file tree, honouring the expanded folders (the tree starts
    /// folded, so only the top level is visible until a folder is opened). The
    /// file list is sorted, so a folder starts a row exactly when it is not
    /// shared with the previous path.
    fn file_tree(&self) -> Vec<FileRow> {
        let mut rows = Vec::new();
        let mut previous: Vec<String> = Vec::new();
        for path in &self.files {
            let segments: Vec<&str> = path.split('/').collect();
            let (dirs, name) = segments.split_at(segments.len() - 1);
            let folders: Vec<String> = (0..dirs.len()).map(|index| dirs[..=index].join("/")).collect();
            let shared = folders.iter().zip(previous.iter()).take_while(|(current, seen)| current == seen).count();
            // A folder that is not expanded hides the whole subtree below it.
            let mut hidden = folders[..shared].iter().any(|folder| !self.file_expanded.contains(folder));
            for (index, (segment, folder)) in dirs.iter().zip(folders.iter()).enumerate().skip(shared) {
                if !hidden {
                    rows.push(FileRow { depth: index, name: (*segment).to_owned(), dir: None, path: folder.clone(), folder: true });
                }
                if !self.file_expanded.contains(folder) {
                    hidden = true;
                }
            }
            if !hidden && !name.is_empty() {
                rows.push(FileRow { depth: dirs.len(), name: name[0].to_owned(), dir: None, path: path.clone(), folder: false });
            }
            previous = folders;
        }
        rows
    }

    /// One row of the file browser; returns the path to open when a file was
    /// clicked. Folders fold instead.
    fn file_row(&mut self, ui: &mut egui::Ui, row: &FileRow) -> Option<String> {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, FILE_ROW_HEIGHT), Sense::click());
        let painter = ui.painter_at(rect);
        if response.hovered() {
            painter.rect_filled(rect, 0.0, theme::TAB_HOVER_BG);
        }
        let indent = row.depth as f32 * 12.0;
        let mut opened = None;
        if row.folder {
            let open = self.file_expanded.contains(&row.path);
            // The chevron is the affordance for folding: big and bright enough
            // to read at a glance (the whole row toggles).
            painter.text(
                egui::Pos2::new(rect.min.x + 5.0 + indent, rect.center().y),
                Align2::LEFT_CENTER,
                if open { "▾" } else { "▸" },
                theme::font(12.5),
                theme::DIM,
            );
            painter.text(
                egui::Pos2::new(rect.min.x + 18.0 + indent, rect.center().y),
                Align2::LEFT_CENTER,
                FOLDER_ICON.0,
                theme::icon_font(13.0),
                FOLDER_ICON.1,
            );
            let name = elide(&painter, &row.name, theme::font(11.5), (rect.width() - indent - 38.0).max(24.0));
            painter.text(
                egui::Pos2::new(rect.min.x + 34.0 + indent, rect.center().y),
                Align2::LEFT_CENTER,
                name,
                theme::font(11.5),
                theme::TEXT,
            );
            if response.clicked() {
                if self.file_expanded.contains(&row.path) {
                    self.file_expanded.remove(&row.path);
                } else {
                    self.file_expanded.insert(row.path.clone());
                }
            }
        } else {
            let (icon, icon_color) = crate::file_icons::for_file(&row.name);
            painter.text(
                egui::Pos2::new(rect.min.x + 4.0 + indent, rect.center().y),
                Align2::LEFT_CENTER,
                icon,
                theme::icon_font(13.0),
                icon_color,
            );
            // Long paths must not run past the row: keep the file name whole
            // and elide the folders in front of it.
            let mut x = rect.min.x + 20.0 + indent;
            let limit = (rect.width() - indent - 24.0).max(24.0);
            let name = elide(&painter, &row.name, theme::font(11.5), limit);
            let name_width = painter.layout_no_wrap(name.clone(), theme::font(11.5), theme::TEXT).size().x;
            if let Some(dir) = &row.dir {
                let dir_limit = (limit - name_width).max(0.0);
                if dir_limit > 10.0 {
                    let dir = elide_front(&painter, dir, theme::font(11.0), dir_limit);
                    let galley = painter.layout_no_wrap(dir, theme::font(11.0), theme::FAINT);
                    painter.galley(egui::Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley.clone(), theme::FAINT);
                    x += galley.size().x;
                }
            }
            painter.text(egui::Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, name, theme::font(11.5), theme::TEXT);
            if response.clicked() {
                opened = Some(row.path.clone());
            }
        }
        let path = row.path.clone();
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
        opened
    }

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
            if ui.add(theme::ghost_button(strings::WORKSPACE_COUNT_LINES)).clicked() {
                self.busy = true;
                self.send(Request::CountLines);
            }
            if let Some((files, lines)) = self.line_count {
                ui.label(RichText::new(strings::workspace_line_count(files, lines)).color(theme::ACCENT).font(theme::font(11.0)));
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.file_filter)
                .font(theme::field_font(12.0))
                .hint_text(strings::WORKSPACE_FILE_FILTER)
                .desired_width(f32::INFINITY),
        );
        let searching = !self.file_filter.trim().is_empty();
        // A search lists the ranked hits flat; an empty filter shows the tree.
        let rows: Vec<FileRow> = if searching {
            let mut matches: Vec<(i32, &String)> = self
                .files
                .iter()
                .filter_map(|path| crate::profiles::fuzzy_score(&self.file_filter, path).map(|score| (score, path)))
                .collect();
            matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            matches
                .iter()
                .take(400)
                .map(|(_, path)| {
                    let (dir, name) = match path.rfind('/') {
                        Some(index) => (&path[..index + 1], &path[index + 1..]),
                        None => ("", path.as_str()),
                    };
                    FileRow { depth: 0, name: display(name, 120), dir: Some(display(dir, 120)), path: (*path).clone(), folder: false }
                })
                .collect()
        } else {
            self.file_tree()
        };
        // The list fills the panel when no preview is open, and always ends on
        // a whole row: a height that is not a multiple of the row height cut
        // the last row in half.
        let room = if self.file_preview.is_some() { ui.available_height() * 0.45 } else { ui.available_height() };
        let list_height = (room / FILE_ROW_HEIGHT).floor().max(1.0) * FILE_ROW_HEIGHT;
        let mut open_file: Option<String> = None;
        ScrollArea::vertical().id_salt("workspace-files").max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows {
                if let Some(path) = self.file_row(ui, row) {
                    open_file = Some(path);
                }
            }
        });
        if let Some(path) = open_file {
            self.busy = true;
            self.send(Request::ReadFile { path });
        }
        if let Some((path, text, truncated)) = self.file_preview.as_ref() {
            ui.add_space(2.0);
            theme::hairline(ui);
            ui.label(RichText::new(path).color(theme::DIM).font(theme::field_font(11.0)));
            // The preview ends on a whole line and takes the space the list
            // leaves: a height that is not a multiple of the line height cut
            // the last line in half, and the old half-of-the-rest cap left the
            // panel empty below (minus a hair, so no sliver shows).
            let line = ui.fonts(|f| f.row_height(&theme::field_font(11.0)));
            let room = (ui.available_height() - 6.0).max(line);
            let height = (room / line).floor().max(1.0) * line - 1.0;
            let markdown = path.ends_with(".md") || path.ends_with(".markdown");
            ScrollArea::vertical().id_salt("workspace-file-preview").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                if markdown {
                    markdown_view(ui, text);
                } else {
                    code_view(ui, text);
                }
                if *truncated {
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

/// One row of the file browser: a folder (click folds it) or a file. `dir` is
/// the leading path shown when the rows come from a search, not from a tree.
struct FileRow {
    depth: usize,
    name: String,
    dir: Option<String>,
    path: String,
    folder: bool,
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

/// File text with a line-number gutter; long lines wrap inside the panel.
fn code_view(ui: &mut egui::Ui, text: &str) {
    ui.spacing_mut().item_spacing.y = 0.0;
    // Painted by hand, like the diff: a label in a grid cell does not wrap, so
    // long lines ran off the panel edge.
    for (index, line) in text.lines().enumerate() {
        let width = ui.available_width();
        let galley = ui.painter().layout(line.to_owned(), theme::field_font(11.0), theme::DIM, (width - GUTTER - 4.0).max(40.0));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, galley.size().y), Sense::hover());
        ui.painter().text(
            egui::Pos2::new(rect.min.x + GUTTER - 8.0, rect.min.y + 1.0),
            Align2::RIGHT_TOP,
            (index + 1).to_string(),
            theme::field_font(10.5),
            theme::FAINT,
        );
        ui.painter().galley(egui::Pos2::new(rect.min.x + GUTTER, rect.min.y), galley, theme::DIM);
    }
}

/// A small Markdown view: headings, lists, quotes, code fences, rules and
/// tables (the tables are the point — a `.md` used to show its pipes). Inline
/// marks are left as they are.
fn markdown_view(ui: &mut egui::Ui, text: &str) {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let plain = line.trim();
        if plain.is_empty() {
            ui.add_space(6.0);
            continue;
        }
        if plain.starts_with("```") {
            let mut code = Vec::new();
            for next in lines.by_ref() {
                if next.trim_start().starts_with("```") {
                    break;
                }
                code.push(next.to_owned());
            }
            for line in &code {
                ui.label(RichText::new(line).color(theme::DIM).font(theme::field_font(11.0)));
            }
            continue;
        }
        if plain.starts_with('#') {
            let level = plain.chars().take_while(|c| *c == '#').count().min(6);
            let title = plain[level..].trim().trim_end_matches('#').trim();
            let size = match level {
                1 => 15.0,
                2 => 13.5,
                _ => 12.5,
            };
            ui.add_space(6.0);
            ui.label(RichText::new(title).color(theme::TEXT).font(theme::title_font(size)));
            ui.add_space(2.0);
            continue;
        }
        if plain == "---" || plain == "***" || plain == "___" {
            theme::hairline(ui);
            ui.add_space(4.0);
            continue;
        }
        if is_table_row(plain) && lines.peek().is_some_and(|next| is_table_separator(next)) {
            let mut rows = vec![table_cells(plain)];
            lines.next();
            while let Some(next) = lines.peek() {
                if !is_table_row(next) {
                    break;
                }
                let row = lines.next().unwrap_or_default();
                rows.push(table_cells(row));
            }
            table_view(ui, &rows);
            ui.add_space(6.0);
            continue;
        }
        if let Some(rest) = plain.strip_prefix("> ") {
            let width = ui.available_width();
            ui.label(inline_job(&format!("│ {rest}"), 11.5, theme::FAINT, width));
            continue;
        }
        let width = ui.available_width();
        let bullet = plain.strip_prefix("- ").or_else(|| plain.strip_prefix("* ")).or_else(|| plain.strip_prefix("+ "));
        match bullet {
            Some(rest) => ui.label(inline_job(&format!("• {rest}"), 11.5, theme::DIM, width)),
            None => ui.label(inline_job(plain, 11.5, theme::DIM, width)),
        };
    }
}

/// Inline Markdown of the panel: `code` spans get a chip, **bold** a brighter
/// tone. Anything else is left as written.
fn inline_job(text: &str, size: f32, colour: egui::Color32, width: f32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let plain = TextFormat { font_id: theme::font(size), color: colour, ..Default::default() };
    let bold = TextFormat { font_id: theme::font(size), color: theme::TEXT, extra_letter_spacing: 0.2, ..Default::default() };
    let code = TextFormat { font_id: theme::field_font(size - 0.5), color: theme::TEXT, background: theme::TAB_ACTIVE_BG, ..Default::default() };
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let mut rest = text;
    while !rest.is_empty() {
        let bold_at = rest.find("**").map(|at| (at, "**", &bold));
        let code_at = rest.find('`').map(|at| (at, "`", &code));
        let Some((at, marker, format)) = [bold_at, code_at].into_iter().flatten().min_by_key(|(at, _, _)| *at) else {
            job.append(rest, 0.0, plain);
            break;
        };
        if at > 0 {
            job.append(&rest[..at], 0.0, plain.clone());
        }
        let after = &rest[at + marker.len()..];
        match after.find(marker) {
            Some(end) => {
                job.append(&after[..end], 0.0, format.clone());
                rest = &after[end + marker.len()..];
            }
            None => {
                job.append(&rest[at..], 0.0, plain);
                break;
            }
        }
    }
    job
}

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() > 2
}

fn is_table_separator(line: &str) -> bool {
    is_table_row(line)
        && table_cells(line).iter().all(|cell| {
            let cell = cell.trim_matches(':');
            !cell.is_empty() && cell.chars().all(|c| c == '-')
        })
}

fn table_cells(line: &str) -> Vec<String> {
    line.trim().trim_matches('|').split('|').map(|cell| cell.trim().to_owned()).collect()
}

/// Table cells are painted by hand so every column wraps inside the panel; a
/// grid of labels would run past its edge.
fn table_view(ui: &mut egui::Ui, rows: &[Vec<String>]) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let width = ui.available_width();
    let gap = 10.0;
    let column = ((width - gap * (columns - 1) as f32) / columns as f32).max(40.0);
    for (index, row) in rows.iter().enumerate() {
        let colour = if index == 0 { theme::TEXT } else { theme::DIM };
        let galleys: Vec<_> = (0..columns)
            .map(|cell| {
                let text = row.get(cell).cloned().unwrap_or_default();
                ui.painter().layout_job(inline_job(&text, 11.5, colour, column))
            })
            .collect();
        let height = galleys.iter().map(|galley| galley.size().y).fold(0.0, f32::max).max(14.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        if index == 0 {
            ui.painter().rect_filled(rect, 0.0, theme::TAB_ACTIVE_BG);
        }
        for (cell, galley) in galleys.into_iter().enumerate() {
            let x = rect.min.x + cell as f32 * (column + gap);
            ui.painter().galley(egui::Pos2::new(x, rect.min.y), galley, colour);
        }
        theme::hairline(ui);
    }
}

/// One patch line: a right-aligned file line number in the gutter, then the
/// patch line itself, with the change band behind both — as in the reference
/// diff view. Added and removed lines get a full-width tinted band with the
/// line colour on top; colouring only the text is easy to miss.
fn patch_line(ui: &mut egui::Ui, line: &str, number: Option<u64>) {
    // The bands must tile: any item spacing would show as a gap between lines.
    ui.spacing_mut().item_spacing.y = 0.0;
    let (band, colour) = if line.starts_with('+') && !line.starts_with("+++") {
        (Some(theme::DIFF_ADD_BG), theme::STATUS_GREEN)
    } else if line.starts_with('-') && !line.starts_with("---") {
        (Some(theme::DIFF_REMOVE_BG), theme::STATUS_RED)
    } else if line.starts_with("@@") {
        (None, theme::DIFF_HUNK)
    } else {
        (None, theme::DIM)
    };
    let width = ui.available_width();
    let galley = ui.painter().layout(line.to_owned(), theme::field_font(11.0), colour, (width - GUTTER - 4.0).max(40.0));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, galley.size().y), Sense::hover());
    if let Some(band) = band {
        // Half-pixel bleed: two bands sharing a fractional edge would blend
        // with the background where they meet and read as a torn line.
        ui.painter().rect_filled(rect.expand2(Vec2::new(0.0, 0.5)), 0.0, band);
    }
    if let Some(number) = number {
        ui.painter().text(
            egui::Pos2::new(rect.min.x + GUTTER - 8.0, rect.min.y + 1.0),
            Align2::RIGHT_TOP,
            number.to_string(),
            theme::field_font(10.5),
            theme::FAINT,
        );
    }
    ui.painter().galley(egui::Pos2::new(rect.min.x + GUTTER, rect.min.y), galley, colour);
}

/// File line numbers of a patch: the old number for removals, the new one for
/// additions and context, reset by every `@@` header.
#[derive(Default)]
struct PatchNumbers {
    old: u64,
    new: u64,
    started: bool,
}

impl PatchNumbers {
    fn line(&mut self, line: &str) -> Option<u64> {
        if line.starts_with("@@") {
            if let Some((old, new)) = parse_hunk(line) {
                self.old = old;
                self.new = new;
                self.started = true;
            }
            return None;
        }
        let header = line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("---")
            || line.starts_with("+++")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("similarity")
            || line.starts_with("rename ")
            || line.starts_with("copy ")
            || line.starts_with("\\ No newline");
        if header || !self.started {
            return None;
        }
        if line.starts_with('+') {
            let number = self.new;
            self.new += 1;
            return Some(number);
        }
        if line.starts_with('-') {
            let number = self.old;
            self.old += 1;
            return Some(number);
        }
        let number = self.new;
        self.old += 1;
        self.new += 1;
        Some(number)
    }
}

/// `@@ -1494,6 +1481,34 @@ impl AnvilApp {` -> (1494, 1481).
fn parse_hunk(line: &str) -> Option<(u64, u64)> {
    let mut parts = line.strip_prefix("@@")?.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let number = |text: &str| text.split(',').next()?.parse::<u64>().ok();
    Some((number(old)?, number(new)?))
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

/// Like `display`, but keeps line breaks: commit bodies and patches are prose
/// whose line structure carries meaning.
fn display_multiline(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max_chars * 4));
    let mut count = 0;
    for ch in text.chars() {
        if ch == '\n' {
            out.push('\n');
            continue;
        }
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

/// Elides the *front* of a path so the folder closest to the file stays
/// readable: `…/egui-default-fonts/`.
fn elide_front(painter: &egui::Painter, text: &str, font: egui::FontId, max_width: f32) -> String {
    let measure = |value: &str| painter.layout_no_wrap(value.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
    if measure(text) <= max_width {
        return text.to_owned();
    }
    let mut tail = String::new();
    for ch in text.chars().rev() {
        tail.insert(0, ch);
        if measure(&format!("…{tail}")) > max_width {
            tail.remove(0);
            break;
        }
    }
    format!("…{tail}")
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

/// Opens a repository file with its associated application. Programs,
/// scripts and shortcuts are shown in Explorer instead: the default verb of
/// a `.js`, `.bat` or `.lnk` from a cloned repository runs it.
fn open_external(root: &Option<PathBuf>, path: &str) {
    if runs_when_opened(path) {
        reveal_in_explorer(root, path);
    } else if let Some(root) = root {
        crate::settings_ui::open_path(&root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR)));
    }
}

/// True when the shell's default action for `path` executes it.
fn runs_when_opened(path: &str) -> bool {
    const RUNNABLE: &[&str] = &[
        "exe", "com", "bat", "cmd", "ps1", "psm1", "psd1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "ws", "hta", "msi",
        "msp", "msc", "lnk", "url", "scr", "pif", "cpl", "reg", "jar", "py", "pyw", "appref-ms", "application", "gadget",
        "inf", "scf", "chm", "settingcontent-ms", "library-ms", "search-ms",
    ];
    let Some((_, extension)) = path.rsplit_once('.') else { return false };
    let extension = extension.to_ascii_lowercase();
    if extension.contains(['/', '\\']) {
        return false;
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_default().to_ascii_lowercase();
    RUNNABLE.contains(&extension.as_str()) || pathext.split(';').any(|known| known.trim_start_matches('.') == extension)
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

fn spawn_worker() -> (Sender<Envelope>, Receiver<Response>) {
    let (request_tx, request_rx) = mpsc::channel::<Envelope>();
    let (response_tx, response_rx) = mpsc::channel::<Response>();
    std::thread::spawn(move || {
        let mut root: Option<PathBuf> = None;
        let mut cwd: Option<PathBuf> = None;
        while let Ok((expected_root, request)) = request_rx.recv() {
            let send = |response: Response| {
                let _ = response_tx.send(response);
            };
            // Paths are resolved against the repository refreshed last: an
            // action the user meant for another one must not run here.
            if request.acts_on_repository() && expected_root != root {
                let error = strings::WORKSPACE_REPO_CHANGED.to_owned();
                send(if matches!(request, Request::AiMessage { .. }) { Response::AiMessage(Err(error)) } else { Response::Error(error) });
                continue;
            }
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
                Request::Diff { path, side } => match &root {
                    Some(root) => {
                        let (bytes, from_index) = match side {
                            Some(staged) => (git::diff_bytes(root, &path, staged), staged),
                            None => {
                                let staged = git::diff_bytes(root, &path, true);
                                if staged.trim_ascii().is_empty() {
                                    (git::diff_bytes(root, &path, false), false)
                                } else {
                                    (staged, true)
                                }
                            }
                        };
                        let files = git::parse_diff(&bytes, from_index);
                        let text = String::from_utf8_lossy(&bytes).into_owned();
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
                        let diff_args = ["--no-ext-diff", "--no-textconv", "--no-color", "--unified=1"];
                        let staged = git::run_git(root, &[&["diff", "--cached"][..], &diff_args].concat()).unwrap_or_default();
                        let unstaged = git::run_git(root, &[&["diff"][..], &diff_args].concat()).unwrap_or_default();
                        let status = git::run_git(root, &["status", "--porcelain"]).unwrap_or_default();
                        let recent = git::recent_subjects(root, 8);
                        let diff = format!("{staged}\n{unstaged}");
                        let prompt = git::ai_prompt(&status, diff.trim(), &recent);
                        git::ai_commit_message(&git::ai_workdir(), command.as_deref(), &prompt, git::AI_TIMEOUT)
                    });
                    send(Response::AiMessage(result.unwrap_or_else(|| Err(strings::WORKSPACE_NO_REPO.to_owned()))));
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
                Request::CountLines => match &root {
                    Some(root) => match git::ls_files(root) {
                        Ok(files) => {
                            let mut lines = 0u64;
                            let mut counted = 0usize;
                            for path in &files {
                                let Ok(full) = git::resolve_path(root, path) else { continue };
                                let Ok(meta) = std::fs::metadata(&full) else { continue };
                                // Huge and binary files would only slow the walk down.
                                if !meta.is_file() || meta.len() > MAX_COUNTED_FILE {
                                    continue;
                                }
                                let Ok(bytes) = std::fs::read(&full) else { continue };
                                if bytes.iter().take(8192).any(|byte| *byte == 0) {
                                    continue;
                                }
                                lines += bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
                                counted += 1;
                            }
                            send(Response::LineCount { files: counted, lines });
                        }
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
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
                        let files = git::parse_diff(git::diff_bytes(root, &path, from_index), from_index);
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
    // std::fs::rename replaces an existing target on Windows. A target that
    // resolves to the source itself is a case-only rename and is allowed.
    if to.symlink_metadata().is_ok() {
        let same = matches!((from.canonicalize(), to.canonicalize()), (Ok(a), Ok(b)) if a == b);
        if !same {
            return Err(strings::WORKSPACE_FILE_EXISTS.to_owned());
        }
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
        workspace.prompt = Some(Prompt { kind: PromptKind::Delete("src".to_owned()), text: String::new(), focus: false });

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
        assert!(workspace.prompt.is_none(), "a delete/rename prompt of the old repository must not survive");
        let requests: Vec<Request> = request_rx.try_iter().map(|(_, request)| request).collect();
        assert!(requests.iter().any(|request| matches!(request, Request::Log)), "the new repository must be read again");
    }

    fn git(dir: &Path, args: &[&str]) -> bool {
        let mut command = std::process::Command::new("git");
        command.args(args).current_dir(dir);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command.output().is_ok_and(|out| out.status.success())
    }

    /// The "index / worktree" button asked for the same automatic side again,
    /// so the worktree hunks of a partly staged file could never be shown.
    #[test]
    fn the_diff_side_can_be_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !git(root, &["init", "--quiet"]) {
            eprintln!("git is not installed; skipping");
            return;
        }
        assert!(git(root, &["config", "user.email", "anvil@test"]) && git(root, &["config", "user.name", "ANVIL test"]));
        std::fs::write(root.join("f.txt"), "one\n").unwrap();
        assert!(git(root, &["add", "f.txt"]) && git(root, &["commit", "--quiet", "-m", "init"]));
        std::fs::write(root.join("f.txt"), "one\nstaged\n").unwrap();
        assert!(git(root, &["add", "f.txt"]));
        std::fs::write(root.join("f.txt"), "one\nstaged\nworktree\n").unwrap();

        let (tx, rx) = spawn_worker();
        let wait = || rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a worker response");
        tx.send((None, Request::Refresh { cwd: root.to_path_buf() })).unwrap();
        let Response::Status(_, shown) = wait() else { panic!("status first") };
        let diff = |side: Option<bool>| {
            tx.send((shown.clone(), Request::Diff { path: "f.txt".to_owned(), side })).unwrap();
            match wait() {
                Response::Diff { text, staged, .. } => (text, staged),
                _ => panic!("a diff"),
            }
        };
        let (text, staged) = diff(None);
        assert!(staged && text.contains("+staged"), "the index side first: {text}");
        let (text, staged) = diff(Some(false));
        assert!(!staged && text.contains("+worktree") && !text.contains("+staged"), "the worktree side on request: {text}");
    }

    /// The worker resolves paths against the repository it last refreshed. A
    /// delete confirmed for one repository after the shell had moved to
    /// another one used to run there; it must be refused instead.
    #[test]
    fn actions_meant_for_another_repository_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut init = std::process::Command::new("git");
        init.args(["init", "--quiet"]).current_dir(dir.path());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            init.creation_flags(0x0800_0000);
        }
        if !init.status().is_ok_and(|status| status.success()) {
            eprintln!("git is not installed; skipping");
            return;
        }
        std::fs::write(dir.path().join("keep.txt"), "x").unwrap();
        let (tx, rx) = spawn_worker();
        let wait = || rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a worker response");
        tx.send((None, Request::Refresh { cwd: dir.path().to_path_buf() })).unwrap();
        let Response::Status(_, Some(_)) = wait() else { panic!("the folder is a repository") };

        let elsewhere = Some(PathBuf::from("C:/some/other/repo"));
        tx.send((elsewhere, Request::DeletePath { path: "keep.txt".to_owned() })).unwrap();
        match wait() {
            Response::Error(message) => assert_eq!(message, strings::WORKSPACE_REPO_CHANGED),
            _ => panic!("the delete must be refused"),
        }
        assert!(dir.path().join("keep.txt").exists(), "nothing was deleted");
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
        panel_frame_clipped(workspace, tab, pane, rect).into_iter().map(|clipped| clipped.shape).collect()
    }

    fn panel_frame_clipped(
        workspace: &mut Workspace,
        tab: PanelTab,
        pane: crate::layout::split_tree::PaneId,
        rect: egui::Rect,
    ) -> Vec<egui::epaint::ClippedShape> {
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
        output.shapes
    }

    /// Helm's layout: the history scrolls in its own viewport under the
    /// `[ КОММИТЫ ]` header, so the repo row and the commit box never scroll
    /// away with it.
    #[test]
    fn scrolling_the_history_keeps_the_header_in_place() {
        let commits: Vec<git::Commit> = (0..200).map(|i| commit(&format!("{i:010}"), git::Section::History)).collect();
        let mut workspace = Workspace {
            status: Status { branch: "main".to_owned(), changes: vec![changed_file("src/lib.rs")], ..Default::default() },
            graph: graph::compute(&commits),
            log: CommitLog { commits, upstream: None, truncated: false },
            tab: PanelTab::Changes,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(600.0, 700.0));
        let commit_button = strings::WORKSPACE_COMMIT.to_owned();
        let mut frame = |events: Vec<egui::Event>, time: f64| {
            let input = egui::RawInput { screen_rect: Some(rect), time: Some(time), events, ..Default::default() };
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = workspace.show(ui, rect, 1, None);
                });
            });
            output.shapes.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == commit_button => Some(text.pos.y),
                _ => None,
            })
        };
        let before = frame(Vec::new(), 0.0).expect("the commit button is drawn");
        let over_history = egui::Pos2::new(300.0, 650.0);
        let wheel = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::Vec2::new(0.0, -400.0), modifiers: Default::default() };
        let mut after = Some(before);
        for step in 1..30 {
            let events = if step == 1 { vec![egui::Event::PointerMoved(over_history), wheel.clone()] } else { Vec::new() };
            after = frame(events, step as f64 * 0.05);
        }
        assert_eq!(after, Some(before), "scrolling the history moved the commit box");
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

    /// "Open in the system" runs ShellExecute's default verb: for a script or
    /// a shortcut from a cloned repository that is running it, not viewing it.
    #[test]
    fn programs_and_scripts_are_not_opened() {
        for path in ["setup.exe", "run.BAT", "a/b/tool.cmd", "x.ps1", "x.js", "x.vbs", "x.wsf", "x.hta", "x.lnk", "x.url", "x.py", "x.reg", "x.msi"] {
            assert!(runs_when_opened(path), "{path}");
        }
        for path in ["README.md", "src/main.rs", "image.png", "notes.txt", "Makefile", "data.json"] {
            assert!(!runs_when_opened(path), "{path}");
        }
    }

    /// std::fs::rename replaces an existing target on Windows: renaming onto
    /// another file used to destroy it. A case-only rename is still allowed.
    #[test]
    fn renaming_never_overwrites_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        assert_eq!(rename_path(root, "a.txt", "b.txt"), Err(strings::WORKSPACE_FILE_EXISTS.to_owned()));
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "b", "the target is untouched");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "a");
        rename_path(root, "a.txt", "A.txt").expect("a case-only rename");
        let names: Vec<String> =
            std::fs::read_dir(root).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"A.txt".to_owned()), "{names:?}");
        rename_path(root, "A.txt", "c.txt").expect("a plain rename");
        assert_eq!(std::fs::read_to_string(root.join("c.txt")).unwrap(), "a");
    }

    /// The lane of a linear history is one unbroken line: rows sit edge to edge,
    /// with no item spacing between them for the line to skip.
    #[test]
    fn the_commit_graph_line_has_no_gaps_between_rows() {
        let hashes = ["3333333333", "2222222222", "1111111111", "0000000000"];
        let commits: Vec<git::Commit> = hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| git::Commit {
                parents: hashes.get(i + 1).map(|p| vec![(*p).to_owned()]).unwrap_or_default(),
                ..commit(hash, git::Section::History)
            })
            .collect();
        let mut workspace = Workspace {
            status: Status { branch: "main".to_owned(), ..Default::default() },
            graph: graph::compute(&commits),
            log: CommitLog { commits, upstream: None, truncated: false },
            ..Default::default()
        };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(600.0, 700.0));
        let shapes = panel_frame(&mut workspace, PanelTab::Changes, 1, rect);
        let colour = egui::epaint::ColorMode::Solid(section_color(git::Section::History));
        let mut spans: Vec<(f32, f32)> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::LineSegment { points, stroke } if stroke.color == colour && points[0].x == points[1].x => {
                    Some((points[0].y.min(points[1].y), points[0].y.max(points[1].y)))
                }
                _ => None,
            })
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(spans.len(), 7, "up+down for every row but the root, which only reaches up: {spans:?}");
        for pair in spans.windows(2) {
            assert!(pair[1].0 <= pair[0].1 + 0.01, "gap in the lane between {:?} and {:?}", pair[0], pair[1]);
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

    #[test]
    fn patch_numbers_follow_the_hunks() {
        let mut numbers = PatchNumbers::default();
        let seen: Vec<Option<u64>> = [
            "diff --git a/x b/x",
            "index 111..222 100644",
            "--- a/x",
            "+++ b/x",
            "@@ -10,3 +10,4 @@ fn main() {",
            " let a = 1;",
            "-let b = 2;",
            "+let b = 3;",
            "+let c = 4;",
        ]
        .iter()
        .map(|line| numbers.line(line))
        .collect();
        assert_eq!(
            seen,
            vec![None, None, None, None, None, Some(10), Some(11), Some(11), Some(12)],
            "context takes the new number, removals the old one"
        );
    }

    #[test]
    fn the_file_tree_starts_folded_and_follows_expanded_folders() {
        let mut workspace = Workspace {
            files: vec![
                "Cargo.toml".to_owned(),
                "src/app.rs".to_owned(),
                "src/term/render.rs".to_owned(),
                "tests/git.rs".to_owned(),
            ],
            ..Default::default()
        };
        let shape = |rows: Vec<FileRow>| rows.iter().map(|row| (row.depth, row.name.clone(), row.folder)).collect::<Vec<_>>();
        // The tree starts folded: only the top level shows.
        assert_eq!(
            shape(workspace.file_tree()),
            vec![(0, "Cargo.toml".to_owned(), false), (0, "src".to_owned(), true), (0, "tests".to_owned(), true)]
        );
        // Opening `src` reveals its own entries, the folders inside it stay folded.
        workspace.file_expanded.insert("src".to_owned());
        assert_eq!(
            shape(workspace.file_tree()),
            vec![
                (0, "Cargo.toml".to_owned(), false),
                (0, "src".to_owned(), true),
                (1, "app.rs".to_owned(), false),
                (1, "term".to_owned(), true),
                (0, "tests".to_owned(), true),
            ]
        );
        workspace.file_expanded.insert("src/term".to_owned());
        assert_eq!(
            shape(workspace.file_tree()),
            vec![
                (0, "Cargo.toml".to_owned(), false),
                (0, "src".to_owned(), true),
                (1, "app.rs".to_owned(), false),
                (1, "term".to_owned(), true),
                (2, "render.rs".to_owned(), false),
                (0, "tests".to_owned(), true),
            ]
        );
    }

    #[test]
    fn ai_generation_survives_unrelated_responses_until_its_result() {
        let (tx, rx) = mpsc::channel();
        let mut workspace = Workspace { rx: Some(rx), ai_generating: true, busy: true, ..Default::default() };
        tx.send(Response::Log(CommitLog::default())).unwrap();
        tx.send(Response::Error("unrelated git error".to_owned())).unwrap();
        workspace.absorb();
        assert!(workspace.ai_generating);
        assert!(workspace.busy);

        let message = "Detailed subject\n\n- First change\n- Second change".to_owned();
        tx.send(Response::AiMessage(Ok(message.clone()))).unwrap();
        workspace.absorb();
        assert!(!workspace.ai_generating);
        assert!(!workspace.busy);
        assert_eq!(workspace.commit_message, message);
    }

    #[test]
    fn ai_failure_stops_generation_without_erasing_the_draft() {
        let (tx, rx) = mpsc::channel();
        let mut workspace = Workspace {
            rx: Some(rx),
            ai_generating: true,
            busy: true,
            commit_message: "Existing draft".to_owned(),
            ..Default::default()
        };
        tx.send(Response::AiMessage(Err("CLI timed out".to_owned()))).unwrap();
        workspace.absorb();
        assert!(!workspace.ai_generating);
        assert!(!workspace.busy);
        assert_eq!(workspace.commit_message, "Existing draft");
        assert_eq!(workspace.notice, Some(("CLI timed out".to_owned(), true)));
    }
}
