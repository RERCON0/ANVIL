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

use egui::{Align, Align2, Layout, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Vec2};

use crate::git::{self, Change, CommitLog, Status};
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
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
/// Height of one file-browser row; the list height is a multiple of it.
const FILE_ROW_HEIGHT: f32 = 20.0;
const TEXT_ROW_HEIGHT: f32 = 18.0;
/// Width of the line-number gutter of the diff views.
const GUTTER: f32 = 64.0;
/// The Seti folder glyph (U+E032, "folder" in the font) in the folder blue of
/// the reference file tree.
const FOLDER_ICON: (char, egui::Color32) = ('\u{e032}', egui::Color32::from_rgb(0x7B, 0xB3, 0xD9));

pub enum Request {
    Refresh {
        cwd: PathBuf,
    },
    /// Approval is for precisely the repository/configuration shown by the UI.
    Approve {
        identity: git::RepositoryIdentity,
    },
    /// `side`: Some(true) the index, Some(false) the worktree, None the
    /// index when it has changes for the file, else the worktree.
    Diff {
        path: String,
        side: Option<bool>,
    },
    CheckDiff {
        stamp: DiffStamp,
    },
    Stage {
        paths: Vec<String>,
        staged: bool,
    },
    Commit {
        message: String,
    },
    AiMessage {
        command: Option<String>,
    },
    Log,
    CommitDetail {
        hash: String,
    },
    CommitDiff {
        hash: String,
        path: String,
    },
    Files,
    /// Lines of every tracked file of the repository.
    CountLines,
    ReadFile {
        path: String,
    },
    /// `seen` is hunk `index` exactly as the user was shown it (the `@@ … @@`
    /// line, the context and the changes, byte for byte); the worker refuses to
    /// apply unless the freshly generated diff has the same bytes at `index`.
    ApplyHunks {
        path: String,
        index: usize,
        seen: Vec<u8>,
        from_index: bool,
    },
    /// The file named by a context-menu click, looked at on the worker: the UI
    /// thread must not canonicalize it or read its first bytes (a cloud
    /// placeholder makes that wait for a download).
    OpenExternal {
        path: String,
    },
    Reveal {
        path: String,
    },
    Fetch,
    Push,
    WritePath {
        path: String,
        folder: bool,
    },
    RenamePath {
        from: String,
        to: String,
    },
    DeletePath {
        path: String,
    },
}

impl Request {
    fn requires_current_repository(&self) -> bool {
        !matches!(self, Request::Refresh { .. } | Request::Approve { .. })
    }
}

/// A request plus the repository root the panel showed when it was made.
type Envelope = (Option<PathBuf>, Request);

/// The worker's answers. Private to the module: it carries the row indexes the
/// worker built, which are not part of anything outside the panel.
enum Response {
    /// Status plus the resolved repository root (None: not a repository).
    Status(Status, Option<PathBuf>),
    TrustRequired {
        identity: Option<git::RepositoryIdentity>,
        error: Option<String>,
    },
    Trusted(git::RepositoryIdentity),
    Diff {
        path: String,
        text: String,
        rows: Vec<TextRow>,
        /// Every hunk of `text` as git printed it, indexed like the `@@` rows.
        hunks: Vec<Vec<u8>>,
        staged: bool,
        side: Option<bool>,
    },
    DiffChecked(DiffStamp),
    Refreshed,
    /// The short hash, or why git refused. Exactly one answer per commit request.
    Committed(Result<String, String>),
    /// What the UI thread starts now (None: nothing to start).
    Launch(Option<Launch>),
    AiMessage(Result<String, String>),
    Log(CommitLog),
    CommitDetail {
        hash: String,
        detail: git::CommitDetail,
    },
    CommitDiff {
        hash: String,
        path: String,
        patch: Result<(String, Vec<TextRow>), String>,
    },
    Files(Vec<String>),
    LineCount {
        files: usize,
        lines: u64,
    },
    FileText {
        path: String,
        text: String,
        rows: Vec<TextRow>,
        truncated: bool,
    },
    Applied,
    Fetched(String),
    Pushed(String),
    Error(String),
}

/// What the UI thread starts for a file of the repository once the worker has
/// looked at it: the program that views it, or Explorer with it selected.
#[derive(Debug, PartialEq, Eq)]
enum Launch {
    Open(PathBuf),
    Reveal(PathBuf),
}

/// A row of the commit list: either a section header or a commit by index.
/// Flattened so the list can be virtualized by index.
enum CommitRow {
    Header { label: &'static str },
    Commit { index: usize },
}

/// Section headers and their commits, in display order.
fn commit_rows_of(log: &CommitLog) -> Vec<CommitRow> {
    let mut rows = Vec::with_capacity(log.commits.len() * 2);
    let mut section = None;
    for (index, commit) in log.commits.iter().enumerate() {
        if section != Some(commit.section) {
            section = Some(commit.section);
            let label = match commit.section {
                git::Section::Outgoing => strings::WORKSPACE_SECTION_OUTGOING,
                git::Section::Incoming => strings::WORKSPACE_SECTION_INCOMING,
                git::Section::History => strings::WORKSPACE_SECTION_HISTORY,
            };
            rows.push(CommitRow::Header { label });
        }
        rows.push(CommitRow::Commit { index });
    }
    rows
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
    Delete { path: String, folder: bool },
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    pub focus: bool,
}

struct CommitFile {
    path: String,
    patch: Option<Result<String, String>>,
    rows: Vec<TextRow>,
    wrapped: WrappedRows,
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
    /// The hunks of `diff_text` byte for byte, as the worker read them: what a
    /// click on a hunk header asks the worker to apply.
    diff_hunks: Vec<Vec<u8>>,
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
    detail_file: Option<CommitFile>,
    pub files: Vec<String>,
    /// Folders of the file tree that the user opened; the tree starts folded.
    pub file_expanded: HashSet<String>,
    /// Files and total lines of the last "Посчитать строки" run.
    pub line_count: Option<(usize, u64)>,
    pub file_filter: String,
    pub file_preview: Option<(String, String, bool)>,
    pub prompt: Option<Prompt>,
    diff_rows: Vec<TextRow>,
    diff_wrapped: WrappedRows,
    diff_stamp: Option<DiffStamp>,
    file_rows: Vec<FileRow>,
    file_rows_dirty: bool,
    files_loaded: bool,
    cached_filter: String,
    change_rows_cache: Vec<Row>,
    changes_dirty: bool,
    preview_rows: Vec<TextRow>,
    preview_wrapped: WrappedRows,
    markdown: MarkdownCache,
    ai_command: Option<String>,
    ai_generating: bool,
    /// A commit request is on its way and has not been answered: the button is
    /// off, so a second click cannot send a second `git commit`.
    committing: bool,
    trust_required: bool,
    trust_approval_pending: bool,
    pending_identity: Option<git::RepositoryIdentity>,
    last_cwd: Option<PathBuf>,
    log_status_seen: bool,
    tx: Option<Sender<Envelope>>,
    rx: Option<Receiver<Response>>,
    /// Requests sent and not yet answered. The worker answers every request
    /// with exactly one response, so this is the queue depth: `poll` stays idle
    /// while it is non-zero. Without it, the chained `Log`/`Files`/`Diff`
    /// requests that follow a status are invisible to `busy`, and a slow
    /// repository collects a fresh `Refresh` every poll interval on top of them.
    inflight: usize,
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
            diff_hunks: Vec::new(),
            diff_from_index: false,
            diff_side: None,
            commit_message: String::new(),
            notice: None,
            busy: false,
            last_poll: Instant::now() - POLL_INTERVAL,
            log: CommitLog::default(),
            graph: Vec::new(),
            detail: None,
            detail_file: None,
            files: Vec::new(),
            file_expanded: HashSet::new(),
            line_count: None,
            file_filter: String::new(),
            file_preview: None,
            prompt: None,
            diff_rows: Vec::new(),
            diff_wrapped: WrappedRows::default(),
            diff_stamp: None,
            file_rows: Vec::new(),
            file_rows_dirty: true,
            files_loaded: false,
            cached_filter: String::new(),
            change_rows_cache: Vec::new(),
            changes_dirty: true,
            preview_rows: Vec::new(),
            preview_wrapped: WrappedRows::default(),
            markdown: MarkdownCache::default(),
            ai_command: None,
            ai_generating: false,
            committing: false,
            trust_required: false,
            trust_approval_pending: false,
            pending_identity: None,
            last_cwd: None,
            log_status_seen: false,
            tx: None,
            rx: None,
            inflight: 0,
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
        // One repository read at a time. A directory change while one runs is
        // not queued as another request: `last_cwd` is left alone, so the next
        // idle poll picks the new directory up. A shell that cd's through a
        // script would otherwise queue one `git status` per step. `inflight`
        // covers the requests this panel chained itself (Log/Files/Diff), which
        // never set `busy`: without it a slow disk grows the queue instead of
        // simply polling less often.
        if self.busy || self.ai_generating || self.inflight > 0 {
            return;
        }
        let cwd_changed = self.last_cwd.as_ref() != Some(&cwd);
        if !cwd_changed && self.last_poll.elapsed() < POLL_INTERVAL {
            return;
        }
        self.last_poll = Instant::now();
        self.last_cwd = Some(cwd.clone());
        self.busy = true;
        self.request(Request::Refresh { cwd });
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
    /// `send` plus queue accounting. `send` itself stays `&self` for the read
    /// paths; this is the mutating entry point every caller that queues work
    /// for the worker goes through.
    fn request(&mut self, request: Request) {
        if self.tx.is_some() {
            self.inflight += 1;
            self.send(request);
        }
    }
    fn clear_repository_view(&mut self) {
        self.busy = false;
        self.trust_approval_pending = false;
        self.ai_generating = false;
        self.committing = false;
        self.status = Status::default();
        self.selected.clear();
        self.collapsed.clear();
        self.diff_path = None;
        self.diff_text.clear();
        self.diff_hunks.clear();
        self.diff_rows.clear();
        self.diff_wrapped = WrappedRows::default();
        self.diff_stamp = None;
        self.diff_side = None;
        self.detail = None;
        self.detail_file = None;
        self.files.clear();
        self.files_loaded = false;
        self.file_expanded.clear();
        self.line_count = None;
        self.file_preview = None;
        self.file_rows.clear();
        self.file_rows_dirty = true;
        self.change_rows_cache.clear();
        self.changes_dirty = true;
        self.preview_rows.clear();
        self.preview_wrapped = WrappedRows::default();
        self.markdown = MarkdownCache::default();
        self.log = CommitLog::default();
        self.graph.clear();
        self.prompt = None;
        // The draft stays: a configuration change must not eat typed or
        // AI-generated text the user has not committed yet.
        self.log_status_seen = false;
    }

    /// Sends the commit message, once: until the worker answers, a second call
    /// (a double click, a held Ctrl+Enter) sends nothing, so no second
    /// `git commit` can fail with "nothing to commit" over the first one's notice.
    fn start_commit(&mut self) {
        // Without a worker nothing would answer, and the button would stay off.
        if self.committing || self.tx.is_none() {
            return;
        }
        self.committing = true;
        self.busy = true;
        self.notice = None;
        self.request(Request::Commit { message: self.commit_message.clone() });
    }

    fn trust_view(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(strings::WORKSPACE_TRUST_TITLE).color(theme::colors().status_yellow).font(theme::font(13.0)),
        );
        if let Some(identity) = &self.pending_identity {
            ui.label(
                RichText::new(display(&identity.root.to_string_lossy(), 240))
                    .color(theme::colors().text)
                    .font(theme::field_font(11.5)),
            );
        }
        ui.label(RichText::new(strings::WORKSPACE_TRUST_HINT).color(theme::colors().dim).font(theme::font(12.0)));
        if let Some(identity) = &self.pending_identity {
            let hazards = identity.stamp.hazards();
            if !hazards.is_empty() {
                ui.label(
                    RichText::new(strings::WORKSPACE_TRUST_HAZARDS).color(theme::colors().text).font(theme::font(11.5)),
                );
                for hazard in hazards {
                    ui.label(
                        RichText::new(format!("· {}", display(hazard, 120)))
                            .color(theme::colors().status_yellow)
                            .font(theme::field_font(11.0)),
                    );
                }
            }
        }
        if let Some((notice, _)) = &self.notice {
            notice_label(ui, notice, theme::colors().status_red);
        }
        if ui
            .add_enabled(
                self.pending_identity.is_some() && !self.trust_approval_pending,
                theme::accent_button(strings::WORKSPACE_TRUST_APPROVE),
            )
            .clicked()
        {
            if let Some(identity) = self.pending_identity.clone() {
                self.busy = true;
                self.trust_approval_pending = true;
                self.request(Request::Approve { identity });
            }
        }
    }

    /// Applies worker responses; returns true when the UI should repaint.
    pub fn absorb(&mut self) -> bool {
        let mut repaint = false;
        let mut responses = Vec::new();
        let mut lost = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(response) => responses.push(response),
                    Err(mpsc::TryRecvError::Empty) => break,
                    // The worker only drops its receiver by panicking: the
                    // panel would otherwise stay `busy` with no one to answer.
                    Err(mpsc::TryRecvError::Disconnected) => {
                        lost = true;
                        break;
                    }
                }
            }
        }
        // Every response retires one queued request. `saturating_sub` because
        // `Approve` is answered twice (Trusted, then the status it unlocked):
        // the count may reach zero early, which costs one extra poll, never a
        // permanently stalled panel.
        self.inflight = self.inflight.saturating_sub(responses.len());
        for response in responses {
            repaint = true;
            match response {
                Response::TrustRequired { identity, error } => {
                    self.clear_repository_view();
                    self.root = identity.as_ref().map(|identity| identity.root.clone());
                    self.pending_identity = identity;
                    self.trust_required = true;
                    self.busy = false;
                    self.ai_generating = false;
                    self.notice = error.map(|message| (notice_text(&message), true));
                }
                Response::Trusted(identity) => {
                    self.root = Some(identity.root);
                    self.pending_identity = None;
                    self.trust_required = false;
                    self.trust_approval_pending = false;
                    self.notice = None;
                }
                Response::Status(status, root) => {
                    self.busy = false;
                    let root_changed = self.root != root;
                    let inventory_changed = root_changed
                        || self.status.head_oid != status.head_oid
                        || self
                            .status
                            .changes
                            .iter()
                            .map(|change| (&change.path, &change.original_path))
                            .ne(status.changes.iter().map(|change| (&change.path, &change.original_path)));
                    let commit_state_changed = root_changed
                        || !self.log_status_seen
                        || self.status.head_oid != status.head_oid
                        || self.status.upstream_oid != status.upstream_oid
                        || self.status.branch != status.branch
                        || self.status.upstream != status.upstream
                        || self.status.ahead != status.ahead
                        || self.status.behind != status.behind;
                    if root_changed {
                        self.clear_repository_view();
                    }
                    self.log_status_seen = true;
                    self.root = root;
                    self.trust_required = false;
                    self.pending_identity = None;
                    self.changes_dirty |= self.status.changes != status.changes;
                    self.status = status;
                    let paths: HashSet<_> = self.status.changes.iter().map(|change| &change.path).collect();
                    self.selected.retain(|path| paths.contains(path));
                    if let Some(path) = self.diff_path.clone() {
                        if !self.status.changes.iter().any(|change| change.path == path) {
                            self.diff_path = None;
                            self.diff_text.clear();
                            self.diff_hunks.clear();
                            self.diff_rows.clear();
                            self.diff_wrapped = WrappedRows::default();
                            self.diff_stamp = None;
                        } else {
                            let stamp = DiffStamp::for_view(self.root.as_deref(), &path, self.diff_side, &self.status);
                            self.request(Request::CheckDiff { stamp });
                        }
                    }
                    if commit_state_changed && self.root.is_some() {
                        self.request(Request::Log);
                    }
                    if inventory_changed || (self.tab == PanelTab::Files && !self.files_loaded) {
                        self.request(Request::Files);
                    }
                }
                Response::DiffChecked(stamp) => {
                    let current = stamp.root == self.root
                        && self.diff_path.as_deref() == Some(&stamp.path)
                        && stamp.side == self.diff_side
                        && stamp.head == self.status.head_oid
                        && stamp.change.as_ref() == self.status.changes.iter().find(|c| c.path == stamp.path);
                    if current && self.diff_stamp.as_ref() != Some(&stamp) {
                        self.request(Request::Diff { path: stamp.path.clone(), side: stamp.side });
                        self.diff_stamp = Some(stamp);
                    }
                }
                Response::Diff { path, text, rows, hunks, staged, side } => {
                    self.busy = false;
                    if self.diff_path.as_deref() == Some(path.as_str()) && self.diff_side == side {
                        // Even for an unchanged text: two diffs can read the same
                        // after the lossy decoding and still differ in their bytes.
                        self.diff_hunks = hunks;
                        if self.diff_text != text {
                            // The rows were indexed on the worker thread: an
                            // 8 MiB diff would otherwise build a row per line
                            // inside the frame that shows it.
                            self.diff_rows = rows;
                            self.diff_wrapped = WrappedRows::default();
                            self.diff_text = text;
                        }
                        self.diff_from_index = staged;
                    }
                }
                Response::Refreshed | Response::Applied => {
                    self.busy = false;
                    self.diff_stamp = None;
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                    self.request(Request::Log);
                    if self.tab == PanelTab::Files {
                        self.request(Request::Files);
                    }
                }
                Response::Committed(result) => {
                    self.busy = false;
                    self.committing = false;
                    match result {
                        Ok(hash) => {
                            self.commit_message.clear();
                            self.notice = Some((notice_text(&strings::workspace_committed(&hash)), false));
                            self.request(Request::Log);
                            self.last_poll = Instant::now() - POLL_INTERVAL;
                        }
                        Err(message) => self.notice = Some((notice_text(&message), true)),
                    }
                }
                Response::Launch(launch) => match launch {
                    Some(Launch::Open(full)) => crate::settings_ui::open_path(&full),
                    Some(Launch::Reveal(full)) => reveal_resolved(&full),
                    None => {}
                },
                Response::AiMessage(result) => {
                    self.busy = false;
                    self.ai_generating = false;
                    match result {
                        Ok(message) => self.commit_message = message,
                        Err(message) => self.notice = Some((notice_text(&message), true)),
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
                    self.detail_file = None;
                }
                Response::CommitDiff { hash, path, patch } => {
                    self.busy = false;
                    if self.detail.as_ref().is_some_and(|(current, _)| current == &hash) {
                        if let Some(file) = self.detail_file.as_mut().filter(|file| file.path == path) {
                            match patch {
                                Ok((patch, rows)) => {
                                    file.rows = rows;
                                    file.patch = Some(Ok(patch));
                                }
                                Err(error) => {
                                    file.rows.clear();
                                    file.patch = Some(Err(error));
                                }
                            }
                            file.wrapped = WrappedRows::default();
                        }
                    }
                }
                Response::Files(files) => {
                    self.busy = false;
                    self.files_loaded = true;
                    if self.files != files {
                        self.files = files;
                        self.file_rows_dirty = true;
                    }
                }
                Response::LineCount { files, lines } => {
                    self.busy = false;
                    self.line_count = Some((files, lines));
                }
                Response::FileText { path, text, rows, truncated } => {
                    self.busy = false;
                    if !self
                        .file_preview
                        .as_ref()
                        .is_some_and(|(old_path, old_text, _)| old_path == &path && old_text == &text)
                    {
                        self.preview_rows = rows;
                        self.preview_wrapped = WrappedRows::default();
                        self.markdown = MarkdownCache::default();
                    }
                    self.file_preview = Some((path, text, truncated));
                }
                Response::Fetched(what) => {
                    self.busy = false;
                    self.notice = Some((notice_text(&strings::workspace_fetch_done(&what)), false));
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                    self.request(Request::Log);
                }
                Response::Pushed(branch) => {
                    self.busy = false;
                    self.notice = Some((notice_text(&strings::workspace_pushed(&branch)), false));
                    self.last_poll = Instant::now() - POLL_INTERVAL;
                    self.request(Request::Log);
                }
                Response::Error(message) => {
                    self.busy = false;
                    self.notice = Some((notice_text(&message), true));
                }
            }
        }
        // After the answers: `Disconnected` is reported only once the channel is
        // drained, so everything the worker sent before it died is applied above.
        if lost {
            self.tx = None;
            self.rx = None;
            self.busy = false;
            self.ai_generating = false;
            self.committing = false;
            self.trust_approval_pending = false;
            self.inflight = 0;
            if self.notice.is_none() {
                self.notice = Some((strings::WORKSPACE_WORKER_LOST.to_owned(), true));
            }
            repaint = true;
        }
        self.busy |= self.ai_generating;
        repaint
    }

    /// Draws the panel; returns actions the app must handle.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        pane: crate::layout::split_tree::PaneId,
        ai_command: Option<&str>,
    ) -> Vec<WorkspaceAction> {
        if self.ai_command.as_deref() != ai_command {
            self.ai_command = ai_command.map(str::to_owned);
        }
        let mut actions = Vec::new();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::colors().lift);
        painter.vline(rect.min.x + 0.5, rect.y_range(), Stroke::new(1.0, theme::colors().line));
        let inner = rect.shrink2(Vec2::new(10.0, 8.0));
        // Every pane has its own panel with its own widgets: without the pane
        // in the salt two panels share one id, and egui paints a clash overlay
        // over them (and their scroll state is shared).
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner).id_salt(("workspace-panel", pane)), |ui| {
            // Reserve the scroll-bar strip instead of letting it float over the
            // rows: file names and hashes were running underneath it.
            ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
            // Two modes only: changes with a separate history viewport, or files.
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (PanelTab::Files, strings::WORKSPACE_TAB_FILES.to_owned()),
                    (PanelTab::Changes, strings::workspace_changes_tab(self.status.changes.len())),
                ] {
                    // Framed like the other panel chips, with the selection dot
                    // of the settings rows inside.
                    let selected = self.tab == tab;
                    let (dot, color) =
                        if selected { ("●", theme::colors().accent) } else { ("○", theme::colors().dim) };
                    let text = egui::RichText::new(format!("{dot} {label}")).font(theme::field_font(12.5)).color(color);
                    if ui.add(egui::Button::new(text)).clicked() {
                        self.tab = tab;
                        self.refresh_soon();
                        match tab {
                            PanelTab::Changes => {
                                self.request(Request::Log);
                            }
                            PanelTab::Files => self.request(Request::Files),
                        }
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(theme::ghost_button("×")).on_hover_text(strings::WORKSPACE_CLOSE).clicked() {
                        actions.push(WorkspaceAction::Close);
                    }
                    if ui.add(theme::ghost_button("⟳")).on_hover_text(strings::WORKSPACE_REFRESH).clicked() {
                        self.refresh_soon();
                        self.request(Request::Log);
                        self.request(Request::Files);
                    }
                });
            });
            theme::hairline(ui);
            if self.trust_required {
                self.trust_view(ui);
            } else {
                if self.prompt.is_some() {
                    self.prompt_row(ui);
                }
                match self.tab {
                    PanelTab::Changes => self.changes_mode(ui),
                    PanelTab::Files => self.files_tab(ui),
                }
            }
        });
        actions
    }

    /// One column: repo header, commit box, changes, then the commit history.
    fn changes_mode(&mut self, ui: &mut egui::Ui) {
        if self.status.changes.is_empty() && self.status.branch.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(strings::WORKSPACE_NO_REPO_HINT).color(theme::colors().faint).font(theme::font(12.0)),
            );
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
            ui.label(RichText::new(name).color(theme::colors().text).font(theme::font(12.5)));
            if !self.status.branch.is_empty() {
                ui.label(
                    RichText::new(&self.status.branch).color(theme::colors().accent).font(theme::field_font(12.0)),
                );
                if self.status.ahead > 0 {
                    ui.label(
                        RichText::new(format!("↑{}", self.status.ahead))
                            .color(theme::colors().accent)
                            .font(theme::field_font(11.5)),
                    );
                }
                if self.status.behind > 0 {
                    ui.label(
                        RichText::new(format!("↓{}", self.status.behind))
                            .color(theme::colors().status_yellow)
                            .font(theme::field_font(11.5)),
                    );
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.status.branch.is_empty() {
                    if ui
                        .add(theme::animated_accent_button(strings::WORKSPACE_PUBLISH))
                        .on_hover_text(strings::WORKSPACE_PUSH_HINT)
                        .clicked()
                    {
                        self.busy = true;
                        self.request(Request::Push);
                    }
                    if ui.add(theme::ghost_button("fetch")).on_hover_text(strings::WORKSPACE_FETCH_HINT).clicked() {
                        self.busy = true;
                        self.request(Request::Fetch);
                    }
                }
            });
        });
        let additions: u32 = self.status.changes.iter().map(|c| c.additions).sum();
        let deletions: u32 = self.status.changes.iter().map(|c| c.deletions).sum();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("+{additions}"))
                    .color(theme::colors().status_green)
                    .font(theme::field_font(11.5)),
            );
            ui.label(
                RichText::new(format!("−{deletions}")).color(theme::colors().status_red).font(theme::field_font(11.5)),
            );
            ui.label(
                RichText::new(strings::workspace_staged(self.counts().0, self.counts().1))
                    .color(theme::colors().faint)
                    .font(theme::font(11.0)),
            );
        });
        if let Some((notice, error)) = &self.notice {
            let colour = if *error { theme::colors().status_red } else { theme::colors().status_green };
            notice_label(ui, notice, colour);
        }
        theme::hairline(ui);
    }

    fn commit_box(&mut self, ui: &mut egui::Ui) {
        // A commit or generation in flight owns the draft it was sent. A merely
        // disabled text edit still consumes keystrokes while it holds focus, so
        // the editor also stops being interactive and gives the focus up; the
        // text otherwise typed here would be cleared with the sent draft.
        let editable = !self.ai_generating && !self.committing;
        let editor = egui::Frame::NONE
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(4, 2))
            .show(ui, |ui| {
                ui.style_mut().spacing.scroll.foreground_color = true;
                ScrollArea::vertical()
                    .id_salt("workspace-commit-message")
                    .max_height(120.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_enabled(
                            editable,
                            egui::TextEdit::multiline(&mut self.commit_message)
                                .frame(egui::Frame::NONE)
                                .margin(egui::Margin::same(0))
                                .font(theme::field_font(12.0))
                                .desired_rows(6)
                                .hint_text(strings::WORKSPACE_COMMIT_HINT)
                                .desired_width(f32::INFINITY)
                                .interactive(editable),
                        )
                    })
                    .inner
            });
        let commit = editor.inner;
        if !editable && commit.has_focus() {
            commit.surrender_focus();
        }
        let visuals = ui.style().interact(&commit);
        let stroke = if commit.has_focus() { ui.visuals().selection.stroke } else { visuals.bg_stroke };
        ui.painter().rect_stroke(editor.response.rect, visuals.corner_radius, stroke, egui::StrokeKind::Middle);
        let ctrl_enter = commit.has_focus() && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        ui.horizontal_wrapped(|ui| {
            let has_staged = self.status.changes.iter().any(Change::staged);
            let can_commit = editable && !self.commit_message.trim().is_empty() && has_staged;
            if ui.add_enabled(can_commit, theme::accent_button(strings::WORKSPACE_COMMIT)).clicked()
                || (can_commit && ctrl_enter)
            {
                self.start_commit();
            }
            let ai_label = match &self.ai_command {
                Some(command) => strings::workspace_ai(command.split_whitespace().next().unwrap_or(command)),
                None => strings::WORKSPACE_AI.to_owned(),
            };
            let ai =
                ui.add_enabled(editable && has_staged, theme::ghost_button(ai_label)).on_hover_text(if has_staged {
                    self.ai_command.as_deref().unwrap_or(strings::WORKSPACE_NO_AI_COMMAND)
                } else {
                    strings::WORKSPACE_AI_NO_STAGE
                });
            if ai.clicked() {
                self.ai_generating = true;
                self.busy = true;
                self.notice = None;
                self.request(Request::AiMessage { command: self.ai_command.clone() });
                ui.ctx().request_repaint();
            }
        });
        if self.ai_generating {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(theme::colors().accent));
                ui.label(
                    RichText::new(strings::WORKSPACE_AI_GENERATING)
                        .color(theme::colors().accent)
                        .font(theme::font(11.5)),
                );
            });
        }
        ui.add_space(4.0);
    }

    /// `[ КОММИТЫ ]` header plus the graph list, which fills the rest of the
    /// column and scrolls under the header.
    fn commits_section(&mut self, ui: &mut egui::Ui) {
        let ahead = self.status.ahead;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("[ {} ]", strings::WORKSPACE_COMMITS_TITLE))
                    .color(theme::colors().faint)
                    .font(theme::font(11.5)),
            );
            if ahead > 0 {
                ui.label(
                    RichText::new(format!("↑{ahead}")).color(theme::colors().accent).font(theme::field_font(11.5)),
                );
            }
            if self.log.truncated {
                ui.label(
                    RichText::new(strings::WORKSPACE_TRUNCATED).color(theme::colors().faint).font(theme::font(10.5)),
                );
            }
        });
        theme::hairline(ui);
        if self.log.commits.is_empty() {
            ui.label(RichText::new(strings::WORKSPACE_NO_COMMITS).color(theme::colors().faint).font(theme::font(11.5)));
            return;
        }
        self.commit_rows(ui);
    }

    /// The commit list as fixed-height rows, so it can be virtualized: the
    /// whole log used to lay out every commit on every frame.
    fn commit_rows(&mut self, ui: &mut egui::Ui) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        const ROW_HEIGHT: f32 = 22.0;
        let rows = commit_rows_of(&self.log);
        let width = ui.available_width();
        let mut opened: Option<String> = None;
        // A single viewport fills the remaining column. Nesting two default
        // ScrollAreas imposed a 400pt ceiling and left tall windows unused.
        let scroll = ScrollArea::vertical()
            .id_salt("workspace-commits-rows")
            .max_height(ui.available_height().max(0.0))
            .auto_shrink([false, false]);
        // show_rows computes its virtual row stride before calling the closure.
        // Set spacing here so off-screen offsets match the actual graph rows.
        ui.spacing_mut().item_spacing.y = 0.0;
        scroll.show_rows(ui, ROW_HEIGHT, rows.len(), |ui, visible| {
            // Rows sit edge to edge: each draws its lane segments only inside
            // its own rect, so any item spacing is a gap in every lane.
            for row in rows[visible].iter() {
                match row {
                    // A header occupies a normal row: virtualization needs one
                    // height for every line, and the label fits in it.
                    CommitRow::Header { label, .. } => {
                        let width = ui.available_width();
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::hover());
                        ui.painter_at(rect).text(
                            Pos2::new(rect.min.x, rect.center().y),
                            Align2::LEFT_CENTER,
                            *label,
                            theme::font(10.5),
                            theme::colors().faint,
                        );
                    }
                    CommitRow::Commit { index } => self.commit_row(ui, *index, now, width, &mut opened),
                }
            }
        });
        if let Some(hash) = opened {
            self.busy = true;
            self.request(Request::CommitDetail { hash });
        }
    }

    /// One commit's row: its lane graph, badges, time, hash and subject.
    fn commit_row(&mut self, ui: &mut egui::Ui, index: usize, now: i64, width: f32, opened: &mut Option<String>) {
        const ROW_HEIGHT: f32 = 22.0;
        let commit = &self.log.commits[index];
        {
            // Borrowed: a clone allocated the segment list of every visible row on every frame.
            let blank = graph::Row { lane: 0, lane_count: 1, segments: Vec::new() };
            let row = self.graph.get(index).unwrap_or(&blank);
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
            let painter = ui.painter_at(rect);
            if response.hovered() {
                painter.rect_filled(rect, 0.0, theme::colors().tab_hover_bg);
            }
            let lane_x = |lane: usize| rect.min.x + lane as f32 * LANE_WIDTH + LANE_WIDTH / 2.0;
            let mid = rect.center().y;
            for segment in &row.segments {
                let stroke = Stroke::new(1.5, section_color(commit.section));
                let (from, to) = (lane_x(segment.from_lane), lane_x(segment.to_lane));
                match segment.kind {
                    graph::Kind::Through => {
                        painter.line_segment(
                            [egui::Pos2::new(from, rect.min.y), egui::Pos2::new(from, rect.max.y)],
                            stroke,
                        );
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
            let time_galley = painter.layout_no_wrap(time_text.clone(), theme::font(10.0), theme::colors().faint);
            let hash_galley =
                painter.layout_no_wrap(commit.short.clone(), theme::field_font(10.0), theme::colors().faint);
            let right_width = time_galley.size().x + hash_galley.size().x + 12.0;
            let right_x = rect.max.x - 2.0;
            painter.galley(
                egui::Pos2::new(right_x - time_galley.size().x, mid - time_galley.size().y / 2.0),
                time_galley,
                theme::colors().faint,
            );
            painter.galley(
                egui::Pos2::new(right_x - right_width, mid - hash_galley.size().y / 2.0),
                hash_galley,
                theme::colors().faint,
            );
            let mut badge_x = text_x;
            for reference in &commit.refs {
                let name = display(reference.strip_prefix("HEAD -> ").unwrap_or(reference), 40);
                let colour = ref_color(&name);
                let galley = painter.layout_no_wrap(name, theme::field_font(10.0), theme::colors().chrome_bg);
                let badge =
                    Rect::from_min_size(egui::Pos2::new(badge_x, mid - 7.0), Vec2::new(galley.size().x + 10.0, 14.0));
                // A chip is only worth drawing while the subject keeps room to
                // the left of the time and hash column.
                if badge.max.x > rect.max.x - right_width - 10.0 - MIN_SUBJECT_WIDTH {
                    break;
                }
                painter.rect_filled(badge, egui::CornerRadius::same(3), colour);
                painter.galley(
                    egui::Pos2::new(badge.min.x + 5.0, badge.min.y + 1.0),
                    galley,
                    theme::colors().chrome_bg,
                );
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
                    theme::colors().text,
                );
            }
            if response.hovered() {
                let tooltip = format!(
                    "{}\n{} · {}\n{}",
                    display(&commit.subject, 500),
                    display(&commit.author, 80),
                    commit.short,
                    time_text
                );
                let _ = response.clone().on_hover_text(tooltip);
            }
            if response.clicked() {
                *opened = Some(commit.hash.clone());
            }
        }
    }

    fn prompt_row(&mut self, ui: &mut egui::Ui) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        let label = match &prompt.kind {
            PromptKind::NewFile => strings::WORKSPACE_NEW_FILE.to_owned(),
            PromptKind::NewFolder => strings::WORKSPACE_NEW_FOLDER.to_owned(),
            PromptKind::Rename(path) => format!("{}: {}", strings::WORKSPACE_RENAME, display(path, 240)),
            PromptKind::Delete { path, .. } => format!("{}: {}", strings::WORKSPACE_DELETE, display(path, 240)),
        };
        ui.label(RichText::new(label).color(theme::colors().dim).font(theme::font(11.5)));
        if let PromptKind::Delete { folder, .. } = prompt.kind {
            let hint = if folder { strings::WORKSPACE_DELETE_HINT } else { strings::WORKSPACE_DELETE_FILE_HINT };
            ui.label(RichText::new(hint).color(theme::colors().faint).font(theme::font(11.5)));
        }
        let mut commit = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let deleting = matches!(prompt.kind, PromptKind::Delete { .. });
            if !deleting {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut prompt.text).font(theme::field_font(12.0)).desired_width(180.0),
                );
                if prompt.focus {
                    field.request_focus();
                    prompt.focus = false;
                }
                commit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            }
            let action = if deleting { strings::WORKSPACE_DELETE } else { strings::SETTINGS_SAVE };
            if ui.add(theme::ghost_button(action)).clicked() {
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
            // The previous answer is about the previous operation.
            self.notice = None;
            match prompt.kind {
                PromptKind::NewFile if !text.is_empty() => {
                    self.busy = true;
                    self.request(Request::WritePath { path: text, folder: false });
                }
                PromptKind::NewFolder if !text.is_empty() => {
                    self.busy = true;
                    self.request(Request::WritePath { path: text, folder: true });
                }
                PromptKind::Rename(from) if !text.is_empty() => {
                    self.busy = true;
                    self.request(Request::RenamePath { from, to: text });
                }
                PromptKind::Delete { path, .. } => {
                    self.busy = true;
                    self.request(Request::DeletePath { path });
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
        let has_selection = !self.selected.is_empty();
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(has_selection, theme::ghost_button(strings::WORKSPACE_STAGE)).clicked() {
                let paths: Vec<String> = self.selected.iter().cloned().collect();
                self.busy = true;
                self.request(Request::Stage { paths, staged: true });
            }
            if ui.add_enabled(has_selection, theme::ghost_button(strings::WORKSPACE_UNSTAGE)).clicked() {
                let paths = unstage_paths(self.status.changes.iter().filter(|c| self.selected.contains(&c.path)));
                self.busy = true;
                self.request(Request::Stage { paths, staged: false });
            }
            if ui.add(theme::ghost_button(strings::WORKSPACE_STAGE_ALL)).clicked() {
                let paths: Vec<String> =
                    self.status.changes.iter().filter(|c| c.unstaged()).map(|c| c.path.clone()).collect();
                self.busy = true;
                self.request(Request::Stage { paths, staged: true });
            }
            if ui.add(theme::ghost_button(strings::WORKSPACE_UNSTAGE_ALL)).clicked() {
                let paths = unstage_paths(self.status.changes.iter().filter(|c| c.staged()));
                self.busy = true;
                self.request(Request::Stage { paths, staged: false });
            }
        });
        const ROW_HEIGHT: f32 = 21.0;
        const INDENT: f32 = 12.0;
        if self.changes_dirty {
            self.change_rows_cache = self.rows();
            self.changes_dirty = false;
        }
        let rows = std::mem::take(&mut self.change_rows_cache);
        // A long tree gets its own bounded scroll, short lists stay inline. The
        // bound is a share of the column, so the commit history below always
        // keeps room for its own scroll.
        let limit = (ui.available_height() * 0.45).clamp(ROW_HEIGHT * 3.0, 300.0);
        let spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical().id_salt("workspace-changes").max_height(limit).auto_shrink([false, true]).show_rows(
            ui,
            ROW_HEIGHT,
            rows.len(),
            |ui, visible| {
                for row in &rows[visible] {
                    let width = ui.available_width();
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
                    let painter = ui.painter_at(rect);
                    if response.hovered() {
                        painter.rect_filled(rect, 0.0, theme::colors().tab_hover_bg);
                    }
                    match row {
                        Row::Folder { path, depth, count } => {
                            let collapsed = self.collapsed.contains(path);
                            let name = display(path.rsplit('/').next().unwrap_or(path), 40);
                            let caret = if collapsed { "▸" } else { "▾" };
                            painter.text(
                                egui::Pos2::new(rect.min.x + *depth as f32 * INDENT, rect.center().y),
                                Align2::LEFT_CENTER,
                                format!("{caret} {name}/"),
                                theme::font(12.0),
                                theme::colors().dim,
                            );
                            painter.text(
                                egui::Pos2::new(rect.max.x, rect.center().y),
                                Align2::RIGHT_CENTER,
                                count.to_string(),
                                theme::field_font(11.0),
                                theme::colors().faint,
                            );
                            if response.clicked() {
                                if collapsed {
                                    self.collapsed.remove(path);
                                } else {
                                    self.collapsed.insert(path.clone());
                                }
                                self.changes_dirty = true;
                            }
                        }
                        Row::File { change } => {
                            let path = &change.path;
                            let depth = path.matches('/').count();
                            let selected = self.selected.contains(path);
                            let box_x = rect.min.x + depth as f32 * INDENT;
                            painter.text(
                                egui::Pos2::new(box_x, rect.center().y),
                                Align2::LEFT_CENTER,
                                if selected { "[×]" } else { "[ ]" },
                                theme::field_font(12.0),
                                if selected { theme::colors().accent } else { theme::colors().faint },
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
                                theme::colors().text,
                            );
                            let letter = change.letter();
                            let letter_galley = painter.layout_no_wrap(
                                letter.to_string(),
                                theme::field_font(11.5),
                                status_color(letter),
                            );
                            let letter_width = letter_galley.size().x;
                            painter.galley(
                                egui::Pos2::new(
                                    rect.max.x - 2.0 - letter_width,
                                    rect.center().y - letter_galley.size().y / 2.0,
                                ),
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
                                theme::colors().faint,
                            );
                            if response.clicked() {
                                let checkbox_hit =
                                    response.interact_pointer_pos().is_some_and(|pos| pos.x < box_x + 28.0);
                                if checkbox_hit {
                                    if selected {
                                        self.selected.remove(path);
                                    } else {
                                        self.selected.insert(path.clone());
                                    }
                                } else {
                                    self.diff_path = Some(path.clone());
                                    self.diff_text.clear();
                                    self.diff_hunks.clear();
                                    self.diff_rows.clear();
                                    self.diff_stamp = None;
                                    self.diff_wrapped = WrappedRows::default();
                                    self.diff_side = None;
                                    self.busy = true;
                                    self.diff_stamp = None;
                                    self.request(Request::Diff { path: path.clone(), side: None });
                                }
                            }
                        }
                    }
                }
            },
        );
        ui.spacing_mut().item_spacing.y = spacing;
        self.change_rows_cache = rows;

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
            ui.label(RichText::new(display(&path, 120)).color(theme::colors().dim).font(theme::field_font(11.5)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::ghost_button(strings::WORKSPACE_DIFF_STAGED)).clicked() {
                    self.diff_text.clear();
                    self.diff_hunks.clear();
                    self.diff_rows.clear();
                    self.diff_wrapped = WrappedRows::default();
                    self.diff_stamp = None;
                    self.diff_side = Some(!self.diff_from_index);
                    self.busy = true;
                    self.request(Request::Diff { path: path.clone(), side: self.diff_side });
                }
            });
        });
        let mut hunks_to_apply: Option<(usize, bool)> = None;
        let height = (ui.available_height() * 0.55).clamp(24.0, 300.0);
        self.diff_wrapped.prepare(ui, &self.diff_text, &self.diff_rows);
        let spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt(("workspace-diff", &path, self.diff_from_index))
            .max_height(height)
            .auto_shrink([false, true])
            .show_rows(ui, TEXT_ROW_HEIGHT, self.diff_wrapped.rows.len(), |ui, visible| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for index in visible {
                    let row = &self.diff_wrapped.rows[index];
                    let line = &self.diff_text[row.bytes.clone()];
                    if let Some(hunk_index) = row.hunk {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(ui.available_width(), TEXT_ROW_HEIGHT), Sense::click());
                        let label = if self.diff_from_index { "◂" } else { "▸" };
                        ui.painter().text(
                            rect.left_center(),
                            Align2::LEFT_CENTER,
                            label,
                            theme::font(12.0),
                            theme::colors().accent,
                        );
                        paint_text_row(ui, rect, line, None, theme::colors().diff_hunk);
                        if response.clicked() {
                            hunks_to_apply = Some((hunk_index, self.diff_from_index));
                        }
                    } else {
                        patch_line(ui, line, row.number, row.kind);
                    }
                }
            });
        ui.spacing_mut().item_spacing.y = spacing;
        if let Some((index, from_index)) = hunks_to_apply {
            // The hunk as it was shown: the worker applies it only if the file still has it.
            if let Some(seen) = self.diff_hunks.get(index).cloned() {
                self.busy = true;
                self.request(Request::ApplyHunks { path, index, seen, from_index });
            }
        }
    }

    /// Commit detail replaces the column while open.
    fn detail_view(&mut self, ui: &mut egui::Ui) -> bool {
        let Some((hash, detail)) = self.detail.take() else { return false };
        let action = commit_detail_view(ui, &hash, &detail, self.detail_file.as_mut());
        if matches!(action, Some(CommitDetailAction::Back)) && self.detail_file.is_none() {
            return true;
        }
        match action {
            Some(CommitDetailAction::Back) => self.detail_file = None,
            Some(CommitDetailAction::OpenFile(path)) => {
                self.detail_file = Some(CommitFile {
                    path: path.clone(),
                    patch: None,
                    rows: Vec::new(),
                    wrapped: WrappedRows::default(),
                });
                self.busy = true;
                self.request(Request::CommitDiff { hash: hash.clone(), path });
            }
            None => {}
        }
        self.detail = Some((hash, detail));
        true
    }
}

enum CommitDetailAction {
    Back,
    OpenFile(String),
}

/// The commit overview or one selected file; navigation stays outside the scroll.
fn commit_detail_view(
    ui: &mut egui::Ui,
    hash: &str,
    detail: &git::CommitDetail,
    mut file: Option<&mut CommitFile>,
) -> Option<CommitDetailAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.add(theme::ghost_button(strings::WORKSPACE_BACK)).clicked() {
            action = Some(CommitDetailAction::Back);
        }
        if ui.add(theme::ghost_button(strings::WORKSPACE_COPY_HASH)).clicked() {
            copy_to_clipboard(hash);
        }
        if let Some(file) = file.as_ref() {
            let patch = file.patch.as_ref().and_then(|result| result.as_ref().ok());
            if ui.add_enabled(patch.is_some(), theme::ghost_button(strings::WORKSPACE_COPY_PATCH)).clicked() {
                if let Some(patch) = patch {
                    copy_to_clipboard(patch);
                }
            }
        }
    });
    if let Some(file) = file.as_mut() {
        ui.label(
            RichText::new(display(&file.path, file.path.len())).color(theme::colors().text).font(theme::font(12.5)),
        );
        match &file.patch {
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0).color(theme::colors().accent));
                    ui.label(strings::WORKSPACE_DIFF_LOADING);
                });
            }
            Some(Err(error)) => {
                ui.label(RichText::new(error).color(theme::colors().status_red));
            }
            Some(Ok(patch)) => {
                file.wrapped.prepare(ui, patch, &file.rows);
                let spacing = ui.spacing().item_spacing.y;
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical()
                    .id_salt(("workspace-commit-file", hash, &file.path))
                    .auto_shrink([false, false])
                    .show_rows(ui, TEXT_ROW_HEIGHT, file.wrapped.rows.len(), |ui, visible| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for index in visible {
                            let row = &file.wrapped.rows[index];
                            patch_line(ui, &patch[row.bytes.clone()], row.number, row.kind);
                        }
                    });
                ui.spacing_mut().item_spacing.y = spacing;
            }
        }
        return action;
    }
    // The complete message and file list share the remaining viewport.
    ScrollArea::vertical().id_salt(("workspace-commit-detail", hash)).auto_shrink([false, false]).show(ui, |ui| {
        let mut fields = detail.header.splitn(5, '\n');
        let full_hash = fields.next().unwrap_or("");
        let author = fields.next().unwrap_or("");
        let date = fields.next().unwrap_or("");
        let subject = fields.next().unwrap_or("");
        let body = fields.next().unwrap_or("").trim();
        let subject = display(subject, subject.len());
        let body = display_multiline(body, body.len());
        ui.label(RichText::new(subject).color(theme::colors().text).font(theme::font(12.5)));
        ui.label(
            RichText::new(format!("{} · {}", display(author, 80), display(date, 40)))
                .color(theme::colors().faint)
                .font(theme::font(10.5)),
        );
        ui.label(RichText::new(full_hash).color(theme::colors().faint).font(theme::field_font(10.5)));
        if !body.is_empty() {
            ui.label(RichText::new(body).color(theme::colors().dim).font(theme::font(11.0)));
        }
        theme::hairline(ui);
        for (status, path, additions, deletions) in &detail.files {
            ui.horizontal(|ui| {
                ui.label(RichText::new(status.to_string()).color(status_color(*status)).font(theme::field_font(11.5)));
                if ui
                    .selectable_label(
                        false,
                        RichText::new(display(path, 120)).color(theme::colors().text).font(theme::font(11.5)),
                    )
                    .on_hover_text(display(path, 240))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    action = Some(CommitDetailAction::OpenFile(path.clone()));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("+{additions} −{deletions}"))
                            .color(theme::colors().faint)
                            .font(theme::field_font(10.5)),
                    );
                });
            });
        }
    });
    action
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
                    rows.push(FileRow {
                        depth: index,
                        name: display(segment, 120),
                        dir: None,
                        path: folder.clone(),
                        folder: true,
                    });
                }
                if !self.file_expanded.contains(folder) {
                    hidden = true;
                }
            }
            if !hidden && !name.is_empty() {
                rows.push(FileRow {
                    depth: dirs.len(),
                    name: display(name[0], 120),
                    dir: None,
                    path: path.clone(),
                    folder: false,
                });
            }
            previous = folders;
        }
        rows
    }

    fn update_file_rows(&mut self) {
        if !self.file_rows_dirty && self.cached_filter == self.file_filter {
            return;
        }
        self.file_rows = if self.file_filter.trim().is_empty() {
            self.file_tree()
        } else {
            let mut matches: Vec<_> = self
                .files
                .iter()
                .filter_map(|path| crate::profiles::fuzzy_score(&self.file_filter, path).map(|score| (score, path)))
                .collect();
            matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            matches
                .into_iter()
                .take(400)
                .map(|(_, path)| {
                    let (dir, name) = path.rsplit_once('/').map_or(("", path.as_str()), |(dir, name)| (dir, name));
                    FileRow {
                        depth: 0,
                        name: display(name, 120),
                        dir: Some(if dir.is_empty() { String::new() } else { format!("{}/", display(dir, 120)) }),
                        path: path.clone(),
                        folder: false,
                    }
                })
                .collect()
        };
        self.cached_filter.clone_from(&self.file_filter);
        self.file_rows_dirty = false;
    }

    fn toggle_file_folder(&mut self, path: &str) {
        if !self.file_expanded.remove(path) {
            self.file_expanded.insert(path.to_owned());
        }
        self.file_rows_dirty = true;
    }

    /// One row of the file browser; returns the path to open when a file was
    /// clicked. Folders fold instead.
    fn file_row(&mut self, ui: &mut egui::Ui, row: &FileRow) -> Option<String> {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, FILE_ROW_HEIGHT), Sense::click());
        let painter = ui.painter_at(rect);
        if response.hovered() {
            painter.rect_filled(rect, 0.0, theme::colors().tab_hover_bg);
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
                theme::colors().dim,
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
                theme::colors().text,
            );
            if response.clicked() {
                self.toggle_file_folder(&row.path);
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
            let name_width = painter.layout_no_wrap(name.clone(), theme::font(11.5), theme::colors().text).size().x;
            if let Some(dir) = &row.dir {
                let dir_limit = (limit - name_width).max(0.0);
                if dir_limit > 10.0 {
                    let dir = elide_front(&painter, dir, theme::font(11.0), dir_limit);
                    let galley = painter.layout_no_wrap(dir, theme::font(11.0), theme::colors().faint);
                    painter.galley(
                        egui::Pos2::new(x, rect.center().y - galley.size().y / 2.0),
                        galley.clone(),
                        theme::colors().faint,
                    );
                    x += galley.size().x;
                }
            }
            painter.text(
                egui::Pos2::new(x, rect.center().y),
                Align2::LEFT_CENTER,
                name,
                theme::font(11.5),
                theme::colors().text,
            );
            if response.clicked() {
                opened = Some(row.path.clone());
            }
        }
        let path = row.path.clone();
        response.context_menu(|ui| {
            if ui.button(strings::WORKSPACE_OPEN_EXTERNAL).clicked() {
                self.request(Request::OpenExternal { path: path.clone() });
                ui.close();
            }
            if ui.button(strings::WORKSPACE_REVEAL).clicked() {
                self.request(Request::Reveal { path: path.clone() });
                ui.close();
            }
            if ui.button(strings::WORKSPACE_RENAME).clicked() {
                self.prompt = Some(Prompt { kind: PromptKind::Rename(path.clone()), text: path.clone(), focus: true });
                ui.close();
            }
            if ui.button(strings::WORKSPACE_DELETE).clicked() {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Delete { path: path.clone(), folder: row.folder },
                    text: String::new(),
                    focus: false,
                });
                ui.close();
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
                self.request(Request::Files);
            }
            if ui.add(theme::ghost_button(strings::WORKSPACE_COUNT_LINES)).clicked() {
                self.busy = true;
                self.request(Request::CountLines);
            }
            if let Some((files, lines)) = self.line_count {
                ui.label(
                    RichText::new(strings::workspace_line_count(files, lines))
                        .color(theme::colors().accent)
                        .font(theme::font(11.0)),
                );
            }
        });
        // The answer of a refused create, rename or delete: only the Changes tab
        // drew the notice, so here the prompt closed and nothing said why.
        if let Some((notice, true)) = &self.notice {
            notice_label(ui, notice, theme::colors().status_red);
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.file_filter)
                .font(theme::field_font(12.0))
                .hint_text(strings::WORKSPACE_FILE_FILTER)
                .desired_width(f32::INFINITY),
        );
        self.update_file_rows();
        // The list fills the panel when no preview is open, and always ends on
        // a whole row: a height that is not a multiple of the row height cut
        // the last row in half.
        let room = if self.file_preview.is_some() { ui.available_height() * 0.45 } else { ui.available_height() };
        let list_height = (room / FILE_ROW_HEIGHT).floor().max(1.0) * FILE_ROW_HEIGHT;
        let mut open_file: Option<String> = None;
        // Move the cache, not its contents: only visible rows are borrowed and
        // actions retain the original path even after a long scroll.
        let rows = std::mem::take(&mut self.file_rows);
        let spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt("workspace-files")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show_rows(ui, FILE_ROW_HEIGHT, rows.len(), |ui, visible| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for index in visible {
                    if let Some(path) = self.file_row(ui, &rows[index]) {
                        open_file = Some(path);
                    }
                }
            });
        ui.spacing_mut().item_spacing.y = spacing;
        self.file_rows = rows;
        if let Some(path) = open_file {
            self.busy = true;
            self.request(Request::ReadFile { path });
        }
        if let Some((path, text, truncated)) = self.file_preview.as_ref() {
            ui.add_space(2.0);
            theme::hairline(ui);
            ui.label(RichText::new(display(path, 240)).color(theme::colors().dim).font(theme::field_font(11.0)));
            // The preview ends on a whole line and takes the space the list
            // leaves: a height that is not a multiple of the line height cut
            // the last line in half, and the old half-of-the-rest cap left the
            // panel empty below (minus a hair, so no sliver shows).
            let line = ui.fonts_mut(|f| f.row_height(&theme::field_font(11.0)));
            let room = (ui.available_height() - 6.0).max(line);
            let height = (room / line).floor().max(1.0) * line - 1.0;
            let markdown = is_markdown(path);
            if markdown {
                self.markdown.prepare(ui, text);
                self.markdown.show(ui, height, path);
            } else {
                self.preview_wrapped.prepare(ui, text, &self.preview_rows);
                let spacing = ui.spacing().item_spacing.y;
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical()
                    .id_salt(("workspace-file-preview", path))
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show_rows(ui, TEXT_ROW_HEIGHT, self.preview_wrapped.rows.len(), |ui, visible| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for index in visible {
                            let row = &self.preview_wrapped.rows[index];
                            let (rect, _) = ui
                                .allocate_exact_size(Vec2::new(ui.available_width(), TEXT_ROW_HEIGHT), Sense::hover());
                            paint_text_row(ui, rect, &text[row.bytes.clone()], row.number, theme::colors().dim);
                        }
                    });
                ui.spacing_mut().item_spacing.y = spacing;
            }
            if *truncated {
                ui.label(
                    RichText::new(strings::WORKSPACE_FILE_TRUNCATED)
                        .color(theme::colors().status_yellow)
                        .font(theme::font(11.0)),
                );
            }
        }
    }

    fn counts(&self) -> (usize, usize) {
        let staged = self.status.changes.iter().filter(|c| c.staged()).count();
        let unstaged = self.status.changes.iter().filter(|c| c.unstaged()).count();
        (staged, unstaged)
    }

    /// Flattened rows: folder headers followed by their files, honoring
    /// `collapsed`.
    ///
    /// `status.changes` is sorted by full path, which interleaves directories —
    /// `src/a.rs`, `src/deep/c.rs`, `src/z.rs` puts `src` before and after
    /// `src/deep`. Grouping consecutive equal directories therefore drew the
    /// `src` header twice, once per run. Ordering by directory first makes each
    /// folder one run: one header, full count. The sort is stable, so files keep
    /// their path order inside a folder.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut changes: Vec<&Change> = self.status.changes.iter().collect();
        changes.sort_by(|a, b| a.directory().cmp(b.directory()));
        let mut index = 0;
        while index < changes.len() {
            let dir = changes[index].directory();
            let mut end = index;
            while end < changes.len() && changes[end].directory() == dir {
                end += 1;
            }
            if !dir.is_empty() {
                let depth = dir.matches('/').count();
                rows.push(Row::Folder { path: dir.to_owned(), depth, count: end - index });
                let collapsed = (0..=depth).any(|level| {
                    let prefix = dir.split('/').take(level + 1).collect::<Vec<_>>().join("/");
                    self.collapsed.contains(&prefix)
                });
                if collapsed {
                    index = end;
                    continue;
                }
            }
            for change in &changes[index..end] {
                rows.push(Row::File { change: (*change).clone() });
            }
            index = end;
        }
        rows
    }
}

pub enum WorkspaceAction {
    Close,
}

#[derive(Clone)]
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

struct TextRow {
    bytes: std::ops::Range<usize>,
    number: Option<u64>,
    hunk: Option<usize>,
    kind: PatchKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatchKind {
    Add,
    Remove,
    Hunk,
    Context,
}

impl PatchKind {
    /// `in_hunk` is false while the file header is being read, which is the
    /// only place a real `---`/`+++` pair appears. Inside a hunk those bytes
    /// are content, so a removed `---` markdown rule paints as a removal
    /// instead of silently losing its row.
    fn of(line: &str, in_hunk: bool) -> Self {
        if line.starts_with("@@") {
            Self::Hunk
        } else if !in_hunk {
            Self::Context
        } else if line.starts_with('+') {
            Self::Add
        } else if line.starts_with('-') {
            Self::Remove
        } else {
            Self::Context
        }
    }
}

type FontCacheKey = (u32, u32, usize);

// The empty galley stays in egui's layout cache while used. Atlas, font and
// text-option resets recreate that cache; retaining the Arc prevents pointer
// reuse from hiding a reset of the UV coordinates in cached Markdown.
fn font_cache_key(ui: &egui::Ui, width: f32) -> (FontCacheKey, std::sync::Arc<egui::Galley>) {
    let font_generation = ui.fonts_mut(|fonts| fonts.layout_job(egui::text::LayoutJob::default()));
    let key =
        (width.to_bits(), ui.ctx().pixels_per_point().to_bits(), std::sync::Arc::as_ptr(&font_generation) as usize);
    (key, font_generation)
}

#[derive(Default)]
struct WrappedRows {
    key: Option<FontCacheKey>,
    font_generation: Option<std::sync::Arc<egui::Galley>>,
    rows: Vec<TextRow>,
}

impl WrappedRows {
    fn prepare(&mut self, ui: &egui::Ui, text: &str, source: &[TextRow]) {
        let width = (ui.available_width()
            - GUTTER
            - 4.0
            - ui.spacing().scroll.bar_width
            - ui.spacing().scroll.bar_outer_margin)
            .max(20.0);
        let (key, font_generation) = font_cache_key(ui, width);
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.font_generation = Some(font_generation);
        self.rows.clear();
        let font = theme::field_font(11.0);
        ui.fonts_mut(|fonts| {
            let mut widths = std::collections::HashMap::new();
            for row in source {
                let mut start = row.bytes.start;
                let mut used = 0.0;
                let mut first = true;
                for (offset, ch) in text[row.bytes.clone()].char_indices() {
                    let at = row.bytes.start + offset;
                    let advance = if let Some(advance) = widths.get(&ch) {
                        *advance
                    } else {
                        let advance =
                            if ch == '\t' { fonts.glyph_width(&font, ' ') * 4.0 } else { fonts.glyph_width(&font, ch) };
                        widths.insert(ch, advance);
                        advance
                    };
                    if at > start && used + advance > width {
                        self.rows.push(TextRow {
                            bytes: start..at,
                            number: if first { row.number } else { None },
                            hunk: if first { row.hunk } else { None },
                            kind: row.kind,
                        });
                        first = false;
                        start = at;
                        used = 0.0;
                    }
                    used += advance;
                }
                self.rows.push(TextRow {
                    bytes: start..row.bytes.end,
                    number: if first { row.number } else { None },
                    hunk: if first { row.hunk } else { None },
                    kind: row.kind,
                });
            }
        });
    }
}

/// Byte ranges and gutter numbers are computed on response, never on scroll.
fn text_rows(text: &str, patch: bool) -> Vec<TextRow> {
    let mut numbers = PatchNumbers::default();
    let mut offset = 0;
    let mut hunk = 0;
    text.split_inclusive('\n')
        .enumerate()
        .map(|(index, raw)| {
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            let line = line.strip_suffix('\r').unwrap_or(line);
            let start = offset;
            offset += raw.len();
            let number = if patch { numbers.line(line) } else { Some(index as u64 + 1) };
            // `diff --cc` (a conflicted file) heads its hunks `@@@`: `parse_diff` has no
            // hunk behind those, so they are not offered as clickable hunks either.
            let hunk_index = if patch && line.starts_with("@@ ") {
                let index = hunk;
                hunk += 1;
                Some(index)
            } else {
                None
            };
            // Read the state after numbering: a `@@` line opens the hunk it names.
            let kind = if patch { PatchKind::of(line, numbers.started) } else { PatchKind::Context };
            TextRow { bytes: start..start + line.len(), number, hunk: hunk_index, kind }
        })
        .collect()
}

fn count_text_lines(reader: impl std::io::Read, cap: u64) -> std::io::Result<Option<u64>> {
    use std::io::Read;
    let mut reader = reader.take(cap.saturating_add(1));
    let mut buffer = [0_u8; 8192];
    let mut bytes = 0_u64;
    let mut lines = 0_u64;
    let mut last = None;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(Some(lines + u64::from(last.is_some_and(|byte| byte != b'\n'))));
        }
        bytes += count as u64;
        if bytes > cap || buffer[..count].contains(&0) {
            return Ok(None);
        }
        lines += buffer[..count].iter().filter(|byte| **byte == b'\n').count() as u64;
        last = Some(buffer[count - 1]);
    }
}

#[cfg(test)]
#[test]
fn text_line_counts_include_an_unterminated_final_line() {
    for (text, expected) in [("", 0), ("one", 1), ("one\n", 1), ("one\nlast", 2), ("\n\n", 2)] {
        assert_eq!(count_text_lines(text.as_bytes(), MAX_COUNTED_FILE).unwrap(), Some(expected));
    }
}

#[cfg(test)]
#[test]
fn streaming_line_count_rejects_late_nul_and_files_that_grow_past_the_cap() {
    let mut binary = vec![b'x'; 9000];
    binary.push(0);
    assert_eq!(count_text_lines(binary.as_slice(), MAX_COUNTED_FILE).unwrap(), None);
    assert_eq!(count_text_lines(&b"one\nlast"[..], 8).unwrap(), Some(2));
    assert_eq!(count_text_lines(&b"one\nlast!"[..], 8).unwrap(), None);
}

#[derive(Debug, PartialEq, Eq)]
pub struct DiffStamp {
    root: Option<PathBuf>,
    path: String,
    side: Option<bool>,
    head: Option<String>,
    change: Option<Change>,
    index: Option<(u64, Option<SystemTime>)>,
    file: Option<(u64, Option<SystemTime>)>,
}

impl DiffStamp {
    fn for_view(root: Option<&Path>, path: &str, side: Option<bool>, status: &Status) -> Self {
        Self {
            root: root.map(Path::to_owned),
            path: path.to_owned(),
            side,
            head: status.head_oid.clone(),
            change: status.changes.iter().find(|change| change.path == path).cloned(),
            index: None,
            file: None,
        }
    }

    #[cfg(test)]
    fn read(root: Option<&Path>, path: &str, side: Option<bool>, status: &Status) -> Self {
        Self::for_view(root, path, side, status).with_metadata()
    }

    fn with_metadata(mut self) -> Self {
        let stat = |full: Result<PathBuf, String>| {
            full.ok().and_then(|full| std::fs::metadata(full).ok()).map(|meta| (meta.len(), meta.modified().ok()))
        };
        self.index = self.root.as_deref().and_then(|root| stat(git::metadata_path(root, "index")));
        self.file = self.root.as_deref().and_then(|root| stat(git::resolve_path(root, &self.path)));
        self
    }
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

fn is_markdown(path: &str) -> bool {
    path.ends_with(".md") || path.ends_with(".markdown")
}

/// Rich Markdown rows retain their laid-out cells, with cumulative heights for
/// binary-searching the viewport. A table is split into rows, never one giant
/// off-screen widget.
#[derive(Default)]
struct MarkdownCache {
    key: Option<FontCacheKey>,
    font_generation: Option<std::sync::Arc<egui::Galley>>,
    rows: Vec<MarkdownRow>,
    height: f32,
}

struct MarkdownRow {
    top: f32,
    height: f32,
    cells: Vec<MarkdownCell>,
    header: bool,
    rule: bool,
}

struct MarkdownCell {
    x: f32,
    lines: Vec<MarkdownLine>,
}

struct MarkdownLine {
    x: f32,
    top: f32,
    bottom: f32,
    galley: Option<std::sync::Arc<egui::Galley>>,
}

impl MarkdownCell {
    fn new(x: f32, galley: std::sync::Arc<egui::Galley>, painter: &egui::Painter) -> Self {
        // A cached one-row galley avoids tessellating every wrapped line when only visible lines paint.
        let mut byte_offset = 0;
        let lines = galley
            .rows
            .iter()
            .map(|placed_row| {
                let row = &placed_row.row;
                let byte_end = byte_offset
                    + galley.job.text[byte_offset..].chars().take(row.glyphs.len()).map(char::len_utf8).sum::<usize>();
                let line_galley = (byte_offset < byte_end)
                    .then(|| painter.layout_job(markdown_row_job(&galley.job, byte_offset..byte_end)));
                byte_offset = byte_end;
                if placed_row.ends_with_newline && galley.job.text.as_bytes().get(byte_offset) == Some(&b'\n') {
                    byte_offset += 1;
                }
                MarkdownLine {
                    x: placed_row.pos.x,
                    top: placed_row.pos.y,
                    bottom: placed_row.pos.y + row.size.y,
                    galley: line_galley,
                }
            })
            .collect();
        Self { x, lines }
    }

    fn visible(&self, top: f32, bottom: f32) -> std::ops::Range<usize> {
        let start = self.lines.partition_point(|line| line.bottom <= top);
        let end = self.lines.partition_point(|line| line.top < bottom);
        start..end.max(start)
    }
}

fn markdown_row_job(source: &egui::text::LayoutJob, range: std::ops::Range<usize>) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.halign = source.halign;
    job.justify = source.justify;
    job.round_output_to_gui = source.round_output_to_gui;
    let first = source.sections.partition_point(|section| section.byte_range.end <= range.start);
    for section in source.sections[first..].iter().take_while(|section| section.byte_range.start < range.end) {
        let start = range.start.max(section.byte_range.start);
        let end = range.end.min(section.byte_range.end);
        if start < end {
            job.append(&source.text[start..end], 0.0, section.format.clone());
        }
    }
    job
}

impl MarkdownCache {
    fn push(
        &mut self,
        ui: &egui::Ui,
        cells: Vec<(f32, std::sync::Arc<egui::Galley>)>,
        padding: f32,
        header: bool,
        rule: bool,
    ) {
        let height = cells.iter().map(|(_, galley)| galley.size().y).fold(0.0, f32::max) + padding;
        let cells = cells.into_iter().map(|(x, galley)| MarkdownCell::new(x, galley, ui.painter())).collect();
        self.rows.push(MarkdownRow { top: self.height, height, cells, header, rule });
        self.height += height;
    }

    fn prepare(&mut self, ui: &egui::Ui, text: &str) {
        // Account for the solid vertical scrollbar before layout, so the
        // cached wrapping width is the viewport width, not the parent width.
        let width =
            (ui.available_width() - ui.spacing().scroll.bar_width - ui.spacing().scroll.bar_outer_margin).max(40.0);
        let (key, font_generation) = font_cache_key(ui, width);
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.font_generation = Some(font_generation);
        self.rows.clear();
        self.height = 0.0;
        let mut lines = text.lines().peekable();
        let mut fenced = false;
        while let Some(line) = lines.next() {
            let plain = line.trim();
            if plain.starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                let galley = ui.painter().layout(line.to_owned(), theme::field_font(11.0), theme::colors().dim, width);
                self.push(ui, vec![(0.0, galley)], 2.0, false, false);
            } else if plain.is_empty() {
                self.push(ui, Vec::new(), 6.0, false, false);
            } else if plain.starts_with('#') {
                let level = plain.chars().take_while(|c| *c == '#').count().min(6);
                let title = plain[level..].trim().trim_end_matches('#').trim();
                let size = match level {
                    1 => 15.0,
                    2 => 13.5,
                    _ => 12.5,
                };
                self.push(ui, Vec::new(), 6.0, false, false);
                let galley =
                    ui.painter().layout(title.to_owned(), theme::title_font(size), theme::colors().text, width);
                self.push(ui, vec![(0.0, galley)], 2.0, false, false);
            } else if matches!(plain, "---" | "***" | "___") {
                self.push(ui, Vec::new(), 6.0, false, true);
            } else if is_table_row(plain) && lines.peek().is_some_and(|next| is_table_separator(next)) {
                let mut rows = vec![table_cells(plain)];
                lines.next();
                while lines.peek().is_some_and(|next| is_table_row(next)) {
                    rows.push(table_cells(lines.next().unwrap_or_default()));
                }
                let columns = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let gap = 10.0_f32.min(width / (columns * 2) as f32);
                let column = ((width - gap * (columns - 1) as f32) / columns as f32).max(1.0);
                for (index, row) in rows.into_iter().enumerate() {
                    let colour = if index == 0 { theme::colors().text } else { theme::colors().dim };
                    let cells = row
                        .iter()
                        .enumerate()
                        .map(|(cell, text)| {
                            (
                                cell as f32 * (column + gap),
                                ui.painter().layout_job(inline_job(text, 11.5, colour, column)),
                            )
                        })
                        .collect();
                    self.push(ui, cells, 3.0, index == 0, true);
                }
                self.push(ui, Vec::new(), 6.0, false, false);
            } else {
                let (text, colour) = if let Some(rest) = plain.strip_prefix("> ") {
                    (format!("│ {rest}"), theme::colors().faint)
                } else if let Some(rest) =
                    plain.strip_prefix("- ").or_else(|| plain.strip_prefix("* ")).or_else(|| plain.strip_prefix("+ "))
                {
                    (format!("• {rest}"), theme::colors().dim)
                } else {
                    (plain.to_owned(), theme::colors().dim)
                };
                let galley = ui.painter().layout_job(inline_job(&text, 11.5, colour, width));
                self.push(ui, vec![(0.0, galley)], 2.0, false, false);
            }
        }
    }

    fn visible(&self, top: f32, bottom: f32) -> std::ops::Range<usize> {
        let start = self.rows.partition_point(|row| row.top + row.height <= top);
        let end = self.rows.partition_point(|row| row.top < bottom);
        start..end.max(start)
    }

    fn show(&self, ui: &mut egui::Ui, height: f32, path: &str) {
        ScrollArea::vertical()
            .id_salt(("workspace-markdown-preview", path))
            .max_height(height)
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let origin = ui.cursor().min;
                ui.set_min_height(self.height);
                for index in self.visible(viewport.min.y, viewport.max.y) {
                    let row = &self.rows[index];
                    let rect = Rect::from_min_size(
                        origin + Vec2::new(0.0, row.top),
                        Vec2::new(ui.available_width(), row.height),
                    );
                    if row.header {
                        ui.painter().rect_filled(rect, 0.0, theme::colors().tab_active_bg);
                    }
                    for cell in &row.cells {
                        let visible = cell.visible(viewport.min.y - row.top, viewport.max.y - row.top);
                        for index in visible {
                            let line = &cell.lines[index];
                            if let Some(galley) = &line.galley {
                                ui.painter().galley(
                                    rect.min + Vec2::new(cell.x + line.x, line.top),
                                    galley.clone(),
                                    theme::colors().dim,
                                );
                            }
                        }
                    }
                    if row.rule {
                        ui.painter().hline(rect.x_range(), rect.max.y - 1.0, Stroke::new(1.0, theme::colors().line));
                    }
                }
            });
    }
}

/// Inline Markdown of the panel: `code` spans get a chip, **bold** a brighter
/// tone. Anything else is left as written.
fn inline_job(text: &str, size: f32, colour: egui::Color32, width: f32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let plain = TextFormat { font_id: theme::font(size), color: colour, ..Default::default() };
    let bold = TextFormat {
        font_id: theme::font(size),
        color: theme::colors().text,
        extra_letter_spacing: 0.2,
        ..Default::default()
    };
    let code = TextFormat {
        font_id: theme::field_font(size - 0.5),
        color: theme::colors().text,
        background: theme::colors().tab_active_bg,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let mut cursor = 0;
    // The next marker of each kind, as an offset into `text`. One is searched for
    // again only once the cursor has passed it: searching both from the cursor at
    // every step rescans the whole remainder for the kind that never occurs, which
    // is quadratic on a long line of one kind (an 8 MiB Markdown line).
    let mut bold_at = text.find("**");
    let mut code_at = text.find('`');
    while cursor < text.len() {
        if bold_at.is_some_and(|at| at < cursor) {
            bold_at = text[cursor..].find("**").map(|at| at + cursor);
        }
        if code_at.is_some_and(|at| at < cursor) {
            code_at = text[cursor..].find('`').map(|at| at + cursor);
        }
        let bold_next = bold_at.map(|at| (at, "**", &bold));
        let code_next = code_at.map(|at| (at, "`", &code));
        let Some((at, marker, format)) = [bold_next, code_next].into_iter().flatten().min_by_key(|(at, _, _)| *at)
        else {
            job.append(&text[cursor..], 0.0, plain);
            break;
        };
        if at > cursor {
            job.append(&text[cursor..at], 0.0, plain.clone());
        }
        let opened = at + marker.len();
        match text[opened..].find(marker) {
            Some(end) => {
                job.append(&text[opened..opened + end], 0.0, format.clone());
                cursor = opened + end + marker.len();
            }
            None => {
                job.append(&text[at..], 0.0, plain);
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

/// Borrowed slices, not owned: a table is one `String` allocation per cell
/// otherwise, and a pathological line of `|` can be millions of cells. The
/// caller lays each cell up immediately and keeps only the galley.
fn table_cells(line: &str) -> Vec<&str> {
    line.trim().trim_matches('|').split('|').map(str::trim).collect()
}

/// One patch line: a right-aligned file line number in the gutter, then the
/// patch line itself, with the change band behind both — as in the reference
/// diff view. Added and removed lines get a full-width tinted band with the
/// line colour on top; colouring only the text is easy to miss.
fn patch_line(ui: &mut egui::Ui, line: &str, number: Option<u64>, kind: PatchKind) {
    // The bands must tile: any item spacing would show as a gap between lines.
    ui.spacing_mut().item_spacing.y = 0.0;
    let (band, colour) = match kind {
        PatchKind::Add => (Some(theme::colors().diff_add_bg), theme::colors().status_green),
        PatchKind::Remove => (Some(theme::colors().diff_remove_bg), theme::colors().status_red),
        PatchKind::Hunk => (None, theme::colors().diff_hunk),
        PatchKind::Context => (None, theme::colors().dim),
    };
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, TEXT_ROW_HEIGHT), Sense::hover());
    if let Some(band) = band {
        // Half-pixel bleed: two bands sharing a fractional edge would blend
        // with the background where they meet and read as a torn line.
        ui.painter().rect_filled(rect.expand2(Vec2::new(0.0, 0.5)), 0.0, band);
    }
    paint_text_row(ui, rect, line, number, colour);
}

fn paint_text_row(ui: &egui::Ui, rect: Rect, line: &str, number: Option<u64>, colour: egui::Color32) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if let Some(number) = number {
        painter.text(
            egui::Pos2::new(rect.min.x + GUTTER - 8.0, rect.min.y + 1.0),
            Align2::RIGHT_TOP,
            number.to_string(),
            theme::field_font(10.5),
            theme::colors().faint,
        );
    }
    let galley = painter.layout_no_wrap(line.to_owned(), theme::field_font(11.0), colour);
    painter.galley(egui::Pos2::new(rect.min.x + GUTTER, rect.min.y), galley, colour);
}

/// File line numbers of a patch: the old number for removals, the new one for
/// additions and context, reset by every `@@` header.
#[derive(Default)]
struct PatchNumbers {
    old: u64,
    new: u64,
    started: bool,
}

/// Header lines that cannot be confused with content: inside a hunk git prefixes
/// every line with a space, `+`, `-` or `\`, so a line beginning with any other
/// byte is unambiguously file metadata. `---`/`+++` are deliberately absent —
/// a removed line whose text starts with `--` is exactly `---`, and those are
/// content.
fn is_unambiguous_header(line: &str) -> bool {
    line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("new file")
        || line.starts_with("deleted file")
        || line.starts_with("old mode")
        || line.starts_with("new mode")
        || line.starts_with("similarity")
        || line.starts_with("rename ")
        || line.starts_with("copy ")
}

impl PatchNumbers {
    fn line(&mut self, line: &str) -> Option<u64> {
        if is_unambiguous_header(line) {
            // A new file section: the `---`/`+++` pair that follows is a
            // header again, so the next hunk must not read it as content.
            self.started = false;
            return None;
        }
        if line.starts_with("@@") {
            if let Some((old, new)) = parse_hunk(line) {
                self.old = old;
                self.new = new;
                self.started = true;
            }
            return None;
        }
        if line.starts_with("\\ No newline") {
            return None;
        }
        if !self.started {
            // Still in the header block: `---`/`+++` and anything else here
            // belongs to the file description, not to a numbered line.
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

/// A notice as the panel keeps and paints it. It is untrusted: git answers a
/// refused commit with its whole `git status` (names from the repository, up to
/// 8 MiB, unescaped bidi overrides), so control and bidi characters are dropped
/// and the size is bounded to a few lines of a few hundred characters. Applying
/// it twice changes nothing.
pub(crate) fn notice_text(text: &str) -> String {
    const LINES: usize = 6;
    const CHARS: usize = 300;
    let mut lines = text.lines();
    let mut kept: Vec<String> = lines.by_ref().take(LINES).map(|line| display(line, CHARS)).collect();
    if lines.next().is_some() {
        kept.push("…".to_owned());
    }
    kept.join("\n")
}

fn notice_label(ui: &mut egui::Ui, notice: &str, colour: egui::Color32) {
    ui.label(RichText::new(notice_text(notice)).color(colour).font(theme::font(11.5)));
}

fn is_format_control(ch: char) -> bool {
    matches!(ch, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// Truncates text to `max_width` with an ellipsis. The single implementation
/// lives in `chrome::tabbar`: this panel and the tab bar must not hold two
/// copies that can drift into showing different text.
///
/// The cut point is found by binary search over char boundaries, so the cost is
/// a handful of layouts rather than one per character. Width grows monotonically
/// with the prefix, which is what makes the search valid.
fn elide(painter: &egui::Painter, text: &str, font: egui::FontId, max_width: f32) -> String {
    crate::chrome::tabbar::elide(painter, text, font, max_width)
}

/// Elides the *front* of a path so the folder closest to the file stays
/// readable: `…/egui-default-fonts/`.
fn elide_front(painter: &egui::Painter, text: &str, font: egui::FontId, max_width: f32) -> String {
    let measure = |value: &str| painter.layout_no_wrap(value.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
    if measure(text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let fits = |count: usize| {
        let kept: String = chars[chars.len() - count..].iter().collect();
        measure(&format!("…{kept}")) <= max_width
    };
    let mut low = 0;
    let mut high = chars.len();
    while low < high {
        let mid = low + (high - low) / 2;
        if fits(mid) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    // `low` is the first suffix count that does NOT fit, not the last that
    // does, so keeping `low` characters overflows `max_width` by one — the
    // off-by-one `chrome::tabbar::elide` was fixed for.
    let kept: String = chars[chars.len() - low.saturating_sub(1)..].iter().collect();
    format!("…{kept}")
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
        'A' | '?' => theme::colors().status_green,
        'D' | 'U' => theme::colors().status_red,
        'M' | 'T' => theme::colors().status_yellow,
        'R' | 'C' => theme::colors().accent,
        _ => theme::colors().dim,
    }
}

fn copy_to_clipboard(text: &str) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text.to_owned());
    }
}

/// Shell-open only a small set of viewer formats with matching content.
/// Unknown extensions (including future script/installer types) reveal in
/// Explorer. Resolve before either action, using the mutation/preview guard.
fn external_open_target(root: &Path, path: &str) -> Result<(PathBuf, bool), String> {
    use std::io::Read;
    let full = git::resolve_path(root, path)?;
    let extension = full.extension().and_then(|extension| extension.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut prefix = [0_u8; 512];
    let safe = std::fs::File::open(&full)
        .ok()
        .and_then(|mut file| file.read(&mut prefix).ok())
        .is_some_and(|size| viewable_content(&extension, &prefix[..size]));
    Ok((full, safe))
}

fn viewable_content(extension: &str, prefix: &[u8]) -> bool {
    match extension {
        "png" => prefix.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => prefix.starts_with(b"\xff\xd8\xff"),
        "gif" => prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a"),
        "bmp" => prefix.starts_with(b"BM"),
        "webp" => prefix.starts_with(b"RIFF") && prefix.get(8..12) == Some(b"WEBP"),
        "pdf" => prefix.starts_with(b"%PDF-"),
        "txt" | "log" | "csv" | "md" | "markdown" => {
            !prefix.starts_with(b"MZ")
                && !prefix.starts_with(b"\x7fELF")
                && !prefix.starts_with(b"#!")
                && !prefix.contains(&0)
                && std::str::from_utf8(prefix).map_or_else(|error| error.error_len().is_none(), |_| true)
        }
        _ => false,
    }
}

/// Which launch a click on a repository file means. The worker runs this (it
/// canonicalizes the path and reads the first bytes of the file); the UI thread
/// only starts the result. A viewer format with matching content opens;
/// everything else, scripts and programs included, is revealed in Explorer.
fn external_launch(root: &Path, path: &str, reveal_only: bool) -> Result<Launch, String> {
    if reveal_only {
        return git::resolve_path(root, path).map(Launch::Reveal);
    }
    let (full, safe) = external_open_target(root, path)?;
    Ok(if safe { Launch::Open(full) } else { Launch::Reveal(full) })
}

/// Every hunk of a diff in the order of its `@@` rows, each as `git::hunk_bytes`
/// reads it. One pass over the diff: every hunk is copied once, from its own
/// lines (rebuilding a whole patch per hunk was quadratic in the hunk count).
fn diff_hunk_bytes(diff: &[u8], staged: bool) -> Vec<Vec<u8>> {
    let files = git::parse_diff(diff, staged);
    files.iter().flat_map(|file| (0..file.hunks.len()).filter_map(|index| git::hunk_bytes(file, index))).collect()
}

fn reveal_resolved(full: &Path) {
    let mut command = std::process::Command::new("explorer.exe");
    command.arg(format!("/select,{}", full.display()));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let _ = command.spawn();
}

/// The actual AI path reads only index content and index file/status context.
/// Errors and an empty index never start a CLI or silently stage anything.
fn staged_ai_prompt(root: &Path) -> Result<String, String> {
    let status =
        git::run_git(root, &["diff", "--cached", "--name-status", "--no-ext-diff", "--no-textconv", "--no-color"])?;
    if status.trim().is_empty() {
        return Err(strings::WORKSPACE_AI_NO_STAGE.to_owned());
    }
    let staged =
        git::run_git(root, &["diff", "--cached", "--no-ext-diff", "--no-textconv", "--no-color", "--unified=1"])?;
    let recent = git::recent_subjects(root, 8);
    Ok(git::ai_prompt(&status, staged.trim(), &recent))
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
            if request.requires_current_repository() && expected_root != root {
                let error = strings::WORKSPACE_REPO_CHANGED.to_owned();
                send(match request {
                    Request::AiMessage { .. } => Response::AiMessage(Err(error)),
                    Request::Commit { .. } => Response::Committed(Err(error)),
                    Request::OpenExternal { .. } | Request::Reveal { .. } => Response::Launch(None),
                    _ => Response::Error(error),
                });
                continue;
            }
            if let Request::Refresh { cwd: new_cwd } = &request {
                cwd = Some(new_cwd.clone());
            }
            // Only root/config inspection is permitted before explicit trust.
            // Re-resolve on refresh/approval, including a shell changing repos.
            let identity = if matches!(request, Request::Refresh { .. } | Request::Approve { .. }) {
                cwd.as_ref().map_or(Ok(None), |cwd| git::repository_identity(cwd))
            } else {
                root.as_ref().map_or(Ok(None), |root| {
                    git::repository_stamp(root).map(|stamp| Some(git::RepositoryIdentity { root: root.clone(), stamp }))
                })
            };
            let identity = match identity {
                Ok(identity) => identity,
                Err(error) => {
                    send(Response::TrustRequired { identity: None, error: Some(error) });
                    continue;
                }
            };
            if matches!(request, Request::Refresh { .. }) {
                root = identity.as_ref().map(|identity| identity.root.clone());
            }
            if let Request::Approve { identity: shown } = &request {
                let current_matches = identity.as_ref().is_some_and(|current| {
                    current.root == shown.root
                        && current.stamp == shown.stamp
                        && root.as_ref() == Some(&shown.root)
                        && expected_root.as_ref() == Some(&shown.root)
                });
                if !current_matches {
                    root = identity.as_ref().map(|identity| identity.root.clone());
                    send(Response::TrustRequired { identity, error: Some(strings::WORKSPACE_TRUST_STALE.to_owned()) });
                    continue;
                }
                if let Some(current) = &identity {
                    git::remember_trust(&current.root, current.stamp.clone());
                }
                send(Response::Trusted(identity.as_ref().unwrap().clone()));
            }
            // Hazard-free repositories never ask; a hazardous digest needs an
            // approval that every pane and window of this process shares.
            if let Some(current) = &identity {
                if !git::trust_approved(&current.root, &current.stamp) {
                    send(Response::TrustRequired { identity, error: None });
                    continue;
                }
            }
            match request {
                Request::Refresh { .. } | Request::Approve { .. } => match &root {
                    Some(root) => match git::status(root) {
                        Ok(status) => send(Response::Status(status, Some(root.clone()))),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Status(Status::default(), None)),
                },
                Request::Diff { path, side } => match &root {
                    Some(root) => {
                        // A failed git must reach the panel: an empty diff reads
                        // as "this file has no changes".
                        let (bytes, from_index) = match side {
                            Some(staged) => match git::diff_bytes(root, &path, staged) {
                                Ok(bytes) => (bytes, staged),
                                Err(e) => {
                                    send(Response::Error(e));
                                    continue;
                                }
                            },
                            None => match git::diff_bytes(root, &path, true) {
                                Ok(staged) if staged.trim_ascii().is_empty() => {
                                    match git::diff_bytes(root, &path, false) {
                                        Ok(bytes) => (bytes, false),
                                        Err(e) => {
                                            send(Response::Error(e));
                                            continue;
                                        }
                                    }
                                }
                                Ok(staged) => (staged, true),
                                Err(e) => {
                                    send(Response::Error(e));
                                    continue;
                                }
                            },
                        };
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        let rows = text_rows(&text, true);
                        let hunks = diff_hunk_bytes(&bytes, from_index);
                        send(Response::Diff { path, text, rows, hunks, staged: from_index, side });
                    }
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::CheckDiff { stamp } => {
                    if stamp.root != root {
                        send(Response::Error(strings::WORKSPACE_REPO_CHANGED.to_owned()));
                    } else {
                        send(Response::DiffChecked(stamp.with_metadata()));
                    }
                }
                Request::Stage { paths, staged } => match &root {
                    Some(root) => match git::stage(root, &paths, staged) {
                        Ok(()) => send(Response::Refreshed),
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::Commit { message } => match &root {
                    Some(root) => send(Response::Committed(git::commit(root, message))),
                    None => send(Response::Committed(Err(strings::WORKSPACE_NO_REPO.to_owned()))),
                },
                Request::AiMessage { command } => {
                    if root.is_none() {
                        send(Response::AiMessage(Err(strings::WORKSPACE_NO_REPO.to_owned())));
                        continue;
                    }
                    let result = root.as_ref().ok_or_else(|| strings::WORKSPACE_NO_REPO.to_owned()).and_then(|root| {
                        let prompt = staged_ai_prompt(root)?;
                        git::ai_commit_message(command.as_deref(), &prompt, git::AI_TIMEOUT)
                    });
                    // An AI subprocess can outlive a configuration change; never
                    // publish its result using approval for the old digest.
                    let current = root.as_ref().ok_or_else(|| strings::WORKSPACE_NO_REPO.to_owned()).and_then(|root| {
                        git::repository_stamp(root).map(|stamp| git::RepositoryIdentity { root: root.clone(), stamp })
                    });
                    match current {
                        Ok(identity) if git::trust_approved(&identity.root, &identity.stamp) => {
                            send(Response::AiMessage(result));
                        }
                        Ok(identity) => {
                            send(Response::TrustRequired {
                                identity: Some(identity),
                                error: Some(strings::WORKSPACE_TRUST_STALE.to_owned()),
                            });
                        }
                        Err(error) => {
                            send(Response::TrustRequired { identity: None, error: Some(error) });
                        }
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
                Request::CommitDiff { hash, path } => {
                    let patch = if expected_root != root {
                        Err(strings::WORKSPACE_REPO_CHANGED.to_owned())
                    } else {
                        root.as_ref()
                            .ok_or_else(|| strings::WORKSPACE_NO_REPO.to_owned())
                            .and_then(|root| git::commit_file_diff(root, &hash, &path))
                            .map(|patch| {
                                let rows = text_rows(&patch, true);
                                (patch, rows)
                            })
                    };
                    send(Response::CommitDiff { hash, path, patch });
                }
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
                            let resolver = git::PathResolver::new(root);
                            for path in &files {
                                let Ok(resolver) = &resolver else { break };
                                let Ok(full) = resolver.resolve(path) else { continue };
                                let Ok(meta) = std::fs::metadata(&full) else { continue };
                                // Huge and binary files would only slow the walk down.
                                if !meta.is_file() || meta.len() > MAX_COUNTED_FILE {
                                    continue;
                                }
                                let Ok(file) = std::fs::File::open(full) else { continue };
                                if let Ok(Some(count)) = count_text_lines(file, MAX_COUNTED_FILE) {
                                    lines += count;
                                    counted += 1;
                                }
                            }
                            send(Response::LineCount { files: counted, lines });
                        }
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::ReadFile { path } => match &root {
                    Some(root) => match git::read_file(root, &path, MAX_PREVIEW_BYTES) {
                        Ok((text, truncated)) => {
                            // Markdown is laid out by `MarkdownCache`, which
                            // needs the text but no per-line rows.
                            let rows = if is_markdown(&path) { Vec::new() } else { text_rows(&text, false) };
                            send(Response::FileText { path, text, rows, truncated });
                        }
                        Err(e) => send(Response::Error(e)),
                    },
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::ApplyHunks { path, index, seen, from_index } => match &root {
                    Some(root) => {
                        let result = git::diff_bytes(root, &path, from_index).and_then(|bytes| {
                            let files = git::parse_diff(&bytes, from_index);
                            files
                                .iter()
                                .find(|file| file.path == path)
                                .ok_or_else(|| strings::WORKSPACE_NO_CHANGES_FOR_FILE.to_owned())
                                .and_then(|file| match git::hunk_bytes(file, index) {
                                    // The file may have changed since it was shown: apply
                                    // only the hunk the user actually saw, whole. The `@@`
                                    // line alone repeats when a line is replaced by another.
                                    Some(current) if current == seen => {
                                        git::apply_hunks(root, file, &[index], from_index)
                                    }
                                    _ => Err(strings::WORKSPACE_DIFF_STALE.to_owned()),
                                })
                        });
                        match result {
                            Ok(()) => send(Response::Applied),
                            Err(e) => send(Response::Error(e)),
                        }
                    }
                    None => send(Response::Error(strings::WORKSPACE_NO_REPO.to_owned())),
                },
                Request::OpenExternal { path } => {
                    send(Response::Launch(root.as_deref().and_then(|root| external_launch(root, &path, false).ok())));
                }
                Request::Reveal { path } => {
                    send(Response::Launch(root.as_deref().and_then(|root| external_launch(root, &path, true).ok())));
                }
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
        // `create_new` is the check and the create: a file another process put
        // here between an `exists()` and a `write()` is left alone instead of
        // being silently truncated to zero bytes.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&full) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(strings::WORKSPACE_FILE_EXISTS.to_owned()),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn rename_path(root: &Path, from: &str, to: &str) -> Result<(), String> {
    rename_path_checked(root, from, to, || {})
}

fn rename_path_checked(root: &Path, from: &str, to: &str, before_move: impl FnOnce()) -> Result<(), String> {
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
    before_move();
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let source: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // No REPLACE_EXISTING: a target created after the check wins atomically.
        // SAFETY: both buffers contain complete NUL-terminated local paths.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        // `std::fs::rename` replaces an existing target, which the check above
        // cannot rule out atomically; only the Windows move above is safe.
        Err("Renaming is only supported on Windows".to_owned())
    }
}

fn delete_path(root: &Path, path: &str) -> Result<(), String> {
    let full = git::resolve_path(root, path)?;
    crate::fsutil::recycle_path(&full)
}

/// What to hand to `git restore --staged` for these changes. A staged rename is
/// two index entries, the new name added and the old one deleted: restoring only
/// the new name leaves `D  old` staged next to an untracked new file.
fn unstage_paths<'a>(changes: impl Iterator<Item = &'a Change>) -> Vec<String> {
    let mut paths = Vec::new();
    for change in changes {
        paths.push(change.path.clone());
        if change.index == 'R' {
            paths.extend(change.original_path.clone());
        }
    }
    paths
}

/// Width clamped to the panel bounds.
pub fn clamp_width(width: f32) -> f32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The commit list is virtualized by row index, so the flattened rows must
    /// carry every commit in order under a header per section run.
    #[test]
    fn commit_rows_carry_each_commit_under_one_header_per_section() {
        let commit = |section, hash: &str| git::Commit {
            hash: hash.to_owned(),
            short: hash[..4].to_owned(),
            parents: Vec::new(),
            author: "a".to_owned(),
            subject: hash.to_owned(),
            refs: Vec::new(),
            time: 0,
            section,
        };
        let log = CommitLog {
            commits: vec![
                commit(git::Section::Outgoing, "aaaa111"),
                commit(git::Section::Outgoing, "bbbb222"),
                commit(git::Section::Incoming, "cccc333"),
                commit(git::Section::History, "dddd444"),
                commit(git::Section::History, "eeee555"),
                commit(git::Section::History, "ffff666"),
            ],
            ..Default::default()
        };
        let rows = commit_rows_of(&log);
        let indexes: Vec<usize> = rows
            .iter()
            .filter_map(|row| match row {
                CommitRow::Commit { index } => Some(*index),
                CommitRow::Header { .. } => None,
            })
            .collect();
        assert_eq!(indexes, [0, 1, 2, 3, 4, 5], "every commit must appear exactly once, in order");
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|row| match row {
                CommitRow::Header { label } => Some(*label),
                CommitRow::Commit { .. } => None,
            })
            .collect();
        assert_eq!(headers.len(), 3, "one header per section run, not one per commit");
        assert!(commit_rows_of(&CommitLog::default()).is_empty());
    }

    #[test]
    fn row_ranges_preserve_unicode_crlf_numbers_and_last_line() {
        let text = "diff --git a/f b/f\r\n@@ -10,2 +20,3 @@\r\n-old\r\n+новый\r\n context\r\n+last";
        let rows = text_rows(text, true);
        assert_eq!(
            rows.iter().map(|row| &text[row.bytes.clone()]).collect::<Vec<_>>(),
            ["diff --git a/f b/f", "@@ -10,2 +20,3 @@", "-old", "+новый", " context", "+last"]
        );
        assert_eq!(
            rows.iter().map(|row| row.number).collect::<Vec<_>>(),
            [None, None, Some(10), Some(20), Some(21), Some(22)]
        );
        assert_eq!(rows[1].hunk, Some(0));
        let plain = "first\n\nпоследний";
        let rows = text_rows(plain, false);
        assert_eq!(rows.last().unwrap().number, Some(3));
        assert_eq!(&plain[rows.last().unwrap().bytes.clone()], "последний");
    }

    #[test]
    fn file_cache_navigation_and_refresh_keep_original_paths() {
        let (tx, rx) = mpsc::channel();
        let mut workspace = Workspace { rx: Some(rx), ..Default::default() };
        let files: Vec<_> = (0..50_001).map(|index| format!("src/file-{index:05}.txt")).collect();
        tx.send(Response::Files(files.clone())).unwrap();
        workspace.absorb();
        workspace.update_file_rows();
        assert_eq!(workspace.file_rows.len(), 1, "folded tree must not expose descendants");
        workspace.toggle_file_folder("src");
        workspace.update_file_rows();
        assert_eq!(workspace.file_rows.len(), files.len() + 1);
        assert_eq!(workspace.file_rows.last().unwrap().path, "src/file-50000.txt");
        workspace.update_file_rows();
        assert_eq!(
            workspace.file_rows.last().unwrap().path,
            "src/file-50000.txt",
            "idle cache preserves the last navigable row"
        );
        workspace.file_filter = "file-50000".to_owned();
        workspace.update_file_rows();
        assert_eq!(workspace.file_rows[0].path, "src/file-50000.txt", "search actions use the full original path");
        tx.send(Response::Files(vec!["renamed.txt".to_owned()])).unwrap();
        workspace.absorb();
        workspace.update_file_rows();
        assert!(workspace.file_rows.is_empty(), "removed search hits must not survive refresh");
        workspace.file_filter.clear();
        workspace.update_file_rows();
        assert_eq!(workspace.file_rows[0].path, "renamed.txt");
    }

    #[test]
    fn unchanged_poll_keeps_diff_and_changed_metadata_reloads_it() {
        let dir = tempfile::tempdir().unwrap();
        if !git(dir.path(), &["init", "--quiet"]) {
            return;
        }
        std::fs::write(dir.path().join("f.txt"), "old\n").unwrap();
        let status = Status { branch: "main".to_owned(), changes: vec![changed_file("f.txt")], ..Default::default() };
        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let mut workspace = Workspace {
            root: Some(dir.path().to_owned()),
            status: status.clone(),
            diff_path: Some("f.txt".to_owned()),
            diff_stamp: Some(DiffStamp::read(Some(dir.path()), "f.txt", None, &status)),
            log_status_seen: true,
            tx: Some(request_tx),
            rx: Some(response_rx),
            ..Default::default()
        };
        response_tx.send(Response::Status(status.clone(), workspace.root.clone())).unwrap();
        workspace.absorb();
        let stamp = DiffStamp::read(workspace.root.as_deref(), "f.txt", None, &status);
        response_tx.send(Response::DiffChecked(stamp)).unwrap();
        workspace.absorb();
        assert!(!request_rx.try_iter().any(|(_, request)| matches!(request, Request::Diff { .. })));
        std::fs::write(dir.path().join("f.txt"), "new content, same status letters\n").unwrap();
        response_tx.send(Response::Status(status.clone(), workspace.root.clone())).unwrap();
        workspace.absorb();
        let stamp = DiffStamp::read(workspace.root.as_deref(), "f.txt", None, &status);
        response_tx.send(Response::DiffChecked(stamp)).unwrap();
        workspace.absorb();
        assert!(request_rx.try_iter().any(|(_, request)| matches!(request, Request::Diff { .. })));
        let diff = "@@ -1 +1 @@\n-old\n+new";
        response_tx
            .send(Response::Diff {
                path: "f.txt".to_owned(),
                text: diff.to_owned(),
                rows: text_rows(diff, true),
                hunks: Vec::new(),
                staged: false,
                side: None,
            })
            .unwrap();
        workspace.absorb();
        assert_eq!(&workspace.diff_text[workspace.diff_rows.last().unwrap().bytes.clone()], "+new");
        assert!(git(dir.path(), &["add", "--", "f.txt"]));
        response_tx.send(Response::Status(status.clone(), workspace.root.clone())).unwrap();
        workspace.absorb();
        let stamp = DiffStamp::read(workspace.root.as_deref(), "f.txt", None, &status);
        response_tx.send(Response::DiffChecked(stamp)).unwrap();
        workspace.absorb();
        assert!(
            request_rx.try_iter().any(|(_, request)| matches!(request, Request::Diff { .. })),
            "an externally changed index invalidates the open diff"
        );
        response_tx.send(Response::Applied).unwrap();
        response_tx.send(Response::Status(status.clone(), workspace.root.clone())).unwrap();
        workspace.absorb();
        let stamp = DiffStamp::read(workspace.root.as_deref(), "f.txt", None, &status);
        response_tx.send(Response::DiffChecked(stamp)).unwrap();
        workspace.absorb();
        assert!(
            request_rx.try_iter().any(|(_, request)| matches!(request, Request::Diff { .. })),
            "explicit staging invalidates even unchanged metadata"
        );
        workspace.diff_side = Some(true);
        response_tx
            .send(Response::Diff {
                path: "f.txt".to_owned(),
                text: "stale worktree".to_owned(),
                rows: Vec::new(),
                hunks: Vec::new(),
                staged: false,
                side: Some(false),
            })
            .unwrap();
        workspace.absorb();
        assert!(workspace.diff_text.ends_with("+new"), "late worktree response cannot replace a selected index diff");
    }

    #[test]
    fn multi_megabyte_preview_reads_and_maps_its_final_line() {
        let repository = tempfile::tempdir().unwrap();
        if !git(repository.path(), &["init", "--quiet"]) {
            return;
        }
        let text = format!(
            "{}tail marker\n",
            "preview row with sufficient width for realistic workload........\n".repeat(120_000)
        );
        assert!(text.len() > 7 * 1024 * 1024 && text.len() < MAX_PREVIEW_BYTES);
        std::fs::write(repository.path().join("large.txt"), &text).unwrap();
        let (tx, rx) = spawn_worker();
        tx.send((None, Request::Refresh { cwd: repository.path().to_owned() })).unwrap();
        let shown = approve_worker(&tx, &rx);
        tx.send((shown.clone(), Request::ReadFile { path: "large.txt".to_owned() })).unwrap();
        let response = rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let Response::FileText { text: loaded, truncated, .. } = &response else {
            panic!("real preview response expected")
        };
        assert!(!truncated && loaded == &text, "supported multi-megabyte previews must not silently stop at 512 KiB");
        let (response_tx, response_rx) = mpsc::channel();
        let mut workspace = Workspace { rx: Some(response_rx), ..Default::default() };
        response_tx.send(response).unwrap();
        workspace.absorb();
        let loaded = &workspace.file_preview.as_ref().unwrap().1;
        let last = workspace.preview_rows.last().unwrap();
        assert_eq!(&loaded[last.bytes.clone()], "tail marker");
        assert_eq!(last.number, Some(120_001));
        std::fs::write(repository.path().join("large.txt"), vec![b'x'; MAX_PREVIEW_BYTES + 1]).unwrap();
        tx.send((shown, Request::ReadFile { path: "large.txt".to_owned() })).unwrap();
        let Response::FileText { text, truncated, .. } = rx.recv_timeout(Duration::from_secs(20)).unwrap() else {
            panic!("real capped response expected")
        };
        assert!(truncated && text.len() == MAX_PREVIEW_BYTES, "files beyond the cap still report truncation");
    }

    #[test]
    fn wrapped_rows_preserve_text_and_hunk_identity_across_resizes() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let text = "@@ -10,1 +10,1 @@ long header with context\n+длинная строка repeated repeated repeated repeated";
        let source = text_rows(text, true);
        let mut cache = WrappedRows::default();
        for width in [180.0, 700.0] {
            let _ = ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    ui.set_max_width(width);
                    cache.prepare(ui, text, &source);
                    assert_eq!(
                        cache.rows.iter().map(|row| &text[row.bytes.clone()]).collect::<String>(),
                        text.replace('\n', "")
                    );
                    assert_eq!(cache.rows.iter().filter(|row| row.hunk == Some(0)).count(), 1);
                    assert_eq!(cache.rows.iter().filter(|row| row.number == Some(10)).count(), 1);
                    assert!(cache.rows.last().unwrap().bytes.end == text.len());
                    cache.prepare(ui, text, &source);
                    assert_eq!(cache.rows.last().unwrap().kind, PatchKind::Add);
                });
            });
        }
    }

    #[test]
    fn rich_markdown_viewport_reaches_last_block_and_keeps_inline_formats() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let text = format!(
            "# Heading\n| Name | Value |\n| --- | --- |\n| **bold** | `code` |\n{}\nlast marker",
            "paragraph with wrapping and **bold** text\n".repeat(2_000)
        );
        let mut cache = MarkdownCache::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.set_max_width(300.0);
                cache.prepare(ui, &text);
                assert!(cache.rows.iter().any(|row| row.header && row.cells.len() == 2));
                assert!(
                    cache
                        .rows
                        .iter()
                        .flat_map(|row| &row.cells)
                        .flat_map(|cell| &cell.lines)
                        .filter_map(|line| line.galley.as_ref())
                        .flat_map(|galley| &galley.job.sections)
                        .any(|section| section.format.background == theme::colors().tab_active_bg),
                    "inline code retains its rich background"
                );
                let last = cache.rows.last().unwrap();
                assert_eq!(last.cells[0].lines[0].galley.as_ref().unwrap().text(), "last marker");
                let visible = cache.visible(last.top, cache.height);
                assert!(visible.contains(&(cache.rows.len() - 1)));
                assert!(visible.len() < 10, "scrolling to the end does not render the whole document");
                cache.prepare(ui, &text);
                assert_eq!(cache.rows.last().unwrap().cells[0].lines[0].galley.as_ref().unwrap().text(), "last marker");
            });
        });
    }

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
            log: CommitLog {
                commits: vec![commit("0123456789", git::Section::History)],
                upstream: None,
                truncated: false,
            },
            files: vec!["old.txt".to_owned()],
            file_preview: Some(("old.txt".to_owned(), "text".to_owned(), false)),
            detail: Some(("0123456789".to_owned(), git::CommitDetail { files: Vec::new(), header: String::new() })),
            ..Default::default()
        };
        workspace.selected.insert("old.txt".to_owned());
        workspace.prompt = Some(Prompt {
            kind: PromptKind::Delete { path: "src".to_owned(), folder: true },
            text: String::new(),
            focus: false,
        });

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
        assert!(
            requests.iter().any(|request| matches!(request, Request::Log)),
            "the new repository must be read again"
        );
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

    /// Approves the repository when its configuration is hazardous and returns
    /// the root. Hazard-free fixtures reply with status immediately: nothing in
    /// them can run a program, so the panel never asks.
    fn approve_worker(tx: &Sender<Envelope>, rx: &Receiver<Response>) -> Option<PathBuf> {
        match rx.recv_timeout(Duration::from_secs(20)).expect("worker response") {
            Response::TrustRequired { identity: Some(identity), error: None } => {
                let shown = Some(identity.root.clone());
                tx.send((shown.clone(), Request::Approve { identity })).unwrap();
                assert!(matches!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), Response::Trusted(_)));
                assert!(matches!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), Response::Status(_, _)));
                shown
            }
            Response::Status(_, root) => root,
            _ => panic!("the worker must either trust the repository or report its status"),
        }
    }

    #[test]
    fn push_and_fetch_refresh_commit_sections() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        let remote = dir.path().join("remote.git");
        let peer = dir.path().join("peer");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&remote).unwrap();
        if !git(&root, &["init", "--quiet", "--initial-branch=main"]) {
            eprintln!("git is not installed; skipping");
            return;
        }
        assert!(git(&remote, &["init", "--quiet", "--bare", "--initial-branch=main"]));
        assert!(
            git(&root, &["config", "user.email", "anvil@test"]) && git(&root, &["config", "user.name", "ANVIL test"])
        );
        assert!(git(&root, &["remote", "add", "origin", remote.to_str().unwrap()]));
        std::fs::write(root.join("file.txt"), "base\n").unwrap();
        assert!(git(&root, &["add", "file.txt"]) && git(&root, &["commit", "--quiet", "-m", "base"]));
        assert!(git(&root, &["push", "--quiet", "--set-upstream", "origin", "main"]));
        std::fs::write(root.join("file.txt"), "base\nlocal\n").unwrap();
        assert!(git(&root, &["add", "file.txt"]) && git(&root, &["commit", "--quiet", "-m", "local change"]));

        // Apply real worker responses one at a time to exercise the same
        // response/poll lifecycle as the UI without sleeps or remote services.
        let (worker_tx, worker_rx) = spawn_worker();
        let (response_tx, response_rx) = mpsc::channel();
        let mut workspace = Workspace { tx: Some(worker_tx), rx: Some(response_rx), ..Default::default() };
        let apply_response = |workspace: &mut Workspace| {
            response_tx.send(worker_rx.recv_timeout(Duration::from_secs(20)).expect("git worker response")).unwrap();
            workspace.absorb();
        };
        // The UI polls only once the panel has no request left in flight, which
        // is what stops a slow repository from queueing a refresh per interval.
        let settle = |workspace: &mut Workspace| {
            let mut guard = 0;
            while workspace.inflight > 0 {
                apply_response(workspace);
                guard += 1;
                assert!(guard < 32, "the panel must settle, not keep chaining requests");
            }
        };
        workspace.poll(root.clone());
        apply_response(&mut workspace);
        if let Some(identity) = workspace.pending_identity.clone() {
            workspace.request(Request::Approve { identity });
        }
        while workspace.log.commits.is_empty() {
            apply_response(&mut workspace);
        }
        let local_hash = workspace.log.commits[0].hash.clone();
        assert_eq!(workspace.status.ahead, 1);
        assert_eq!(workspace.log.commits[0].section, git::Section::Outgoing);
        assert!(!workspace.log.commits[0].refs.iter().any(|reference| reference == "origin/main"));

        workspace.busy = true;
        workspace.request(Request::Push);
        while workspace.notice.is_none() {
            apply_response(&mut workspace);
        }
        assert!(!workspace.notice.as_ref().unwrap().1, "{:?}", workspace.notice);
        settle(&mut workspace);
        workspace.poll(root.clone());
        while workspace.status.ahead != 0 {
            apply_response(&mut workspace);
        }
        let pushed = workspace.log.commits.iter().find(|commit| commit.hash == local_hash).unwrap();
        assert_eq!(pushed.section, git::Section::History, "pushed commits must leave the outgoing section");
        assert!(
            pushed.refs.iter().any(|reference| reference == "origin/main"),
            "the remote label must move to the pushed tip"
        );

        assert!(git(dir.path(), &["clone", "--quiet", remote.to_str().unwrap(), peer.to_str().unwrap()]));
        assert!(
            git(&peer, &["config", "user.email", "anvil@test"]) && git(&peer, &["config", "user.name", "ANVIL test"])
        );
        std::fs::write(peer.join("file.txt"), "base\nlocal\nremote\n").unwrap();
        assert!(git(&peer, &["add", "file.txt"]) && git(&peer, &["commit", "--quiet", "-m", "remote change"]));
        assert!(git(&peer, &["push", "--quiet"]));
        workspace.notice = None;
        workspace.busy = true;
        workspace.request(Request::Fetch);
        while workspace.notice.is_none() {
            apply_response(&mut workspace);
        }
        assert!(!workspace.notice.as_ref().unwrap().1, "{:?}", workspace.notice);
        settle(&mut workspace);
        workspace.poll(root.clone());
        while workspace.status.behind != 1 {
            apply_response(&mut workspace);
        }
        let incoming = &workspace.log.commits[0];
        assert_eq!(incoming.subject, "remote change");
        assert_eq!(incoming.section, git::Section::Incoming);
        assert!(incoming.refs.iter().any(|reference| reference == "origin/main"));
        let local = workspace.log.commits.iter().find(|commit| commit.hash == local_hash).unwrap();
        assert_eq!(local.section, git::Section::History);
        assert!(
            !local.refs.iter().any(|reference| reference == "origin/main"),
            "fetch must move the remote label to the incoming tip"
        );
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
        assert!(
            git(root, &["config", "user.email", "anvil@test"]) && git(root, &["config", "user.name", "ANVIL test"])
        );
        std::fs::write(root.join("f.txt"), "one\n").unwrap();
        assert!(git(root, &["add", "f.txt"]) && git(root, &["commit", "--quiet", "-m", "init"]));
        std::fs::write(root.join("f.txt"), "one\nstaged\n").unwrap();
        assert!(git(root, &["add", "f.txt"]));
        std::fs::write(root.join("f.txt"), "one\nstaged\nworktree\n").unwrap();

        let (tx, rx) = spawn_worker();
        let wait = || rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a worker response");
        tx.send((None, Request::Refresh { cwd: root.to_path_buf() })).unwrap();
        let shown = approve_worker(&tx, &rx);
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
        assert!(
            !staged && text.contains("+worktree") && !text.contains("+staged"),
            "the worktree side on request: {text}"
        );
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
        assert!(approve_worker(&tx, &rx).is_some(), "the folder is a repository");

        let elsewhere = Some(PathBuf::from("C:/some/other/repo"));
        tx.send((elsewhere, Request::DeletePath { path: "keep.txt".to_owned() })).unwrap();
        match wait() {
            Response::Error(message) => assert_eq!(message, strings::WORKSPACE_REPO_CHANGED),
            _ => panic!("the delete must be refused"),
        }
        assert!(dir.path().join("keep.txt").exists(), "nothing was deleted");
    }

    /// Eliding must never hand back something wider than the space it was
    /// given: the file name is drawn into whatever the elided folder leaves.
    /// The binary search finds the first suffix count that does *not* fit, so
    /// keeping that many characters overflows by exactly one.
    #[test]
    fn eliding_the_front_never_exceeds_the_given_width() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(800.0, 600.0))),
                ..Default::default()
            },
            |ui| {
                let painter = ui.painter().clone();
                let font = theme::font(11.0);
                let measure =
                    |value: &str| painter.layout_no_wrap(value.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
                for path in [
                    "src/term/glyphs.rs",
                    "a/very/deeply/nested/path/to/some/file.rs",
                    "x.rs",
                    "src/a.rs",
                    "C:/Users/someone/AppData/Local/Temp/anvil/status/notes.md",
                ] {
                    for limit in [4.0_f32, 12.0, 30.0, 60.0, 120.0, 400.0] {
                        let elided = elide_front(&painter, path, font.clone(), limit);
                        let width = measure(&elided);
                        // Either it fits, or not even the ellipsis alone does
                        // and no shorter result exists. The binary search
                        // returns the first count that does NOT fit, so keeping
                        // that many characters overflows by one whole glyph.
                        assert!(
                            width <= limit + 0.01 || measure("…") > limit,
                            "{path:?} at {limit}pt -> {elided:?} measures {width}pt"
                        );
                        assert!(elided.starts_with('…') || elided == path, "{path:?} -> {elided:?}");
                    }
                }
            },
        );
        let _ = output;
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

    /// `git status` sorts by full path, so `src/a.rs`, `src/deep/c.rs` and
    /// `src/z.rs` arrive with `src` interrupted by `src/deep`. Grouping only
    /// consecutive equal directories drew the `src` header twice, each with a
    /// count of one, instead of once with three.
    #[test]
    fn a_folder_is_one_header_however_its_files_are_interleaved() {
        let folders = |rows: Vec<Row>| {
            rows.iter()
                .filter_map(|row| match row {
                    Row::Folder { path, count, .. } => Some(format!("{path}:{count}")),
                    Row::File { .. } => None,
                })
                .collect::<Vec<_>>()
        };
        let interleaved = Workspace {
            status: Status {
                changes: vec![changed_file("src/a.rs"), changed_file("src/deep/c.rs"), changed_file("src/z.rs")],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(folders(interleaved.rows()), vec!["src:2".to_owned(), "src/deep:1".to_owned()]);

        let together = Workspace {
            status: Status {
                changes: vec![
                    changed_file("src/a.rs"),
                    changed_file("src/b.rs"),
                    changed_file("src/deep/c.rs"),
                    changed_file("src/z.rs"),
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(folders(together.rows()), vec!["src:3".to_owned(), "src/deep:1".to_owned()]);
    }

    fn shape_text(shape: &egui::Shape) -> Option<String> {
        match shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(shape_text),
            _ => None,
        }
    }

    /// Every text the shapes paint, in order.
    fn collect_text(shapes: &[egui::Shape], out: &mut Vec<String>) {
        for shape in shapes {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(inner) => collect_text(inner, out),
                _ => {}
            }
        }
    }

    fn panel_frame(
        workspace: &mut Workspace,
        tab: PanelTab,
        pane: crate::layout::split_tree::PaneId,
        rect: egui::Rect,
    ) -> Vec<egui::Shape> {
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
        let output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let _ = workspace.show(ui, rect, pane, None);
            });
        });
        output.shapes
    }

    /// Helm's layout: the history scrolls in its own viewport under the
    /// `[ КОММИТЫ ]` header, so the repo row and the commit box never scroll
    /// away with it.
    #[test]
    fn history_fills_tall_panels_and_virtual_rows_stay_aligned_after_scrolling() {
        let commits: Vec<_> = (0..300).map(|i| commit(&format!("{i:010}"), git::Section::History)).collect();
        let mut workspace = Workspace {
            status: Status { branch: "main".into(), ..Default::default() },
            graph: graph::compute(&commits),
            log: CommitLog { commits, ..Default::default() },
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Test", &[], false);
        for height in [900.0, 1400.0] {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, height));
            for step in 0..20 {
                let pointer = Pos2::new(300.0, height - 100.0);
                let events = if step == 2 {
                    vec![
                        egui::Event::PointerMoved(pointer),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: Vec2::new(0.0, -440.0),
                            phase: egui::TouchPhase::Move,
                            modifiers: Default::default(),
                        },
                    ]
                } else {
                    Vec::new()
                };
                let input = egui::RawInput {
                    screen_rect: Some(rect),
                    time: Some(height as f64 + step as f64 * 0.1),
                    events,
                    ..Default::default()
                };
                let output = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        workspace.show(ui, rect, 1, None);
                    });
                });
                if step < 3 {
                    continue;
                }
                let rows: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text)
                            if text.galley.text().starts_with("commit ")
                                && shape.clip_rect.min.x >= 10.0
                                && shape.clip_rect.max.x <= rect.max.x - 10.0 =>
                        {
                            Some((text.visual_bounding_rect(), shape.clip_rect))
                        }
                        _ => None,
                    })
                    .filter(|(text, clip)| text.intersects(*clip))
                    .collect();
                assert!(!rows.is_empty());
                let bottom = rows.last().unwrap();
                assert!(bottom.1.max.y > height - 30.0, "history clipped early: {bottom:?}");
                assert!(bottom.0.max.y > height - 40.0, "unused bottom of {height}pt panel: {bottom:?}");
                for pair in rows.windows(2) {
                    assert!((pair[1].0.min.y - pair[0].0.min.y - 22.0).abs() < 0.1);
                }
            }
        }
    }

    #[test]
    fn scrolling_the_history_keeps_the_header_in_place() {
        let commits: Vec<git::Commit> = (0..200).map(|i| commit(&format!("{i:010}"), git::Section::History)).collect();
        let mut workspace = Workspace {
            status: Status {
                branch: "main".to_owned(),
                changes: vec![changed_file("src/lib.rs")],
                ..Default::default()
            },
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
            let output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
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
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::Vec2::new(0.0, -400.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        };
        let mut after = Some(before);
        for step in 1..30 {
            let events =
                if step == 1 { vec![egui::Event::PointerMoved(over_history), wheel.clone()] } else { Vec::new() };
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
                head_oid: None,
                upstream_oid: None,
            },
            graph: graph::compute(&commits),
            log: CommitLog { commits, upstream: Some("origin/main".to_owned()), truncated: false },
            ..Default::default()
        };
        for tab in [PanelTab::Changes, PanelTab::Files] {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 700.0));
            let shapes = panel_frame(&mut workspace, tab, 1, rect);
            let clashes: Vec<String> =
                shapes.iter().filter_map(shape_text).filter(|text| text.contains("of ScrollArea")).collect();
            assert!(clashes.is_empty(), "egui reported an id clash in the {tab:?} tab: {clashes:?}");
        }
    }

    /// "Open in the system" runs ShellExecute's default verb: for a script or
    /// a shortcut from a cloned repository that is running it, not viewing it.
    #[test]
    fn programs_and_scripts_are_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        for extension in [
            "exe",
            "BAT",
            "cmd",
            "ps1",
            "js",
            "vbs",
            "wsf",
            "hta",
            "lnk",
            "url",
            "py",
            "reg",
            "msi",
            "sh",
            "pyz",
            "pyzw",
            "diagcab",
            "msix",
            "appx",
            "appinstaller",
            "xll",
            "rdp",
            "website",
            "unknown",
        ] {
            let path = format!("file.{extension}");
            std::fs::write(dir.path().join(&path), b"ordinary text\n").unwrap();
            assert!(!external_open_target(dir.path(), &path).unwrap().1, "{path} must reveal, never execute");
        }
        for (path, content) in [
            ("notes.txt", b"read me\n".as_slice()),
            ("README.md", b"# title\n"),
            ("image.png", b"\x89PNG\r\n\x1a\n"),
            ("paper.pdf", b"%PDF-1.7\n"),
        ] {
            std::fs::write(dir.path().join(path), content).unwrap();
            assert!(external_open_target(dir.path(), path).unwrap().1, "{path} can use its viewer");
            std::fs::write(dir.path().join(path), b"MZrenamed executable").unwrap();
            assert!(
                !external_open_target(dir.path(), path).unwrap().1,
                "renaming a program to {path} must not approve it"
            );
        }
        assert!(external_open_target(dir.path(), "../outside.txt").is_err());
        assert!(external_open_target(dir.path(), "notes.txt:stream").is_err());
    }

    #[test]
    fn external_open_rejects_repository_reparse_escape() {
        let repository = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.txt");
        std::fs::write(&target, "outside repository").unwrap();
        let link = repository.path().join("link.txt");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(not(windows))]
        let linked = std::os::unix::fs::symlink(&target, &link);
        if let Err(error) = linked {
            eprintln!("symlink creation unavailable: {error}");
            return;
        }
        assert!(external_open_target(repository.path(), "link.txt").is_err());
    }

    /// std::fs::rename replaces an existing target on Windows: renaming onto
    /// another file used to destroy it. A case-only rename is still allowed.
    #[test]
    #[cfg(windows)]
    fn renaming_does_not_replace_a_target_created_after_the_last_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source.txt"), "source").unwrap();
        let result = rename_path_checked(dir.path(), "source.txt", "target.txt", || {
            std::fs::write(dir.path().join("target.txt"), "concurrent target").unwrap();
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(dir.path().join("source.txt")).unwrap(), "source");
        assert_eq!(std::fs::read_to_string(dir.path().join("target.txt")).unwrap(), "concurrent target");
    }

    /// std::fs::write opens with create+truncate, so creating a file and then
    /// writing an empty one silently destroyed a file another process put
    /// there in between (a checkout, a second window, an editor). The create
    /// must be exclusive.
    #[test]
    fn creating_a_file_never_truncates_one_that_appeared_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_path(root, "new.txt", false).expect("create");
        assert_eq!(std::fs::read_to_string(root.join("new.txt")).unwrap(), "");

        // Stand in for the race: the file exists by the time we ask.
        let result = write_path(root, "new.txt", false);
        assert_eq!(result, Err(strings::WORKSPACE_FILE_EXISTS.to_owned()));
        assert_eq!(std::fs::read_to_string(root.join("new.txt")).unwrap(), "", "an existing file is left as it was");

        std::fs::write(root.join("taken.txt"), "someone else's work").unwrap();
        assert!(write_path(root, "taken.txt", false).is_err());
        assert_eq!(std::fs::read_to_string(root.join("taken.txt")).unwrap(), "someone else's work", "content survives");
    }

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
        let names: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
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
        let colour = section_color(git::Section::History);
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
            status: Status {
                branch: "main".to_owned(),
                changes: vec![changed_file(path)],
                additions: 3,
                deletions: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut left = panel("src/lib.rs");
        let mut right = panel("README.md");
        let output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
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

    /// A removed line whose text starts with `--` is exactly `---`, and an
    /// added one starting with `++` is exactly `+++`. Reading those as the
    /// file header dropped the row and shifted every number below it.
    /// The shape `git diff` actually prints for a file holding markdown rules:
    /// the context line ` ---` is a `---` that must not be read as a header, and
    /// the context line ` +++` likewise. Captured from a real repository.
    #[test]
    fn real_git_output_numbering_survives_markdown_rules() {
        // `concat!` rather than a `\`-continued literal: that continuation eats
        // the leading whitespace of the next line, and here the leading space
        // *is* the context marker under test.
        let diff = concat!(
            "diff --git a/notes.md b/notes.md\n",
            "index 8b92888..3162765 100644\n",
            "--- a/notes.md\n",
            "+++ b/notes.md\n",
            "@@ -1,6 +1,6 @@\n",
            " intro\n",
            " ---\n",
            "-old\n",
            "+new\n",
            " +++\n",
            " middle\n",
            " footer\n",
        );
        let rows = text_rows(diff, true);
        let numbered: Vec<Option<u64>> = rows.iter().map(|row| row.number).collect();
        assert_eq!(
            numbered,
            vec![None, None, None, None, None, Some(1), Some(2), Some(3), Some(3), Some(4), Some(5), Some(6)],
            "old side: intro/---/old/+++/middle/footer; context rows take the new number, the removal the old one"
        );
        let kinds: Vec<PatchKind> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(kinds[5], PatchKind::Context, "the ` ---` context line is not a header");
        assert_eq!(kinds[7], PatchKind::Remove, "the removed `old` line");
        assert_eq!(kinds[8], PatchKind::Add, "the added `new` line");
        assert_eq!(kinds[9], PatchKind::Context, "the ` +++` context line is not a header");
    }

    #[test]
    fn patch_numbers_keep_content_that_looks_like_a_file_header() {
        let diff = "diff --git a/notes.md b/notes.md\n\
                    --- a/notes.md\n\
                    +++ b/notes.md\n\
                    @@ -1,4 +1,5 @@\n\
                    intro\n\
                    ----\n\
                    ++++\n\
                    outro\n";
        let rows = text_rows(diff, true);
        let numbers: Vec<Option<u64>> = rows.iter().map(|row| row.number).collect();
        assert_eq!(numbers, vec![None, None, None, None, Some(1), Some(2), Some(2), Some(3)]);
        let kinds: Vec<PatchKind> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            vec![
                PatchKind::Context,
                PatchKind::Context,
                PatchKind::Context,
                PatchKind::Hunk,
                PatchKind::Context,
                PatchKind::Remove,
                PatchKind::Add,
                PatchKind::Context
            ],
            "`----` is a removal and `++++` an addition, not header text"
        );
    }

    /// The header block of the next file must not inherit the previous hunk's
    /// numbering, or its `---`/`+++` pair is counted as one removal and one
    /// addition.
    #[test]
    fn patch_numbers_restart_at_every_file_header() {
        let diff = "diff --git a/a.md b/a.md\n\
                    --- a/a.md\n\
                    +++ b/a.md\n\
                    @@ -1,1 +1,1 @@\n\
                    -one\n\
                    +two\n\
                    diff --git a/b.md b/b.md\n\
                    --- a/b.md\n\
                    +++ b/b.md\n\
                    @@ -7,1 +7,1 @@\n\
                    -three\n";
        let numbers: Vec<Option<u64>> = text_rows(diff, true).iter().map(|row| row.number).collect();
        assert_eq!(
            numbers,
            vec![None, None, None, None, Some(1), Some(1), None, None, None, None, Some(7)],
            "the second file numbers from its own hunk header"
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
        let shape =
            |rows: Vec<FileRow>| rows.iter().map(|row| (row.depth, row.name.clone(), row.folder)).collect::<Vec<_>>();
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

    #[test]
    fn hostile_clean_filter_requires_current_explicit_trust() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !git(root, &["init", "--quiet"]) {
            return;
        }
        assert!(git(root, &["config", "user.email", "anvil@test"]));
        assert!(git(root, &["config", "user.name", "ANVIL test"]));
        std::fs::write(root.join("victim.txt"), "original\n").unwrap();
        std::fs::write(root.join(".gitattributes"), "victim.txt filter=hostile\n").unwrap();
        assert!(git(root, &["add", "."]));
        assert!(git(root, &["commit", "--quiet", "-m", "initial"]));
        let marker = root.join("filter-ran");
        let command =
            format!("printf executed > '{}'; cat", marker.to_string_lossy().replace('\\', "/").replace('\'', "'\\''"));
        assert!(git(root, &["config", "filter.hostile.clean", &command]));
        std::fs::write(root.join("victim.txt"), "modified with different size\n").unwrap();
        let (tx, rx) = spawn_worker();
        let wait = || rx.recv_timeout(Duration::from_secs(20)).expect("worker response");
        let required = || match wait() {
            Response::TrustRequired { identity: Some(identity), .. } => identity,
            _ => panic!("trust must be required"),
        };
        tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        let first = required();
        assert!(!marker.exists(), "opening must not execute a clean filter");
        let shown = Some(first.root.clone());
        tx.send((shown.clone(), Request::Diff { path: "victim.txt".to_owned(), side: Some(false) })).unwrap();
        required();
        tx.send((shown.clone(), Request::Refresh { cwd: root.to_owned() })).unwrap();
        required();
        assert!(!marker.exists(), "automatic polls and diff requests must stay gated");

        assert!(git(root, &["config", "filter.hostile.clean", &format!("{command} # rotated")]));
        tx.send((shown.clone(), Request::Approve { identity: first })).unwrap();
        let current = required();
        assert!(!marker.exists(), "stale configuration approval must not execute Git");
        let other = git::RepositoryIdentity { root: root.join("wrong-root"), stamp: current.stamp.clone() };
        tx.send((shown.clone(), Request::Approve { identity: other })).unwrap();
        let current = required();
        assert!(!marker.exists(), "approval for another root must not bypass the gate");

        tx.send((shown.clone(), Request::Approve { identity: current.clone() })).unwrap();
        assert!(matches!(wait(), Response::Trusted(_)));
        assert!(matches!(wait(), Response::Status(_, _)));
        assert!(marker.exists(), "the real clean filter must run only after valid approval");
        std::fs::remove_file(&marker).unwrap();
        // Only hazardous keys revoke: routine tracking keys (`branch.*`,
        // `remote.*`) must not, or every push --set-upstream would ask again.
        assert!(git(root, &["config", "branch.main.remote", "origin"]));
        assert!(git(root, &["config", "remote.origin.url", "."]));
        tx.send((shown.clone(), Request::Log)).unwrap();
        assert!(matches!(wait(), Response::Log(_)), "tracking keys keep the approval");
        assert!(git(root, &["config", "core.hooksPath", ".git/hooks"]));
        tx.send((shown.clone(), Request::Stage { paths: vec!["victim.txt".to_owned()], staged: true })).unwrap();
        required();
        tx.send((shown, Request::Approve { identity: current })).unwrap();
        required();
        assert!(!marker.exists(), "config changes revoke approval before staging or stale reapproval");
    }

    #[test]
    fn trust_survives_a_second_pane_and_routine_git_operations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !git(root, &["init", "--quiet", "--initial-branch=main"]) {
            return;
        }
        assert!(git(root, &["config", "user.email", "anvil@test"]));
        assert!(git(root, &["config", "user.name", "ANVIL test"]));
        assert!(git(root, &["config", "filter.hostile.clean", "cat"]));
        let (first_tx, first_rx) = spawn_worker();
        first_tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        let Response::TrustRequired { identity: Some(identity), .. } =
            first_rx.recv_timeout(Duration::from_secs(20)).unwrap()
        else {
            panic!("a clean filter must ask for trust");
        };
        let shown = Some(identity.root.clone());
        assert_eq!(identity.stamp.hazards(), ["filter.hostile.clean"], "the prompt names what can run");
        first_tx.send((shown.clone(), Request::Approve { identity })).unwrap();
        assert!(matches!(first_rx.recv_timeout(Duration::from_secs(20)).unwrap(), Response::Trusted(_)));
        assert!(matches!(first_rx.recv_timeout(Duration::from_secs(20)).unwrap(), Response::Status(_, _)));
        // What `push --set-upstream` and `checkout -b` write must not re-ask.
        assert!(git(root, &["config", "branch.main.remote", "origin"]));
        assert!(git(root, &["config", "branch.main.merge", "refs/heads/main"]));
        assert!(git(root, &["config", "remote.origin.url", "."]));
        // A second pane of the same repository runs its own worker: no prompt.
        let (second_tx, second_rx) = spawn_worker();
        second_tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        assert!(matches!(second_rx.recv_timeout(Duration::from_secs(20)).unwrap(), Response::Status(_, _)));
        // A hazardous change still revokes.
        assert!(git(root, &["config", "core.hooksPath", ".git/hooks"]));
        second_tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        assert!(matches!(second_rx.recv_timeout(Duration::from_secs(20)).unwrap(), Response::TrustRequired { .. }));
    }

    #[test]
    fn ai_prompt_uses_real_index_without_unstaged_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !git(root, &["init", "--quiet"]) {
            return;
        }
        assert!(git(root, &["config", "user.email", "anvil@test"]));
        assert!(git(root, &["config", "user.name", "ANVIL test"]));
        std::fs::write(root.join("mixed.txt"), "base\n").unwrap();
        std::fs::write(root.join("unstaged-only.txt"), "base\n").unwrap();
        assert!(git(root, &["add", "."]));
        assert!(git(root, &["commit", "--quiet", "-m", "initial"]));
        std::fs::write(root.join("mixed.txt"), "base\nSTAGED_SENTINEL\n").unwrap();
        assert!(git(root, &["add", "mixed.txt"]));
        std::fs::write(root.join("mixed.txt"), "base\nSTAGED_SENTINEL\nUNSTAGED_SENTINEL\n").unwrap();
        std::fs::write(root.join("unstaged-only.txt"), "UNSTAGED_ONLY_SENTINEL\n").unwrap();
        std::fs::write(root.join("untracked.txt"), "UNTRACKED_SENTINEL\n").unwrap();
        let prompt = staged_ai_prompt(root).unwrap();
        assert!(prompt.contains("STAGED_SENTINEL") && prompt.contains("mixed.txt"));
        for excluded in
            ["UNSTAGED_SENTINEL", "UNSTAGED_ONLY_SENTINEL", "UNTRACKED_SENTINEL", "unstaged-only.txt", "untracked.txt"]
        {
            assert!(!prompt.contains(excluded), "unstaged data leaked: {excluded}");
        }
        assert!(git(root, &["reset", "--quiet", "HEAD"]));
        assert_eq!(staged_ai_prompt(root), Err(strings::WORKSPACE_AI_NO_STAGE.to_owned()));
        let (tx, rx) = spawn_worker();
        tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        let shown = approve_worker(&tx, &rx);
        tx.send((shown, Request::AiMessage { command: Some("nonexistent-ai-command".to_owned()) })).unwrap();
        let Response::AiMessage(Err(message)) = rx.recv_timeout(Duration::from_secs(20)).unwrap() else {
            panic!("empty index must reject generation")
        };
        assert_eq!(message, strings::WORKSPACE_AI_NO_STAGE, "the CLI must not start for an empty index");
        let missing = root.join("missing");
        assert!(staged_ai_prompt(&missing).is_err(), "index read errors cannot start generation");
    }

    #[test]
    fn late_commit_diff_releases_busy_without_replacing_current_file() {
        let (tx, rx) = mpsc::channel();
        let mut workspace = Workspace { busy: true, rx: Some(rx), ..Default::default() };
        tx.send(Response::CommitDiff {
            hash: "old".to_owned(),
            path: "old.txt".to_owned(),
            patch: Ok(("old patch".to_owned(), Vec::new())),
        })
        .unwrap();
        workspace.absorb();
        assert!(!workspace.busy && workspace.detail.is_none());
        workspace.busy = true;
        workspace.detail = Some(("new".to_owned(), git::CommitDetail { files: Vec::new(), header: String::new() }));
        workspace.detail_file = Some(CommitFile {
            path: "new.txt".to_owned(),
            patch: None,
            rows: Vec::new(),
            wrapped: WrappedRows::default(),
        });
        tx.send(Response::CommitDiff {
            hash: "new".to_owned(),
            path: "old.txt".to_owned(),
            patch: Ok(("stale patch".to_owned(), Vec::new())),
        })
        .unwrap();
        workspace.absorb();
        assert!(!workspace.busy);
        assert!(workspace.detail_file.as_ref().unwrap().patch.is_none());
    }

    /// The worker answered and then died on the next request. The channel
    /// reports `Disconnected` only once it is drained, so the answers already
    /// in it were collected and then thrown away together with the reset.
    #[test]
    fn answers_a_dying_worker_already_sent_are_still_applied() {
        let (tx, rx) = mpsc::channel();
        let mut workspace =
            Workspace { rx: Some(rx), busy: true, commit_message: "feat: draft".to_owned(), ..Default::default() };
        tx.send(Response::Committed(Ok("0123456789".to_owned()))).unwrap();
        drop(tx);
        assert!(workspace.absorb());
        assert!(workspace.commit_message.is_empty(), "the commit that finished is acknowledged");
        assert!(workspace.notice.as_ref().is_some_and(|(text, error)| text.contains("0123456789") && !error));
        assert!(workspace.rx.is_none() && workspace.tx.is_none() && !workspace.busy, "the lost worker is still reset");
    }

    /// Both marker kinds were searched for again from the cursor at every step,
    /// so a line of code spans (no `**` anywhere) rescanned the whole remainder
    /// for `**` once per span: quadratic, and a Markdown file may be one 8 MiB
    /// line that stalls the UI thread for minutes.
    #[test]
    fn inline_markup_is_linear_in_the_length_of_the_line() {
        let spans = "`a`".repeat(100_000);
        let started = Instant::now();
        let job = inline_job(&spans, 11.5, egui::Color32::WHITE, 300.0);
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
        assert_eq!(job.text.len(), 100_000);

        let job = inline_job("x **bold** `code` **open", 11.5, egui::Color32::WHITE, 300.0);
        let pieces: Vec<&str> = job.sections.iter().map(|section| &job.text[section.byte_range.clone()]).collect();
        assert_eq!(pieces, ["x ", "bold", " ", "code", " ", "**open"]);
        assert!(job.sections[1].format.extra_letter_spacing > 0.0, "bold keeps its format");
        assert_eq!(job.sections[3].format.background, theme::colors().tab_active_bg, "code keeps its chip");
    }

    /// Names in a repository are untrusted: a bidi override spoofs the
    /// extension and a line break splits a fixed-height row. The change list
    /// and the history already strip them; the file tree, the delete/rename
    /// confirmation and the preview header painted the raw path.
    #[test]
    fn paths_from_the_repository_are_painted_without_bidi_or_control_characters() {
        let hostile = "dir/evil\u{202e}gpj.exe\nsecond line";
        let mut workspace = Workspace {
            files: vec![hostile.to_owned()],
            file_expanded: HashSet::from(["dir".to_owned()]),
            file_preview: Some((hostile.to_owned(), "text".to_owned(), false)),
            prompt: Some(Prompt {
                kind: PromptKind::Delete { path: hostile.to_owned(), folder: false },
                text: String::new(),
                focus: false,
            }),
            ..Default::default()
        };
        assert_eq!(workspace.file_tree()[1].name, "evilgpj.exesecond line");
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 600.0));
        let shapes = panel_frame(&mut workspace, PanelTab::Files, 1, rect);
        let mut painted = Vec::new();
        collect_text(&shapes, &mut painted);
        assert!(painted.iter().any(|text| text.contains("evilgpj.exe")), "the name is still shown: {painted:?}");
        let raw: Vec<_> = painted.iter().filter(|text| text.contains('\u{202e}') || text.contains('\n')).collect();
        assert!(raw.is_empty(), "unsanitised text reached the screen: {raw:?}");
    }

    /// A failed create, rename or delete answers with `Response::Error`, which
    /// is stored as the notice. Only the Changes tab and the trust screen drew
    /// it, so in the Files tab the prompt closed and nothing said why the file
    /// was not renamed.
    #[test]
    fn a_failed_file_operation_is_reported_in_the_files_tab() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 600.0));
        let mut workspace = Workspace {
            notice: Some((strings::WORKSPACE_FILE_EXISTS.to_owned(), true)),
            files: vec!["a.txt".to_owned()],
            ..Default::default()
        };
        let mut painted = Vec::new();
        collect_text(&panel_frame(&mut workspace, PanelTab::Files, 1, rect), &mut painted);
        assert!(painted.iter().any(|text| text == strings::WORKSPACE_FILE_EXISTS), "{painted:?}");
        // A success notice belongs to the Changes tab and stays out of this one.
        workspace.notice = Some((strings::workspace_committed("abc"), false));
        let mut painted = Vec::new();
        collect_text(&panel_frame(&mut workspace, PanelTab::Files, 1, rect), &mut painted);
        assert!(!painted.iter().any(|text| text.contains("abc")), "{painted:?}");
    }

    /// The `@@` line repeats when a line is replaced by another one, so the
    /// worker used to apply whatever hunk now sat at the index: a file changed
    /// between the diff on screen and the click staged lines nobody had seen.
    #[test]
    fn a_hunk_is_applied_only_if_it_is_still_the_one_that_was_shown() {
        // Bytes git printed, not their lossy text: two invalid bytes read the same.
        let diff = |byte: &[u8]| {
            [b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+".as_slice(), byte, b"\n"].concat()
        };
        assert_ne!(diff_hunk_bytes(&diff(b"\xC0"), false), diff_hunk_bytes(&diff(b"\xC1"), false));
        assert_eq!(diff_hunk_bytes(&diff(b"a"), false).len(), 1);

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !git(root, &["init", "--quiet"]) {
            return;
        }
        assert!(
            git(root, &["config", "user.email", "anvil@test"]) && git(root, &["config", "user.name", "ANVIL test"])
        );
        std::fs::write(root.join("f.txt"), "one\n").unwrap();
        assert!(git(root, &["add", "f.txt"]) && git(root, &["commit", "--quiet", "-m", "init"]));
        std::fs::write(root.join("f.txt"), "two\n").unwrap();
        let seen = diff_hunk_bytes(&git::diff_bytes(root, "f.txt", false).unwrap(), false);
        assert_eq!(seen.len(), 1);
        // The file changes under the open diff: the same `@@ -1 +1 @@`, another line.
        std::fs::write(root.join("f.txt"), "three\n").unwrap();

        let (tx, rx) = spawn_worker();
        tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        let shown = approve_worker(&tx, &rx);
        let apply = |seen: Vec<u8>| {
            let request = Request::ApplyHunks { path: "f.txt".to_owned(), index: 0, seen, from_index: false };
            tx.send((shown.clone(), request)).unwrap();
            rx.recv_timeout(Duration::from_secs(20)).expect("a worker response")
        };
        match apply(seen[0].clone()) {
            Response::Error(message) => assert_eq!(message, strings::WORKSPACE_DIFF_STALE),
            _ => panic!("a hunk that is no longer the one shown must be refused"),
        }
        assert!(git(root, &["diff", "--cached", "--quiet"]), "nothing was staged");
        let fresh = diff_hunk_bytes(&git::diff_bytes(root, "f.txt", false).unwrap(), false);
        assert!(matches!(apply(fresh[0].clone()), Response::Applied), "the hunk that is shown still applies");
        assert!(!git(root, &["diff", "--cached", "--quiet"]), "the shown hunk was staged");
    }

    /// 50 000 hunks (a 1.5 MB diff, far under the 8 MiB cap) cost seconds on the
    /// worker at every refresh: the bytes of each hunk were cut out of a patch
    /// rebuilt from every hunk of the file.
    #[test]
    fn collecting_the_hunks_of_a_diff_is_linear_in_their_number() {
        let mut diff = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n".to_vec();
        for line in 0..50_000 {
            diff.extend_from_slice(format!("@@ -{line},1 +{line},1 @@\n-a\n+b\n").as_bytes());
        }
        let started = Instant::now();
        let hunks = diff_hunk_bytes(&diff, false);
        assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
        assert_eq!(hunks.len(), 50_000);
        assert_eq!(hunks[49_999], b"@@ -49999,1 +49999,1 @@\n-a\n+b\n");
    }

    /// `git commit` answers "nothing to commit" with the whole `git status` on
    /// stdout, names from the repository included: the notice is untrusted
    /// text, kept and painted without control or bidi characters and bounded.
    #[test]
    fn a_notice_is_painted_without_control_characters_and_within_a_limit() {
        let hostile = format!(
            "On branch main\n\tmodified: evil\u{202e}gpj.exe\u{7}\n{}\n{}",
            "x".repeat(5_000),
            "untracked: file\n".repeat(200)
        );
        let (tx, rx) = mpsc::channel();
        let mut workspace = Workspace {
            rx: Some(rx),
            status: Status { branch: "main".to_owned(), ..Default::default() },
            ..Default::default()
        };
        tx.send(Response::Committed(Err(hostile.clone()))).unwrap();
        workspace.absorb();
        let stored = workspace.notice.clone().expect("the refusal is reported").0;
        assert_eq!(stored, notice_text(&stored), "storing it again changes nothing");
        assert!(stored.lines().count() <= 7 && stored.chars().count() < 2_200, "{} chars", stored.chars().count());
        assert!(!stored.contains('\u{202e}') && !stored.contains('\u{7}'), "{stored:?}");
        assert!(stored.starts_with("On branch main\n"), "the first lines are what explains it");

        // Painted bounded and clean whatever is in the field, in every place it is drawn.
        workspace.notice = Some((hostile, true));
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 600.0));
        for tab in [PanelTab::Changes, PanelTab::Files] {
            let mut painted = Vec::new();
            collect_text(&panel_frame(&mut workspace, tab, 1, rect), &mut painted);
            let notice = painted.iter().find(|text| text.starts_with("On branch main")).expect("the notice is drawn");
            assert!(!notice.contains('\u{202e}') && !notice.contains('\u{7}'), "{tab:?}: {notice:?}");
            assert!(notice.lines().count() <= 7 && notice.chars().count() < 2_200, "{tab:?}");
        }
        workspace.trust_required = true;
        let mut painted = Vec::new();
        collect_text(&panel_frame(&mut workspace, PanelTab::Changes, 1, rect), &mut painted);
        let notice = painted.iter().find(|text| text.starts_with("On branch main")).expect("drawn on the trust screen");
        assert!(!notice.contains('\u{202e}') && notice.chars().count() < 2_200);
    }

    /// A conflicted file is shown as `diff --cc` with `@@@` hunk lines. There is
    /// no hunk behind them (`parse_diff` reads only `diff --git`), so they are
    /// plain rows: a click used to do nothing at all.
    #[test]
    fn combined_diff_hunk_lines_are_not_clickable_hunks() {
        let text = "diff --cc f\n@@@ -1,2 -1,2 +1,4 @@@\n  a\n++b\n";
        assert!(text_rows(text, true).iter().all(|row| row.hunk.is_none()));
        assert!(diff_hunk_bytes(text.as_bytes(), false).is_empty());
        let plain = "diff --git a/f b/f\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(text_rows(plain, true).iter().filter(|row| row.hunk.is_some()).count(), 1);
    }

    /// A double click used to send two `git commit`s: the second failed with
    /// "nothing to commit" and its red answer replaced the first one's notice.
    #[test]
    fn a_second_commit_click_while_the_first_is_pending_sends_nothing() {
        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let mut workspace = Workspace {
            tx: Some(request_tx),
            rx: Some(response_rx),
            commit_message: "feat: change".to_owned(),
            ..Default::default()
        };
        workspace.start_commit();
        workspace.start_commit();
        let commits = request_rx.try_iter().filter(|(_, request)| matches!(request, Request::Commit { .. })).count();
        assert_eq!(commits, 1, "one request for two clicks");
        response_tx.send(Response::Committed(Ok("abc1234".to_owned()))).unwrap();
        workspace.absorb();
        assert_eq!(workspace.notice, Some((strings::workspace_committed("abc1234"), false)));
        assert!(!workspace.committing, "the button is back once the answer arrived");
        // Its own failure is reported, and only for the request that failed.
        workspace.commit_message = "feat: next".to_owned();
        workspace.start_commit();
        response_tx.send(Response::Committed(Err("nothing to commit".to_owned()))).unwrap();
        workspace.absorb();
        assert_eq!(workspace.notice, Some(("nothing to commit".to_owned(), true)));
        assert!(!workspace.committing && workspace.commit_message == "feat: next", "the draft survives a refusal");
    }

    /// A slow commit owns the submitted draft until it answers. Editing that
    /// draft used to accept text which the success response then cleared; AI
    /// generation queued here also ran after the staged index was committed.
    #[test]
    fn pending_commit_does_not_accept_draft_edits_or_ai_generation() {
        fn frame(
            workspace: &mut Workspace,
            ctx: &egui::Context,
            events: Vec<egui::Event>,
        ) -> Vec<egui::epaint::ClippedShape> {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0))),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| workspace.commit_box(ui));
            })
            .shapes
        }
        let click = |point: Pos2| {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ]
        };
        let (request_tx, request_rx) = mpsc::channel();
        let mut workspace = Workspace {
            tx: Some(request_tx),
            commit_message: "submitted draft".to_owned(),
            status: Status { changes: vec![changed_file("f.txt")], ..Default::default() },
            ..Default::default()
        };
        workspace.status.changes[0].index = 'M';
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let shapes = frame(&mut workspace, &ctx, Vec::new());
        let text_point = |shapes: &[egui::epaint::ClippedShape], value: &str| {
            shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == value => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .expect("the control is painted")
        };
        frame(&mut workspace, &ctx, click(text_point(&shapes, "submitted draft")));
        assert!(ctx.memory(|memory| memory.focused().is_some()), "the editor has focus");
        workspace.start_commit();
        let shapes = frame(&mut workspace, &ctx, vec![egui::Event::Text("lost text".to_owned())]);
        assert_eq!(workspace.commit_message, "submitted draft", "pending text cannot be accepted and then lost");
        frame(&mut workspace, &ctx, click(text_point(&shapes, strings::WORKSPACE_AI)));
        assert!(!workspace.ai_generating);
        let requests: Vec<_> = request_rx.try_iter().collect();
        assert_eq!(requests.len(), 1);
        assert!(matches!(requests[0].1, Request::Commit { .. }));
    }

    /// What a click on a file launches is decided on the worker: it
    /// canonicalizes the path and reads the file's first bytes, which on a
    /// cloud placeholder waits for a download. The UI thread starts the result.
    #[test]
    fn the_worker_chooses_what_a_click_on_a_file_launches() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("notes.txt"), "read me\n").unwrap();
        std::fs::write(root.join("run.bat"), "echo hi\r\n").unwrap();
        let full = |path: &str| git::resolve_path(root, path).unwrap();
        assert_eq!(external_launch(root, "notes.txt", false), Ok(Launch::Open(full("notes.txt"))));
        assert_eq!(external_launch(root, "run.bat", false), Ok(Launch::Reveal(full("run.bat"))), "scripts are shown");
        assert_eq!(external_launch(root, "notes.txt", true), Ok(Launch::Reveal(full("notes.txt"))));
        assert!(external_launch(root, "../outside.txt", false).is_err());

        if !git(root, &["init", "--quiet"]) {
            return;
        }
        let (tx, rx) = spawn_worker();
        tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        let shown = approve_worker(&tx, &rx);
        let ask = |request: Request| {
            tx.send((shown.clone(), request)).unwrap();
            match rx.recv_timeout(Duration::from_secs(20)).expect("a worker response") {
                Response::Launch(launch) => launch,
                _ => panic!("a launch answer"),
            }
        };
        assert_eq!(ask(Request::OpenExternal { path: "notes.txt".to_owned() }), Some(Launch::Open(full("notes.txt"))));
        assert_eq!(ask(Request::OpenExternal { path: "run.bat".to_owned() }), Some(Launch::Reveal(full("run.bat"))));
        assert_eq!(ask(Request::Reveal { path: "notes.txt".to_owned() }), Some(Launch::Reveal(full("notes.txt"))));
        assert_eq!(ask(Request::OpenExternal { path: "../outside.txt".to_owned() }), None);
    }

    /// A staged rename is two index entries. Unstaging only the new name left
    /// the deletion of the old one staged: `D  old` next to an untracked `new`.
    #[test]
    fn unstaging_a_rename_restores_both_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !git(root, &["init", "--quiet"]) {
            return;
        }
        assert!(
            git(root, &["config", "user.email", "anvil@test"]) && git(root, &["config", "user.name", "ANVIL test"])
        );
        std::fs::write(root.join("old.txt"), "content\n").unwrap();
        assert!(git(root, &["add", "old.txt"]) && git(root, &["commit", "--quiet", "-m", "init"]));
        assert!(git(root, &["mv", "old.txt", "new.txt"]));
        let status = git::status(root).unwrap();
        let renamed = status.changes.iter().find(|change| change.path == "new.txt").expect("the rename is listed");
        assert_eq!((renamed.index, renamed.original_path.as_deref()), ('R', Some("old.txt")));

        // What the buttons used to send: the cause, shown with the real git.
        git::stage(root, &["new.txt".to_owned()], false).unwrap();
        let half = git::status(root).unwrap();
        assert!(half.changes.iter().any(|change| change.path == "old.txt" && change.staged()), "{:?}", half.changes);

        assert!(git(root, &["add", "-A"]));
        let status = git::status(root).unwrap();
        git::stage(root, &unstage_paths(status.changes.iter()), false).unwrap();
        let after = git::status(root).unwrap();
        assert!(after.changes.iter().all(|change| !change.staged()), "nothing stays staged: {:?}", after.changes);
        assert_eq!(after.changes.len(), 2, "the old name is deleted and the new one untracked: {:?}", after.changes);
    }

    #[test]
    fn revoked_trust_clears_views_and_releases_ai_busy() {
        let (tx, rx) = mpsc::channel();
        let mut workspace = Workspace {
            busy: true,
            ai_generating: true,
            rx: Some(rx),
            log: CommitLog { commits: vec![commit("0123456789", git::Section::History)], ..Default::default() },
            commit_message: "old draft".to_owned(),
            diff_text: "old diff".to_owned(),
            ..Default::default()
        };
        tx.send(Response::TrustRequired { identity: None, error: Some("configuration unavailable".to_owned()) })
            .unwrap();
        workspace.absorb();
        assert!(!workspace.busy && !workspace.ai_generating && workspace.trust_required);
        assert!(workspace.log.commits.is_empty() && workspace.diff_text.is_empty());
        assert_eq!(workspace.commit_message, "old draft", "revocation must not eat the commit draft");
    }

    #[test]
    fn external_commit_state_refreshes_log_but_unchanged_unborn_status_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        if !git(root, &["init", "--quiet", "--initial-branch=main"]) {
            return;
        }
        assert!(git(root, &["config", "user.email", "anvil@test"]));
        assert!(git(root, &["config", "user.name", "ANVIL test"]));
        let (worker_tx, worker_rx) = spawn_worker();
        worker_tx.send((None, Request::Refresh { cwd: root.to_owned() })).unwrap();
        let shown = approve_worker(&worker_tx, &worker_rx);
        let (response_tx, response_rx) = mpsc::channel();
        let (request_tx, request_rx) = mpsc::channel();
        let mut workspace = Workspace { tx: Some(request_tx), rx: Some(response_rx), ..Default::default() };
        let apply_status = |workspace: &mut Workspace| {
            worker_tx.send((shown.clone(), Request::Refresh { cwd: root.to_owned() })).unwrap();
            let response = worker_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert!(matches!(&response, Response::Status(_, _)));
            response_tx.send(response).unwrap();
            workspace.absorb();
            request_rx.try_iter().filter(|(_, request)| matches!(request, Request::Log)).count()
        };
        assert_eq!(apply_status(&mut workspace), 1);
        assert_eq!(apply_status(&mut workspace), 0, "unborn status is not an excuse to repeatedly load empty history");
        std::fs::write(root.join("file.txt"), "first\n").unwrap();
        assert!(git(root, &["add", "."]));
        assert!(git(root, &["commit", "--quiet", "-m", "first external commit"]));
        assert_eq!(apply_status(&mut workspace), 1);
        assert_eq!(apply_status(&mut workspace), 0);
        let first = workspace.status.head_oid.clone();
        std::fs::write(root.join("file.txt"), "second\n").unwrap();
        assert!(git(root, &["add", "."]));
        assert!(git(root, &["commit", "--quiet", "-m", "second external commit"]));
        assert_eq!(apply_status(&mut workspace), 1, "same branch/counts but new HEAD invalidates history");
        assert_ne!(workspace.status.head_oid, first);
        assert_eq!(apply_status(&mut workspace), 0);
        let remote = "refs/remotes/origin/main";
        assert!(git(root, &["update-ref", remote, "HEAD"]));
        assert!(git(root, &["config", "branch.main.remote", "origin"]));
        assert!(git(root, &["config", "branch.main.merge", "refs/heads/main"]));
        assert!(git(root, &["config", "remote.origin.url", "."]));
        assert!(git(root, &["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"]));
        worker_tx.send((shown.clone(), Request::Refresh { cwd: root.to_owned() })).unwrap();
        assert_eq!(approve_worker(&worker_tx, &worker_rx), shown);
        assert_eq!(apply_status(&mut workspace), 1);
        assert!(git(root, &["update-ref", remote, "HEAD~1"]));
        assert_eq!(apply_status(&mut workspace), 1, "external upstream changes invalidate history");
        assert_eq!(apply_status(&mut workspace), 0);
    }
}
