//! AnvilApp: application state, the egui frame and every window-level action.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use egui::{Align2, FontId, Pos2, Rect, Vec2};
use winit::window::{Window, WindowAttributes};

use crate::chrome::collapsed_list;
use crate::chrome::dialogs::{self, DialogState};
use crate::chrome::profile_picker;
use crate::chrome::tabbar::{self, TabInfo};
use crate::chrome::titlebar::{resize_borders, title_bar};
use crate::claude_setup::{self, Plan};
use crate::claude_status::StatusRecord;
use crate::config::{Bell, Config, CursorConfig, CursorShapeConfig};
use crate::fonts;
use crate::host::route::KeyFocus;
use crate::host::WindowCommand;
use crate::hotkeys::{Action, Keymap};
use crate::layout::split_tree::{Anchor, Dir, PaneId, SplitTree};
use crate::profiles::{self, Profile};
use crate::session::{self, PaneState, SessionState, TabState, WindowState};
use crate::strings;
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

use crate::tabs::{FrameEnv, PaneContent, PaneEntry, Tab, TabAction};
use crate::term::input::{encode, InputModes, KeyPress};
use crate::term::pane::PaneEvent;
use crate::term::pane::{Pane, SpawnOptions};
use crate::term::paste;
use crate::term::style::Palette;
use crate::term::view::{PaneCommand, TerminalView};
use crate::theme;

const TOAST_MS: u64 = 3000;

pub struct Toast {
    pub text: String,
    pub at: Instant,
}

pub struct RenameEdit {
    pub tab: usize,
    pub text: String,
    pub focus: bool,
}

pub struct PickerState {
    pub filter: String,
    pub selected: usize,
    pub focus: bool,
    /// The pass the picker appeared in. The click that opened it is otherwise
    /// "a click elsewhere" for the brand-new window and closes it at once.
    pub opened_pass: u64,
}

/// The list of panes hidden by Ctrl+Alt+C, opened with Ctrl+Alt+L.
#[derive(Default)]
pub struct CollapsedListState {
    pub selected: usize,
    /// The pass the list appeared in, so the click that opened
    /// it does not close it at once (see PickerState).
    pub opened_pass: u64,
}

#[derive(Default)]
pub struct UiState {
    pub toasts: Vec<Toast>,
    pub picker: Option<PickerState>,
    pub collapsed_list: Option<CollapsedListState>,
    pub dialog: Option<DialogState>,
}

pub struct ClosedTab {
    pub state: TabState,
}

/// The quota line's segments with the inputs they were built from.
#[derive(Default)]
struct QuotaSegments {
    version: u64,
    minute: i64,
    config: crate::config::QuotaConfig,
    segments: Vec<crate::quota::view::Segment>,
}

/// Command-line flag of a window opened with Ctrl+Shift+N.
pub const NEW_WINDOW_ARG: &str = "--new-window";

/// Takes the value a finished worker sent, if one is waiting, and clears the
/// slot. The slot is finished either way (a disconnected worker sends nothing
/// more), and `try_recv` *consumes*: asking whether a value is ready without
/// taking it loses it, so this is the only place the channel is read.
fn take_ready<T>(slot: &mut Option<std::sync::mpsc::Receiver<T>>) -> Option<T> {
    let rx = slot.as_ref()?;
    match rx.try_recv() {
        Ok(value) => {
            *slot = None;
            Some(value)
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            *slot = None;
            None
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
    }
}

/// How long typing in a settings text field may rest before config.json is
/// written. Every keystroke still applies in memory at once.
const CONFIG_SAVE_IDLE: Duration = Duration::from_millis(700);
/// The longest an edit may wait for the disk, however long the typing goes on.
const CONFIG_SAVE_MAX: Duration = Duration::from_secs(3);
/// The pause before the first automatic retry of a write that failed, doubled
/// after each further failure up to `CONFIG_SAVE_RETRY_MAX`. A write that cannot
/// succeed (a read-only file, another process holding it) costs the UI thread an
/// fsync and up to half a second of sharing retries per attempt, so the attempts
/// thin out instead of recurring every few seconds for as long as the app runs.
const CONFIG_SAVE_RETRY: Duration = Duration::from_secs(5);
const CONFIG_SAVE_RETRY_MAX: Duration = Duration::from_secs(300);

/// An edit of the config that is applied but not yet on disk.
#[derive(Clone, Copy, Debug)]
struct PendingSave {
    /// The first unsaved keystroke, and the latest.
    first: Instant,
    last: Instant,
    /// Writes that failed in a row since the last new typing or forced attempt;
    /// 0: none, the edit is just waiting for its idle moment.
    failures: u32,
    /// The failure has been logged: once per series, not once per attempt.
    reported: bool,
}

impl PendingSave {
    fn new(now: Instant) -> PendingSave {
        PendingSave { first: now, last: now, failures: 0, reported: false }
    }

    /// More typing: a new edit, whose first write is not held back by the
    /// failures of the earlier ones. Its three seconds start now: keeping the
    /// failed edit's `first` would make every further keystroke due at once,
    /// a write (and, on a file that cannot be replaced, half a second of
    /// frozen UI) per keystroke.
    fn touched(self, now: Instant) -> PendingSave {
        let first = if self.failures > 0 { now } else { self.first };
        PendingSave { first, last: now, failures: 0, ..self }
    }

    /// The state after a write failed at `now`. `forced`: the attempt was not
    /// the timer's but the user's (a field or the page left, the window
    /// deactivated, the app closing), which always writes; the pause starts over
    /// from its initial length after it.
    fn failed(previous: Option<PendingSave>, now: Instant, forced: bool) -> PendingSave {
        PendingSave {
            first: previous.map_or(now, |pending| pending.first),
            last: now,
            failures: if forced { 1 } else { previous.map_or(0, |pending| pending.failures).saturating_add(1) },
            reported: true,
        }
    }

    /// The pause before the retry that follows the `failures`-th failure.
    fn retry_pause(failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        CONFIG_SAVE_RETRY.saturating_mul(1 << doublings).min(CONFIG_SAVE_RETRY_MAX)
    }

    /// `None`: write now. `Some(wait)`: ask again after `wait`. Typing is only
    /// possible on the settings page, so leaving it settles the edit at once.
    fn wait(&self, settings_open: bool, now: Instant) -> Option<Duration> {
        let left =
            |since: Instant, limit: Duration| (since + limit).checked_duration_since(now).filter(|d| !d.is_zero());
        if self.failures > 0 {
            return left(self.last, Self::retry_pause(self.failures));
        }
        if !settings_open {
            return None;
        }
        let idle = left(self.last, CONFIG_SAVE_IDLE)?;
        Some(left(self.first, CONFIG_SAVE_MAX)?.min(idle))
    }
}

/// What `check_config` has already said about config.json, so a file that
/// stays broken is reported once and not on every tick.
#[derive(Default)]
struct ReloadLog {
    failure: Option<(Option<SystemTime>, String)>,
}

impl ReloadLog {
    /// Whether this failure is news: the first one, or another error, or the
    /// file changed (and is broken in a new way) since the last one reported.
    fn failed(&mut self, mtime: Option<SystemTime>, error: &str) -> bool {
        let news = !self.failure.as_ref().is_some_and(|(seen, text)| *seen == mtime && text == error);
        if news {
            self.failure = Some((mtime, error.to_owned()));
        }
        news
    }

    /// Whether a good load ends a failure that was reported.
    fn recovered(&mut self) -> bool {
        self.failure.take().is_some()
    }
}

pub struct AnvilApp {
    config: Config,
    config_path: PathBuf,
    config_mtime: Option<SystemTime>,
    config_checked: Instant,
    /// Typing in a settings text field waiting for the disk.
    config_pending: Option<PendingSave>,
    /// The settings page was open in the last frame (to notice it closing).
    config_page_open: bool,
    config_reload_log: ReloadLog,
    keymap: Keymap,
    palette: Palette,
    profiles: Vec<Profile>,
    builtin_profiles: Vec<Profile>,
    wsl_results: Option<std::sync::mpsc::Receiver<Vec<Profile>>>,
    font_entries: Vec<(String, String)>,
    font_families: Vec<String>,
    ai_commands: HashMap<PaneId, String>,
    tabs: Vec<Tab>,
    active: usize,
    settings_open: bool,
    closed_tabs: Vec<ClosedTab>,
    next_pane_id: u64,
    ui: UiState,
    tabbar: tabbar::TabbarState,
    settings: crate::settings_ui::SettingsState,
    session: SessionState,
    /// Opened with Ctrl+Shift+N: starts with one tab and never writes
    /// session.json, which belongs to the first window.
    extra_window: bool,
    session_dirty: Option<Instant>,
    /// Last geometry while not maximized, plus the maximized flag.
    window_state: Option<WindowState>,
    window_maximized: bool,
    status_dir: PathBuf,
    /// Claude Code's statusLine as the settings page shows it, with the
    /// settings.json mtime it was read at.
    claude_line: (Option<SystemTime>, crate::claude_setup::LineState),
    /// This window's quota worker; None while quotas are switched off.
    quota: Option<crate::quota::QuotaHandle>,
    /// Segments of the quota line, rebuilt when the snapshot, the quota
    /// settings or the minute changes (relative times print to the minute).
    quota_segments: QuotaSegments,
    run_dir: Option<PathBuf>,
    repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    inherited_prompt_command: Option<String>,
    proc_snapshot: Vec<crate::procs::ProcInfo>,
    /// A process enumeration in flight. The Toolhelp snapshot blocks for
    /// milliseconds, which does not belong on the frame path.
    proc_results: Option<std::sync::mpsc::Receiver<Option<Vec<crate::procs::ProcInfo>>>>,
    /// When the enumeration in flight (or the last one adopted) was requested:
    /// the snapshot was taken no earlier, so it cannot know about anything
    /// that started after this instant.
    proc_requested: Option<SystemTime>,
    last_status_poll: Instant,
    last_proc_poll: Instant,
    last_tab_area: Rect,
    ime_area: Option<Rect>,
    debug_frame: bool,
    frame_ms: f32,
    started: Instant,
    first_frame_logged: bool,
    /// The heavy system fallback fonts (symbols/emoji/CJK) are installed.
    fallbacks_loaded: bool,
}

impl Default for AnvilApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AnvilApp {
    pub fn new() -> AnvilApp {
        let path = Config::path();
        let crate::config::LoadOutcome { config, notice } = Config::load(&path);
        let config_mtime = file_mtime(&path);
        // Keep disk discovery out of state construction (also used by tests).
        let status_dir = status_dir();
        let extra_window = std::env::args().skip(1).any(|arg| arg == NEW_WINDOW_ARG);
        let mut session = SessionState::load(&SessionState::path());
        if extra_window {
            session = session.for_extra_window();
        }
        let mut app = Self::from_state(config, path, config_mtime, session, extra_window, status_dir);
        app.inherited_prompt_command = std::env::var("PROMPT_COMMAND").ok();
        if let Some(notice) = notice {
            app.toast(notice);
        }
        app
    }

    fn from_state(
        config: Config,
        config_path: PathBuf,
        config_mtime: Option<SystemTime>,
        session: SessionState,
        extra_window: bool,
        status_dir: PathBuf,
    ) -> Self {
        let (keymap, problems) = Keymap::with_overrides(&config.hotkeys);
        let has_problems = !problems.is_empty();
        let palette = scheme_palette(&config);
        theme::set_scheme(&palette);
        let session_window = session.window;
        let mut app = AnvilApp {
            config,
            config_path,
            config_mtime,
            config_checked: Instant::now(),
            config_pending: None,
            config_page_open: false,
            config_reload_log: ReloadLog::default(),
            keymap,
            palette,
            profiles: Vec::new(),
            builtin_profiles: Vec::new(),
            wsl_results: None,
            font_entries: Vec::new(),
            font_families: Vec::new(),
            ai_commands: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            settings_open: false,
            closed_tabs: Vec::new(),
            next_pane_id: 1,
            ui: UiState::default(),
            tabbar: tabbar::TabbarState::default(),
            settings: crate::settings_ui::SettingsState::default(),
            session,
            extra_window,
            session_dirty: None,
            window_state: session_window.map(|w| WindowState { maximized: false, ..w }),
            window_maximized: session_window.map(|w| w.maximized).unwrap_or(false),
            status_dir,
            claude_line: (None, crate::claude_setup::LineState::Missing),
            quota: None,
            quota_segments: QuotaSegments::default(),
            run_dir: None,
            repaint: None,
            inherited_prompt_command: None,
            proc_snapshot: Vec::new(),
            proc_results: None,
            proc_requested: None,
            last_status_poll: Instant::now(),
            last_proc_poll: Instant::now(),
            last_tab_area: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            ime_area: None,
            debug_frame: false,
            frame_ms: 0.0,
            started: Instant::now(),
            first_frame_logged: false,
            fallbacks_loaded: false,
        };
        for problem in &problems {
            log::warn!("config hotkeys: {problem}");
        }
        if has_problems {
            app.toast(strings::UNKNOWN_HOTKEYS.to_owned());
        }
        app
    }

    pub fn window_attributes(&self, screens: &[(i32, i32, u32, u32)]) -> WindowAttributes {
        let mut attributes = Window::default_attributes()
            .with_title(strings::APP_TITLE)
            .with_decorations(false)
            .with_visible(false)
            .with_min_inner_size(winit::dpi::LogicalSize::new(640.0, 400.0))
            .with_window_icon(window_icon());
        if let Some(window) = self.session.window {
            let window = window.on_screens(screens);
            // Saved straight from `inner_size`/`outer_position` of the
            // *restored* (not maximized) window: physical units.
            attributes = attributes
                .with_inner_size(winit::dpi::PhysicalSize::new(window.width, window.height))
                .with_position(winit::dpi::PhysicalPosition::new(window.x, window.y))
                .with_maximized(self.window_maximized);
        } else {
            attributes = attributes.with_inner_size(winit::dpi::LogicalSize::new(1100.0, 640.0));
        }
        attributes
    }

    pub fn on_start(&mut self, ctx: &egui::Context) {
        self.font_entries = fonts::registry_font_entries();
        self.font_families = font_families(&self.font_entries);
        let report = fonts::install(ctx, &self.config.font.family, &self.font_entries, false);
        for missing in &report.missing {
            log::info!("font not found: {missing}");
        }
        let c = ctx.clone();
        self.repaint = Some(Arc::new(move || c.request_repaint()));
        self.builtin_profiles = profiles::detect_builtin();
        let (tx, rx) = std::sync::mpsc::channel();
        self.wsl_results = Some(rx);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(profiles::detect_wsl());
            repaint.request_repaint();
        });
        self.profiles = self.build_profiles();
        let (dir, run_dir) = prepare_status_dir();
        self.status_dir = dir;
        self.run_dir = run_dir;
        self.setup_claude();
        self.sync_quota();

        let mut tabs = Vec::new();
        if self.config.restore_session {
            for state in self.session.tabs.clone() {
                if let Some(tab) = self.restore_tab(&state) {
                    tabs.push(tab);
                }
            }
        }
        if tabs.is_empty() {
            let profile = self.default_profile();
            tabs.push(self.new_tab(&profile, None));
        }
        self.active = self.session.active_tab.min(tabs.len() - 1);
        self.tabs = tabs;
        self.tabbar.rename = None;
        self.mark_session_dirty();
        // The DLL is loaded while the first pane starts, so check it now.
        if !crate::term::pane::bundled_conpty_loaded() {
            log::warn!("conpty.dll is not loaded; the system ConPTY is in use");
            self.toast(strings::CONPTY_MISSING.to_owned());
        }
    }

    pub fn frame(&mut self, ui: &mut egui::Ui, maximized: bool) -> Vec<WindowCommand> {
        let ctx = ui.ctx().clone();
        let started = Instant::now();
        let mut commands = Vec::new();
        let mut refresh_quota = false;
        self.check_config(&ctx);
        self.absorb_wsl_profiles();
        self.poll_panes(&ctx);
        self.poll_statuses();
        self.expire_toasts();
        // On a border the press belongs to the window resize, not to the pane
        // under it: the flag is threaded down so a drag there does not also
        // start a text selection.
        let window_edge = resize_borders(&ctx, maximized, &mut commands);

        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::colors().chrome_bg)).show_inside(ui, |ui| {
            if self.ui.dialog.is_some() {
                ui.disable();
            }
            let full = ui.max_rect();
            let title = Rect::from_min_size(full.min, Vec2::new(full.width(), theme::TITLEBAR_HEIGHT));
            title_bar(ui, title, maximized, window_edge, &mut commands);
            // The quota line takes the bottom of the window while quotas are on;
            // tabs and panes get the rest, so nothing is ever drawn under it.
            let footer = if self.quota.is_some() { crate::chrome::quota_bar::HEIGHT } else { 0.0 };
            let bottom = Pos2::new(full.max.x, full.max.y - footer);
            let body = Rect::from_min_max(Pos2::new(full.min.x, title.max.y), bottom);
            if footer > 0.0 {
                let line = Rect::from_min_max(Pos2::new(full.min.x, body.max.y), full.max);
                let segments = self.current_quota_segments();
                if crate::chrome::quota_bar::show(ui, line, segments).is_some() {
                    refresh_quota = true;
                }
            }
            let tabbar_rect = Rect::from_min_size(body.min, Vec2::new(theme::TABBAR_WIDTH, body.height()));
            let area = Rect::from_min_max(Pos2::new(body.min.x + theme::TABBAR_WIDTH, body.min.y), body.max);
            ui.painter().vline(
                tabbar_rect.max.x - 0.5,
                tabbar_rect.y_range(),
                egui::Stroke::new(1.0, theme::colors().border),
            );

            let badge = &self.config.claude_status;
            let show_badge = badge.badge && badge.badge_fields.any();
            let infos: Vec<TabInfo> = (0..self.tabs.len())
                .map(|i| TabInfo {
                    title: self.tabs[i].title().into(),
                    active: i == self.active && !self.settings_open,
                    activity: self.tabs[i].has_activity && i != self.active,
                    // Hidden badge: no extra row height either.
                    claude: show_badge.then(|| self.tabs[i].claude_status()).flatten(),
                    color: self.tabs[i].color,
                })
                .collect();
            let badge_fields = self.config.claude_status.badge_fields;
            let collapsed: usize = self.tabs.iter().map(|tab| tab.collapsed.len()).sum();
            let tabbar_actions =
                tabbar::show(ui, tabbar_rect, &mut self.tabbar, &infos, self.settings_open, &badge_fields, collapsed);
            for action in tabbar_actions {
                self.apply_tabbar_action(action, &ctx);
            }

            if self.settings_open {
                self.ime_area = None;
                self.show_settings(ui, area);
            } else {
                self.show_active_tab(ui, area, &ctx, window_edge);
            }
        });

        self.show_toasts(&ctx);
        self.show_picker(&ctx);
        self.show_collapsed_list(&ctx);
        self.show_dialog(&ctx);
        self.flush_config_if_due(&ctx);
        if let Some(at) = self.session_dirty {
            if at.elapsed() >= Duration::from_secs(1) {
                self.save_session();
                self.session_dirty = None;
            } else {
                // Idle windows draw no frames, so ask for one when the save is due.
                ctx.request_repaint_after(Duration::from_millis(1200));
            }
        }
        if self.debug_frame {
            ctx.debug_painter().text(
                ctx.content_rect().right_top() + Vec2::new(-8.0, 8.0),
                Align2::RIGHT_TOP,
                format!("{:.1} ms", self.frame_ms),
                FontId::monospace(11.0),
                theme::colors().accent,
            );
        }
        if refresh_quota {
            self.request_quota_refresh();
        }
        // The config watcher and the Claude status poll are periodic: keep a
        // 1 Hz tick alive so they run even when nothing else asks for a frame.
        ctx.request_repaint_after(Duration::from_millis(1000));
        self.frame_ms = started.elapsed().as_secs_f32() * 1000.0;
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            log::info!("first frame in {} ms", self.started.elapsed().as_millis());
        }
        commands
    }

    pub fn ime_area(&self) -> Option<Rect> {
        self.ime_area
    }

    pub fn key_focus(&self, ctx: &egui::Context) -> KeyFocus {
        // Open overlays own the keyboard even when their widgets lost focus,
        // otherwise Esc would reach the shell and leave a stuck popup.
        let overlay = self.ui.picker.is_some() || self.ui.collapsed_list.is_some() || self.ui.dialog.is_some();
        KeyFocus {
            egui_wants_keyboard: ctx.egui_wants_keyboard_input() || overlay,
            terminal_focused: !self.settings_open && self.tabs.get(self.active).is_some(),
        }
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn toggle_debug_overlay(&mut self) {
        self.debug_frame = !self.debug_frame;
    }

    // ---- frame helpers -------------------------------------------------

    /// Drains pane events and output flags for *every* pane of every tab, so
    /// background tabs still deliver clipboard requests, titles, exits and the
    /// activity dot instead of growing an unconsumed queue.
    fn poll_panes(&mut self, ctx: &egui::Context) {
        let active = self.active;
        let mut close: Vec<PaneId> = Vec::new();
        let mut bells = 0usize;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            for (id, entry) in tab.panes.iter_mut() {
                let PaneContent::Live(pane) = &mut entry.content else { continue };
                for event in pane.drain_events() {
                    match event {
                        PaneEvent::Title(title) => entry.title = title,
                        PaneEvent::ResetTitle => entry.title = entry.profile_name.clone(),
                        PaneEvent::Clipboard(text) => {
                            if self.config.terminal.allow_osc52 {
                                ctx.copy_text(text);
                            }
                        }
                        PaneEvent::Bell => bells += 1,
                        PaneEvent::CursorBlinkingChange => {
                            // A state notification: read the effective style, so a
                            // steady cursor does not start blinking forever.
                            entry.view.app_blink = pane.term.lock().cursor_style().blinking;
                        }
                        PaneEvent::Exited(code) => match code {
                            Some(0) => close.push(*id),
                            other => {
                                let message = format!("\r\n\x1b[90m{}\x1b[0m\r\n", strings::process_exited(other));
                                let mut term = pane.term.lock();
                                let mut processor: Processor<StdSyncHandler> = Processor::new();
                                processor.advance(&mut *term, message.as_bytes());
                                entry.exited = true;
                            }
                        },
                    }
                }
                // Drain on its own line, for every pane including the active
                // tab's. Written as `index != active && take_output_flag()` the
                // `&&` short-circuits, so an active tab never cleared the flag
                // and the next switch to it lit a dot for output the user had
                // just watched being printed.
                let produced_output = pane.take_output_flag();
                if produced_output && index != active {
                    tab.has_activity = true;
                }
            }
        }
        if bells > 0 && self.config.terminal.bell == Bell::Visual {
            self.toast(strings::BELL.to_owned());
        }
        // By id: closing one tab shifts the indices of the tabs after it.
        for id in close {
            self.close_pane_anywhere(id);
        }
    }

    fn show_active_tab(&mut self, ui: &mut egui::Ui, rect: Rect, ctx: &egui::Context, window_edge: bool) {
        self.last_tab_area = rect;
        let cursor = &self.config.terminal.cursor;
        let metrics = self.tabs.get(self.active).and_then(|t| t.focused_entry()).and_then(|e| e.view.metrics);
        let (cell_w, cell_h) = metrics.map(|m| (m.width, m.height)).unwrap_or((9.0, 18.0));
        let env = FrameEnv {
            palette: &self.palette,
            active: true,
            cursor_blink: cursor.blink,
            right_click: self.config.terminal.right_click,
            paste_on_middle: self.config.terminal.paste_on_middle_click,
            copy_on_select: self.config.terminal.copy_on_select,
            min_pane_width: cell_w * 8.0,
            min_pane_height: cell_h * 3.0,
            fallbacks_loaded: self.fallbacks_loaded,
            ai_command: self.ai_command(),
            window_edge,
        };
        let actions = match self.tabs.get_mut(self.active) {
            Some(tab) => tab.show(ui, rect, &env),
            None => return,
        };
        for action in actions {
            self.apply_tab_action(action, ctx);
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.has_activity {
                tab.has_activity = false;
            }
            // The IME candidate window follows the focused pane's cursor.
            self.ime_area = tab.ime_area;
        }
    }

    /// Starts or stops this window's quota worker to match the settings.
    fn sync_quota(&mut self) {
        if !self.config.quota.enabled {
            self.quota = None;
            self.quota_segments = QuotaSegments::default();
            return;
        }
        if self.quota.is_some() {
            return;
        }
        let repaint = self.repaint.clone();
        let config_path = self.config_path.clone();
        self.quota = Some(crate::quota::QuotaHandle::start(
            crate::quota::Paths::in_dir(&crate::quota::Paths::default_dir()),
            // Read per cycle from the file every window saves to, so a provider
            // switched off anywhere gets no further request.
            move || match Config::load_for_reload(&config_path) {
                Ok(config) => Some(crate::quota::prefs_from(&config.quota)),
                Err(_) if !config_path.exists() => Some(crate::quota::Prefs::new()),
                Err(_) => None,
            },
            move || {
                if let Some(repaint) = &repaint {
                    repaint();
                }
            },
        ));
    }

    fn current_quota_segments(&mut self) -> &[crate::quota::view::Segment] {
        let Some(handle) = &self.quota else { return &[] };
        let now = crate::quota::time::now_unix();
        let version = handle.version();
        let cache = &mut self.quota_segments;
        if cache.version != version || cache.minute != now / 60 || cache.config != self.config.quota {
            cache.segments = crate::quota::view::segments(&handle.snapshot(), &self.config.quota, now);
            cache.version = version;
            cache.minute = now / 60;
            cache.config = self.config.quota.clone();
        }
        &cache.segments
    }

    /// Re-reads Claude Code's user settings when they changed (one stat per
    /// frame, only while the settings page is open).
    fn refresh_claude_line(&mut self) {
        let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let Some(path) = claude_setup::settings_path(claude_dir.as_deref(), home.as_deref()) else { return };
        let mtime = file_mtime(&path);
        if mtime.is_some() && mtime == self.claude_line.0 {
            return;
        }
        let text = std::fs::read_to_string(&path).ok();
        self.claude_line = (mtime, claude_setup::describe(text.as_deref()));
    }

    fn show_settings(&mut self, ui: &mut egui::Ui, rect: Rect) {
        self.refresh_claude_line();
        let quota_snapshot = self.quota.as_ref().map(|q| q.snapshot());
        let rows = self.keymap.describe();
        // Keep the applied config intact until apply_config compares the two:
        // a default placeholder loses transition detection and can reach disk.
        let mut next = self.config.clone();
        let claude_line = self.claude_line.1.clone();
        let outcome = {
            let mut context = crate::settings_ui::SettingsContext {
                config: &mut next,
                keymap_rows: rows,
                profiles: &self.profiles,
                fonts: &self.font_families,
                claude_line: &claude_line,
                quota: quota_snapshot.as_ref(),
            };
            crate::settings_ui::show(ui, rect, &mut context, &mut self.settings)
        };
        self.settle_settings(ui.ctx(), next, &outcome);
        if outcome.open_config {
            crate::settings_ui::open_path(&self.config_path);
        }
        if outcome.refresh_fonts {
            self.font_entries = fonts::registry_font_entries();
            self.font_families = font_families(&self.font_entries);
            fonts::install(ui.ctx(), &self.config.font.family, &self.font_entries, self.fallbacks_loaded);
            for tab in &mut self.tabs {
                for entry in tab.panes.values_mut() {
                    entry.view.font_changed();
                }
            }
        }
        if outcome.install_claude {
            self.setup_claude_explicit();
        }
        if outcome.restore_claude {
            self.disable_claude();
        }
        if outcome.quota_refresh {
            self.request_quota_refresh();
        }
    }

    fn settle_settings(&mut self, ctx: &egui::Context, next: Config, outcome: &crate::settings_ui::SettingsOutcome) {
        if outcome.changed || outcome.typed {
            // Typing applies in memory at once; its write waits for the field to
            // be left or the typing to rest, so a keystroke is not an fsync.
            self.apply_config(ctx.clone(), next, outcome.changed);
            if !outcome.changed {
                self.defer_config_save();
            }
        }
        if outcome.commit {
            self.flush_config();
        }
        if outcome.open_config {
            self.save_config_now();
        }
    }

    /// Notes that the applied config is ahead of config.json by typing.
    fn defer_config_save(&mut self) {
        let now = Instant::now();
        self.config_pending = Some(self.config_pending.map_or(PendingSave::new(now), |pending| pending.touched(now)));
    }

    /// Writes config.json and makes it the file's current state.
    fn write_config(&mut self) -> std::io::Result<()> {
        let path = self.config_path.clone();
        self.config.save(&path)?;
        self.config_mtime = file_mtime(&path);
        self.config_pending = None;
        Ok(())
    }

    /// `write_config`, logging a failure once per series. The edit stays pending
    /// then, so it is tried again instead of living in memory only. `forced`
    /// is the user's attempt (see `PendingSave::failed`); the timer's retries
    /// back off.
    fn try_save_config(&mut self, forced: bool) {
        if let Err(e) = self.write_config() {
            if !self.config_pending.is_some_and(|pending| pending.reported) {
                log::warn!("cannot save {}: {e}", self.config_path.display());
            }
            self.config_pending = Some(PendingSave::failed(self.config_pending, Instant::now(), forced));
        }
    }

    /// An attempt that is always made: the edit is the user's own act.
    fn save_config_now(&mut self) {
        self.try_save_config(true);
    }

    /// Writes what typing left pending, now, whatever an earlier failure has
    /// backed off. Called wherever the file has to be current: a field or the
    /// page is left, the window is deactivated, a window starts, the app exits.
    fn flush_config(&mut self) {
        if self.config_pending.is_some() {
            self.save_config_now();
        }
    }

    /// The frame's share of the deferred write: due, or asks for a frame when it
    /// will be. The frame that finds the settings page just closed makes the
    /// write unconditionally, so a backed-off retry does not hold it up.
    fn flush_config_if_due(&mut self, ctx: &egui::Context) {
        let page_closed = self.config_page_open && !self.settings_open;
        self.config_page_open = self.settings_open;
        if page_closed {
            self.flush_config();
            return;
        }
        let Some(pending) = self.config_pending else { return };
        match pending.wait(self.settings_open, Instant::now()) {
            None => self.try_save_config(false),
            Some(wait) => ctx.request_repaint_after(wait),
        }
    }

    /// Asks for a quota cycle, saying so when the anti-hammer gap defers it:
    /// otherwise a click inside the gap looks exactly like a dead button.
    fn request_quota_refresh(&mut self) {
        let Some(quota) = &self.quota else { return };
        let wait = quota.refresh();
        if wait > 0 {
            self.toast(strings::QUOTA_REFRESH_DEFERRED.replace("{0}", &wait.to_string()));
        }
    }

    fn apply_tabbar_action(&mut self, action: tabbar::TabbarAction, ctx: &egui::Context) {
        match action {
            tabbar::TabbarAction::Select(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                    self.settings_open = false;
                    ctx.request_repaint();
                }
            }
            tabbar::TabbarAction::Close(index) => self.close_tab(index),
            tabbar::TabbarAction::Duplicate(index) => self.duplicate_tab(index),
            tabbar::TabbarAction::CloseOthers(index) => self.close_other_tabs(index),
            tabbar::TabbarAction::Rename(index, title) => {
                if let Some(tab) = self.tabs.get_mut(index) {
                    tab.custom_title = (!title.is_empty()).then_some(title);
                }
            }
            tabbar::TabbarAction::SetColor(index, color) => {
                if let Some(tab) = self.tabs.get_mut(index) {
                    tab.color = color;
                    self.mark_session_dirty();
                }
            }
            tabbar::TabbarAction::Move(from, to) => self.move_tab(from, to),
            tabbar::TabbarAction::NewTab => {
                let profile = self.default_profile();
                let cwd = self.focused_cwd();
                self.new_tab_at_end(&profile, cwd);
            }
            tabbar::TabbarAction::Profiles => self.open_picker(ctx),
            tabbar::TabbarAction::CollapsedList => self.open_collapsed_list(ctx),
            tabbar::TabbarAction::Settings => {
                self.settings_open = true;
                ctx.request_repaint();
            }
        }
    }

    fn apply_tab_action(&mut self, action: TabAction, ctx: &egui::Context) {
        match action {
            TabAction::Focus(id) => self.focus_pane(id),
            TabAction::ClosePane(id) => self.close_pane(id),
            TabAction::Split(id, dir) => self.split_pane(id, dir),
            TabAction::Relocate { pane, target, dir, after } => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if tab.tree.relocate(pane, target, dir, after) {
                        self.focus_pane(pane);
                        ctx.request_repaint();
                    }
                }
            }
            TabAction::ToggleMaximized(id) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.maximized = if tab.maximized == Some(id) { None } else { Some(id) };
                }
            }
            TabAction::Collapse(id) => self.collapse_pane(id),
            TabAction::RestoreCollapsed(id) => self.restore_collapsed(id),
            TabAction::Clipboard(text) => ctx.copy_text(text),
            TabAction::Pane(id, command) => self.apply_pane_command(id, command, ctx),
            TabAction::Bell => {
                if self.config.terminal.bell == Bell::Visual {
                    self.toast(strings::BELL.to_owned());
                }
            }
            TabAction::NeedsFallbacks => self.load_fallbacks(ctx),
        }
        self.mark_session_dirty();
    }

    /// Installs the system fallback fonts the first time a glyph needs them.
    /// They cost ~100 MB of working set, so panes start without them. Reading
    /// and parsing them takes long enough to drop frames, so it happens on a
    /// worker: the glyphs appear one repaint later instead of freezing the
    /// window the first time a CJK or emoji character arrives.
    fn load_fallbacks(&mut self, ctx: &egui::Context) {
        if self.fallbacks_loaded {
            return;
        }
        self.fallbacks_loaded = true;
        let ctx = ctx.clone();
        let family = self.config.font.family.clone();
        let entries = self.font_entries.clone();
        std::thread::spawn(move || {
            let report = fonts::install(&ctx, &family, &entries, true);
            for missing in &report.missing {
                log::info!("font not found: {missing}");
            }
            log::info!("system fallback fonts installed");
            ctx.request_repaint();
        });
    }

    fn apply_pane_command(&mut self, id: PaneId, command: PaneCommand, ctx: &egui::Context) {
        match command {
            PaneCommand::Copy => {
                if let Some(text) = self.selection_text(id) {
                    self.set_clipboard(&text);
                    self.toast(strings::COPIED.to_owned());
                }
                if let Some(pane) = self.pane(id) {
                    pane.term.lock().selection = None;
                }
            }
            PaneCommand::CopySelection => {
                if let Some(text) = self.selection_text(id) {
                    ctx.copy_text(text);
                }
            }
            PaneCommand::Paste => self.paste_clipboard(id),
            PaneCommand::SelectAll => self.select_all(id),
            PaneCommand::Clear => self.clear_pane(id),
            PaneCommand::SplitRight => self.split_pane(id, Dir::Row),
            PaneCommand::SplitDown => self.split_pane(id, Dir::Column),
            PaneCommand::ClosePane => self.close_pane(id),
        }
        ctx.request_repaint();
    }

    // ---- terminal input -------------------------------------------------

    pub fn send_key(&mut self, press: &KeyPress) {
        if self.ui.dialog.is_some() {
            return;
        }
        // Ctrl or Alt alone is not "a key": it may start Ctrl+Shift+C on the
        // exit message.
        if !press.is_modifier_only() && self.tabs.get(self.active).is_some_and(Tab::focused_exited) {
            let id = self.tabs[self.active].focused;
            self.close_pane(id);
            return;
        }
        let Some(entry) = self.tabs.get_mut(self.active).and_then(Tab::focused_entry_mut) else { return };
        let bytes = match entry.live() {
            Some(pane) => {
                let app_cursor = pane.term.lock().mode().contains(TermMode::APP_CURSOR);
                encode(press, InputModes { app_cursor })
            }
            None => None,
        };
        if let Some(bytes) = bytes {
            if let Some(pane) = entry.live() {
                pane.term.lock().scroll_display(Scroll::Bottom);
                pane.write(bytes);
            }
            entry.view.reset_blink();
        }
    }

    pub fn send_text(&mut self, text: &str) {
        if self.ui.dialog.is_some() {
            return;
        }
        // The same guard as a key press: text goes nowhere once the process is
        // gone, and typing is what dismisses the exit message.
        if self.tabs.get(self.active).is_some_and(Tab::focused_exited) {
            let id = self.tabs[self.active].focused;
            self.close_pane(id);
            return;
        }
        if let Some(pane) = self.focused_pane() {
            pane.term.lock().scroll_display(Scroll::Bottom);
            pane.write(text.as_bytes().to_vec());
        }
        if let Some(view) = self.focused_view_mut() {
            view.reset_blink();
        }
    }

    /// Called by the host on every resize/move: keeps the restore geometry
    /// (the non-maximized one) and the maximized flag for the session file.
    pub fn window_geometry(
        &mut self,
        size: winit::dpi::PhysicalSize<u32>,
        position: Option<winit::dpi::PhysicalPosition<i32>>,
        maximized: bool,
    ) {
        self.window_maximized = maximized;
        // Minimized windows report a zero size (and an off-screen position);
        // keeping those would destroy the stored restore bounds.
        if maximized || size.width == 0 || size.height == 0 {
            return;
        }
        let previous = self.window_state;
        self.window_state = Some(WindowState {
            x: position.map(|p| p.x).or(previous.map(|w| w.x)).unwrap_or(0),
            y: position.map(|p| p.y).or(previous.map(|w| w.y)).unwrap_or(0),
            width: size.width,
            height: size.height,
            maximized: false,
        });
    }

    pub fn window_focus_changed(&mut self, focused: bool) {
        if !focused {
            // Another window or program may read config.json next.
            self.flush_config();
        }
        if let Some(pane) = self.focused_pane() {
            if pane.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
                pane.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
            }
        }
    }

    pub fn run_action(&mut self, action: &Action, ctx: &egui::Context) -> Vec<WindowCommand> {
        if self.ui.dialog.is_some() {
            return Vec::new();
        }
        let mut commands = Vec::new();
        match action {
            Action::NewTab => {
                let profile = self.default_profile();
                let cwd = self.focused_cwd();
                self.new_tab_at_end(&profile, cwd);
            }
            Action::NewWindow => {
                // The new process reads config.json at once.
                self.flush_config();
                if let Ok(exe) = std::env::current_exe() {
                    if let Err(e) = std::process::Command::new(exe).arg(NEW_WINDOW_ARG).spawn() {
                        log::warn!("cannot start a new window: {e}");
                    }
                }
            }
            Action::CloseTab => self.close_tab(self.active),
            Action::ReopenTab => self.reopen_tab(),
            Action::RenameTab => {
                let title = self.tabs.get(self.active).map(Tab::title).unwrap_or_default();
                self.tabbar.rename = Some(tabbar::RenameEdit { tab: self.active, text: title.to_owned(), focus: true });
            }
            Action::NextTab => self.cycle_tab(1),
            Action::PreviousTab => self.cycle_tab(-1),
            Action::MoveTabLeft => self.move_tab(self.active, self.active.saturating_sub(1)),
            Action::MoveTabRight => {
                self.move_tab(self.active, (self.active + 1).min(self.tabs.len().saturating_sub(1)))
            }
            Action::Tab(n) => {
                let index = (*n as usize).saturating_sub(1);
                if index < self.tabs.len() {
                    self.active = index;
                    self.settings_open = false;
                }
            }
            Action::SplitRight => self.split_focused(Dir::Row),
            Action::SplitBottom => self.split_focused(Dir::Column),
            Action::PaneNavLeft => self.navigate_pane(-1, 0),
            Action::PaneNavRight => self.navigate_pane(1, 0),
            Action::PaneNavUp => self.navigate_pane(0, -1),
            Action::PaneNavDown => self.navigate_pane(0, 1),
            Action::PaneNavPrevious => self.navigate_cycle(false),
            Action::PaneNavNext => self.navigate_cycle(true),
            Action::PaneMaximize => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let id = tab.focused;
                    tab.maximized = if tab.maximized == Some(id) { None } else { Some(id) };
                }
            }
            Action::ClosePane => {
                let id = self.tabs.get(self.active).map(|t| t.focused);
                if let Some(id) = id {
                    self.close_pane(id);
                }
            }
            Action::PaneCollapse => {
                let id = self.tabs.get(self.active).map(|t| t.focused);
                if let Some(id) = id {
                    self.collapse_pane(id);
                }
            }
            Action::PaneRestore => {
                let id = self.tabs.get(self.active).and_then(|t| t.collapsed.last().map(|(id, _)| *id));
                if let Some(id) = id {
                    self.restore_collapsed(id);
                }
            }
            Action::Profile(id) => {
                if let Some(profile) = self.profiles.iter().find(|p| p.id == *id).cloned() {
                    let cwd = self.focused_cwd();
                    self.new_tab_at_end(&profile, cwd);
                }
            }
            Action::ProfileSelector => self.open_picker(ctx),
            Action::CollapsedList => self.open_collapsed_list(ctx),
            Action::Settings => self.settings_open = true,
            Action::ToggleFullscreen => commands.push(WindowCommand::ToggleFullscreen),
            Action::ToggleFrameStats => self.toggle_debug_overlay(),
            Action::CtrlC => {
                if let Some(id) = self.focused_id() {
                    if self.selection_text(id).is_some() {
                        self.apply_pane_command(id, PaneCommand::Copy, ctx);
                    } else {
                        self.write_focused(b"\x03".to_vec());
                    }
                }
            }
            Action::Copy => {
                if let Some(id) = self.focused_id() {
                    self.apply_pane_command(id, PaneCommand::Copy, ctx);
                }
            }
            Action::Paste => {
                if let Some(id) = self.focused_id() {
                    self.paste_clipboard(id);
                }
            }
            Action::SelectAll => {
                if let Some(id) = self.focused_id() {
                    self.select_all(id);
                }
            }
            Action::Clear => {
                if let Some(id) = self.focused_id() {
                    self.clear_pane(id);
                }
            }
            Action::ZoomIn => self.zoom_text(1.0, ctx),
            Action::ZoomOut => self.zoom_text(-1.0, ctx),
            Action::ResetZoom => self.zoom_text(0.0, ctx),
            Action::PreviousWord => self.write_focused(b"\x1b[1;5D".to_vec()),
            Action::NextWord => self.write_focused(b"\x1b[1;5C".to_vec()),
            Action::DeletePreviousWord => self.write_focused(b"\x17".to_vec()),
            Action::DeleteNextWord => self.write_focused(b"\x1bd\x1b[3;5~".to_vec()),
            Action::DeleteLine => self.write_focused(b"\x1bw".to_vec()),
            Action::Search => {
                if let Some(view) = self.focused_view_mut() {
                    view.search.open = true;
                    view.search.focus_requested = true;
                }
            }
            Action::ToggleWorkspace => {
                if let Some(workspace) = self.tabs.get_mut(self.active).and_then(Tab::workspace_mut) {
                    workspace.open = !workspace.open;
                    if workspace.open {
                        workspace.refresh_soon();
                    }
                }
            }
            Action::ScrollToTop => self.scroll_focused(Scroll::Top),
            Action::ScrollToBottom => self.scroll_focused(Scroll::Bottom),
            Action::ScrollPageUp => self.scroll_focused(Scroll::PageUp),
            Action::ScrollPageDown => self.scroll_focused(Scroll::PageDown),
            Action::ScrollUp => self.scroll_focused(Scroll::Delta(1)),
            Action::ScrollDown => self.scroll_focused(Scroll::Delta(-1)),
        }
        self.mark_session_dirty();
        ctx.request_repaint();
        commands
    }

    pub fn on_exit(&mut self, window: Option<&Window>) {
        self.flush_config();
        if let Some(window) = window {
            self.window_geometry(window.inner_size(), window.outer_position().ok(), window.is_maximized());
        }
        self.save_session();
        for tab in &mut self.tabs {
            tab.panes.clear();
        }
        if let Some(dir) = self.run_dir.take() {
            remove_owned_status_dir(&dir);
        }
    }

    // ---- tabs and panes -------------------------------------------------

    fn focused_id(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    fn focused_pane(&self) -> Option<&Pane> {
        self.tabs.get(self.active).and_then(|t| t.focused_entry()).and_then(PaneEntry::live)
    }

    fn focused_view_mut(&mut self) -> Option<&mut TerminalView> {
        self.tabs.get_mut(self.active).and_then(Tab::focused_entry_mut).map(|e| &mut e.view)
    }

    fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.tabs.get(self.active).and_then(|t| t.pane(id)).and_then(PaneEntry::live)
    }

    /// CLI for AI commit messages: the configured one, else the AI CLI found
    /// in the focused pane's process tree (claude, opencode, codex, …).
    fn ai_command(&self) -> Option<String> {
        let command = self
            .config
            .workspace
            .ai_commit_command
            .clone()
            .or_else(|| self.focused_id().and_then(|id| self.ai_commands.get(&id).cloned()))?;
        match self.config.workspace.ai_commit_model.as_deref().filter(|model| !model.is_empty()) {
            Some(model) if command.trim() == "opencode" => Some(format!("opencode run --model {model}")),
            Some(model) if command.starts_with("opencode ") => Some(format!("{command} --model {model}")),
            _ => Some(command),
        }
    }

    fn focused_cwd(&self) -> Option<PathBuf> {
        self.tabs.get(self.active).and_then(|t| t.focused_entry()).and_then(PaneEntry::cwd)
    }

    fn write_focused(&mut self, bytes: Vec<u8>) {
        if let Some(pane) = self.focused_pane() {
            pane.term.lock().scroll_display(Scroll::Bottom);
            pane.write(bytes);
        }
        if let Some(view) = self.focused_view_mut() {
            view.reset_blink();
        }
    }

    fn scroll_focused(&mut self, scroll: Scroll) {
        if let Some(pane) = self.focused_pane() {
            pane.term.lock().scroll_display(scroll);
        }
    }

    fn focus_pane(&mut self, id: PaneId) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if tab.focused == id {
            return;
        }
        let previous = tab.focused;
        if let Some(pane) = tab.pane(previous).and_then(PaneEntry::live) {
            if pane.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
                pane.write(b"\x1b[O".to_vec());
            }
        }
        tab.set_focus(id);
        if let Some(pane) = tab.pane(id).and_then(PaneEntry::live) {
            if pane.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
                pane.write(b"\x1b[I".to_vec());
            }
        }
        tab.has_activity = false;
    }

    fn split_focused(&mut self, dir: Dir) {
        if let Some(id) = self.focused_id() {
            self.split_pane(id, dir);
        }
    }

    fn split_pane(&mut self, target: PaneId, dir: Dir) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let Some(entry) = tab.pane(target) else { return };
        let profile_id = entry.profile_id.clone();
        let cwd = entry.cwd();
        // A profile removed from the config after this pane started must not
        // make the split silently do nothing: fall back as restore does.
        let profile =
            self.profiles.iter().find(|p| p.id == profile_id).cloned().unwrap_or_else(|| self.default_profile());
        let id = self.alloc_pane_id();
        let entry = self.spawn_entry(id, &profile, cwd);
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let inserted = match dir {
            Dir::Row => tab.tree.split_right(target, id),
            Dir::Column => tab.tree.split_down(target, id),
        };
        if inserted {
            tab.panes.insert(id, entry);
            tab.set_focus(id);
        }
        self.mark_session_dirty();
    }

    fn close_pane(&mut self, id: PaneId) {
        self.close_pane_in(self.active, id);
    }

    fn close_pane_in(&mut self, index: usize, id: PaneId) {
        let Some(tab) = self.tabs.get_mut(index) else { return };
        if tab.remove_pane(id) {
            self.close_tab(index);
            return;
        }
        self.mark_session_dirty();
    }

    /// Closes pane `id` wherever it lives now. Pane ids are unique across
    /// tabs, while tab indices shift as soon as an earlier tab closes.
    fn close_pane_anywhere(&mut self, id: PaneId) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.panes.contains_key(&id)) {
            self.close_pane_in(index, id);
        }
    }

    fn collapse_pane(&mut self, id: PaneId) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if tab.panes.len() <= 1 || tab.collapsed.iter().any(|(pane, _)| *pane == id) {
            return;
        }
        let Some(anchor) = tab.tree.remove(id) else { return };
        tab.collapsed.push((id, anchor));
        if tab.focused == id {
            tab.focused = tab.tree.panes().first().copied().unwrap_or(id);
        }
        if tab.maximized == Some(id) {
            tab.maximized = None;
        }
        self.mark_session_dirty();
    }

    fn restore_collapsed(&mut self, id: PaneId) {
        let active = self.active;
        self.restore_collapsed_in(active, id);
    }

    /// Restores a pane hidden by Ctrl+Alt+C. The pane may live in a
    /// background tab (the collapsed list offers panes from every
    /// tab), so the tab is activated first.
    fn restore_collapsed_in(&mut self, tab: usize, id: PaneId) {
        if tab != self.active {
            if self.tabs.get(tab).is_none() {
                return;
            }
            self.active = tab;
            self.settings_open = false;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let Some(index) = tab.collapsed.iter().position(|(pane, _)| *pane == id) else { return };
        let (_, anchor) = tab.collapsed.remove(index);
        let fallback = tab.focused;
        if tab.tree.restore(id, anchor, fallback) {
            tab.focused = id;
        }
        self.mark_session_dirty();
    }

    fn navigate_pane(&mut self, dx: i32, dy: i32) {
        let nav = match (dx, dy) {
            (-1, _) => crate::layout::split_tree::NavDir::Left,
            (1, _) => crate::layout::split_tree::NavDir::Right,
            (_, -1) => crate::layout::split_tree::NavDir::Up,
            _ => crate::layout::split_tree::NavDir::Down,
        };
        let area = self.last_tab_area;
        let Some(tab) = self.tabs.get(self.active) else { return };
        if let Some(id) = tab.tree.navigate(
            tab.focused,
            nav,
            crate::layout::split_tree::Rect::new(area.min.x, area.min.y, area.width(), area.height()),
            theme::DIVIDER_WIDTH,
        ) {
            self.focus_pane(id);
        }
    }

    fn navigate_cycle(&mut self, forward: bool) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let next = if forward { tab.tree.next(tab.focused) } else { tab.tree.previous(tab.focused) };
        if let Some(id) = next {
            self.focus_pane(id);
        }
    }

    fn selection_text(&self, id: PaneId) -> Option<String> {
        let pane = self.pane(id)?;
        let term = pane.term.lock();
        term.selection_to_string().filter(|text| !text.is_empty())
    }

    fn set_clipboard(&self, text: &str) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            if let Err(e) = clipboard.set_text(text.to_owned()) {
                log::warn!("clipboard write failed: {e}");
            }
        }
    }

    fn clipboard_text(&self) -> Option<String> {
        let mut clipboard = arboard::Clipboard::new().ok()?;
        clipboard.get_text().ok().filter(|text| !text.is_empty())
    }

    fn paste_clipboard(&mut self, id: PaneId) {
        if let Some(text) = self.clipboard_text() {
            self.paste_text(id, text, false);
        }
    }

    fn paste_text(&mut self, id: PaneId, text: String, confirmed: bool) {
        if self.ui.dialog.is_some() {
            return;
        }
        let Some(pane) = self.pane(id) else { return };
        let bracketed = pane.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if !confirmed && paste::needs_confirmation(&text, bracketed) {
            self.ui.dialog = Some(DialogState::Paste { pane_id: id, text });
            if let Some(repaint) = &self.repaint {
                repaint();
            }
            return;
        }
        let bytes = paste::prepare_paste(&text, bracketed);
        pane.term.lock().scroll_display(Scroll::Bottom);
        pane.write(bytes);
    }

    /// A file or folder dropped on the window (Helm's path drop): its path,
    /// quoted for the pane's shell, is typed into the pane under the drop point
    /// (else the focused one), which takes the focus.
    pub fn drop_path(&mut self, path: &Path, at: Option<Pos2>) {
        if self.settings_open {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else { return };
        let id = at.and_then(|pos| tab.pane_at(pos)).unwrap_or(tab.focused);
        let Some(entry) = tab.pane(id) else { return };
        let quoting = self
            .profiles
            .iter()
            .find(|profile| profile.id == entry.profile_id)
            .map_or(paste::PathQuoting::Unix, profiles::path_quoting);
        self.paste_text(id, paste::quote_path(&path.to_string_lossy(), quoting), false);
        self.focus_pane(id);
    }

    fn select_all(&mut self, id: PaneId) {
        let Some(pane) = self.pane(id) else { return };
        let mut term = pane.term.lock();
        let history = term.grid().history_size();
        let lines = term.screen_lines();
        let columns = term.columns();
        let start = Point::new(Line(-(history as i32)), Column(0));
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(Point::new(Line(lines as i32 - 1), Column(columns.saturating_sub(1))), Side::Right);
        term.selection = Some(selection);
    }

    fn clear_pane(&mut self, id: PaneId) {
        let Some(pane) = self.pane(id) else { return };
        let mut term = pane.term.lock();
        term.grid_mut().clear_history();
        let mut processor: alacritty_terminal::vte::ansi::Processor<alacritty_terminal::vte::ansi::StdSyncHandler> =
            alacritty_terminal::vte::ansi::Processor::new();
        processor.advance(&mut *term, b"\x1b[2J\x1b[H");
        term.selection = None;
    }

    fn alloc_pane_id(&mut self) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        id
    }

    fn spawn_entry(&mut self, id: PaneId, profile: &Profile, cwd: Option<PathBuf>) -> PaneEntry {
        if profile.kind == profiles::ProfileKind::Wsl && !self.builtin_profiles.iter().any(|p| p.id == profile.id) {
            self.builtin_profiles.push(profile.clone());
            self.profiles = self.build_profiles();
        }
        // The folder of a split or a new tab is what the program in the pane
        // last printed, and a restored one is what a file said: neither may
        // send the next shell to another machine. The profile's own folder is
        // the user's setting and stays as configured.
        let cwd = session::usable_cwd(cwd.as_deref());
        let version = env!("CARGO_PKG_VERSION");
        let env =
            profiles::pane_env(profile, id, Some(&self.status_dir), version, self.inherited_prompt_command.as_deref());
        let options = SpawnOptions {
            pane_id: id,
            program: profile.command.clone(),
            args: profile.args.clone(),
            // Unknown pane folder: the profile's, else the user profile (11.2).
            cwd: cwd.or_else(|| profile.cwd.clone()).or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from)),
            env,
            columns: 100,
            lines: 30,
            cell_width: 9,
            cell_height: 18,
            scrollback: self.config.terminal.scrollback,
            word_separators: self.config.terminal.word_separators.clone(),
            allow_osc52: self.config.terminal.allow_osc52,
            palette: self.palette.clone(),
            cursor_style: cursor_style(&self.config.terminal.cursor),
        };
        // `on_start` installs the real repaint before any pane can be spawned
        // (tabs start empty and the first ones are created there). A pane built
        // without one would hold a no-op that still latches `wake_pending`, so
        // its output would wait for an unrelated repaint; say so instead of
        // failing silently if that ever stops holding.
        let repaint = self.repaint.clone().unwrap_or_else(|| {
            log::error!("a pane was created before the window started; its repaints are no-ops");
            Arc::new(|| {})
        });
        let view = TerminalView::new(self.config.font.size);
        let start_cwd = options.cwd.clone();
        let base = PaneEntry {
            content: PaneContent::Error(String::new()),
            view,
            workspace: crate::workspace::Workspace::default(),
            start_cwd,
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            title: String::new(),
            exited: false,
            claude: None,
            claude_mtime: None,
            has_claude: false,
        };
        match Pane::spawn(options, repaint) {
            Ok(pane) => PaneEntry { content: PaneContent::Live(pane), ..base },
            Err(e) => {
                log::error!("cannot start {}: {e}", profile.command);
                PaneEntry { content: PaneContent::Error(format!("{}: {e}", strings::SPAWN_FAILED)), ..base }
            }
        }
    }

    fn new_tab(&mut self, profile: &Profile, cwd: Option<PathBuf>) -> Tab {
        let id = self.alloc_pane_id();
        let entry = self.spawn_entry(id, profile, cwd);
        let mut panes = HashMap::new();
        panes.insert(id, entry);
        Tab::new(SplitTree::new(id), panes, id)
    }

    fn new_tab_at_end(&mut self, profile: &Profile, cwd: Option<PathBuf>) {
        let tab = self.new_tab(profile, cwd);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.settings_open = false;
        self.mark_session_dirty();
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        self.tabbar.tab_removed(index);
        let state = self.tab_state(&tab);
        self.closed_tabs.push(ClosedTab { state });
        if self.closed_tabs.len() > 10 {
            self.closed_tabs.remove(0);
        }
        drop(tab);
        if self.tabs.is_empty() {
            let profile = self.default_profile();
            let tab = self.new_tab(&profile, None);
            self.tabs.push(tab);
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
        self.mark_session_dirty();
    }

    fn close_other_tabs(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        for closed in (0..self.tabs.len()).rev().filter(|closed| *closed != index) {
            self.tabbar.tab_removed(closed);
        }
        let keep = self.tabs.remove(index);
        let others: Vec<Tab> = self.tabs.drain(..).collect();
        for tab in others {
            let state = self.tab_state(&tab);
            self.closed_tabs.push(ClosedTab { state });
            if self.closed_tabs.len() > 10 {
                self.closed_tabs.remove(0);
            }
            drop(tab);
        }
        self.tabs.push(keep);
        self.active = 0;
        self.mark_session_dirty();
    }

    fn duplicate_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let state = self.tab_state(&self.tabs[index]);
        if let Some(tab) = self.restore_tab(&state) {
            let title = self.tabs[index].custom_title.clone();
            let mut tab = tab;
            tab.custom_title = title;
            self.tabs.insert(index + 1, tab);
            self.tabbar.tab_inserted(index + 1);
            self.active = index + 1;
            self.settings_open = false;
            self.mark_session_dirty();
        }
    }

    fn reopen_tab(&mut self) {
        if let Some(closed) = self.closed_tabs.pop() {
            if let Some(tab) = self.restore_tab(&closed.state) {
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.settings_open = false;
                self.mark_session_dirty();
            }
        }
    }

    fn cycle_tab(&mut self, delta: i32) {
        if self.tabs.len() < 2 {
            return;
        }
        let len = self.tabs.len() as i32;
        self.active = ((self.active as i32 + delta).rem_euclid(len)) as usize;
        self.settings_open = false;
    }

    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.tabbar.tab_moved(from, to);
        if self.active == from {
            self.active = to;
        } else if from < self.active && to >= self.active {
            self.active -= 1;
        } else if from > self.active && to <= self.active {
            self.active += 1;
        }
        self.mark_session_dirty();
    }

    fn restore_tab(&mut self, state: &TabState) -> Option<Tab> {
        let mut entries: HashMap<PaneId, PaneEntry> = HashMap::new();
        let mut order: Vec<PaneId> = Vec::new();
        let node = {
            let entries = &mut entries;
            let order = &mut order;
            state.layout.to_node(&mut |pane_state: &PaneState| {
                let id = self.alloc_pane_id();
                let profile = self
                    .profiles
                    .iter()
                    .find(|p| p.id == pane_state.profile_id)
                    .cloned()
                    .or_else(|| {
                        pane_state
                            .profile_id
                            .strip_prefix("wsl-")
                            .filter(|distro| !distro.is_empty())
                            .map(profiles::wsl_profile)
                    })
                    .unwrap_or_else(|| self.default_profile());
                let mut entry = self.spawn_entry(id, &profile, pane_state.cwd.clone());
                entry.workspace.open = pane_state.workspace_open;
                if let Some(width) = pane_state.workspace_width {
                    entry.workspace.width = crate::workspace::clamp_width(width);
                }
                if let Some(tab) = &pane_state.workspace_tab {
                    entry.workspace.tab = crate::workspace::PanelTab::parse(tab);
                }
                entries.insert(id, entry);
                order.push(id);
                id
            })
        };
        if order.is_empty() {
            return None;
        }
        let tree = SplitTree::from_root(node);
        let focused = order.get(state.focused).copied().unwrap_or(order[0]);
        let mut tab = Tab::new(tree, entries, focused);
        tab.custom_title = state.custom_title.clone();
        tab.color = state.color;
        // Hidden panes come back hidden: their shell, cwd and workspace state
        // are saved like any other pane's, and discarding them on restart
        // silently loses the pane the user collapsed to get out of the way.
        for saved in &state.collapsed {
            let id = self.alloc_pane_id();
            let profile = self
                .profiles
                .iter()
                .find(|p| p.id == saved.pane.profile_id)
                .cloned()
                .or_else(|| {
                    saved.pane.profile_id.strip_prefix("wsl-").filter(|d| !d.is_empty()).map(profiles::wsl_profile)
                })
                .unwrap_or_else(|| self.default_profile());
            let mut entry = self.spawn_entry(id, &profile, saved.pane.cwd.clone());
            entry.workspace.open = saved.pane.workspace_open;
            if let Some(width) = saved.pane.workspace_width {
                entry.workspace.width = crate::workspace::clamp_width(width);
            }
            if let Some(name) = &saved.pane.workspace_tab {
                entry.workspace.tab = crate::workspace::PanelTab::parse(name);
            }
            let neighbor = order.get(saved.neighbor).copied().unwrap_or(focused);
            tab.collapsed.push((id, Anchor { neighbor, dir: saved.dir, after: saved.after, fraction: saved.fraction }));
            tab.panes.insert(id, entry);
        }
        Some(tab)
    }

    fn tab_state(&self, tab: &Tab) -> TabState {
        let describe = |id: PaneId| {
            tab.pane(id)
                .map(|entry| PaneState {
                    profile_id: entry.profile_id.clone(),
                    cwd: entry.cwd(),
                    workspace_open: entry.workspace.open,
                    workspace_width: Some(entry.workspace.width),
                    workspace_tab: Some(entry.workspace.tab.as_str().to_owned()),
                })
                .unwrap_or(PaneState {
                    profile_id: String::new(),
                    cwd: None,
                    workspace_open: false,
                    workspace_width: None,
                    workspace_tab: None,
                })
        };
        let focused = tab.tree.panes().iter().position(|id| *id == tab.focused).unwrap_or(0);
        let order = tab.tree.panes();
        let collapsed = tab
            .collapsed
            .iter()
            .map(|(id, anchor)| session::CollapsedPane {
                pane: describe(*id),
                neighbor: order.iter().position(|pane| *pane == anchor.neighbor).unwrap_or(focused),
                dir: anchor.dir,
                after: anchor.after,
                fraction: anchor.fraction,
            })
            .collect();
        TabState {
            layout: crate::session::SavedNode::from_node(tab.tree.root(), &describe),
            focused,
            custom_title: tab.custom_title.clone(),
            color: tab.color,
            collapsed,
        }
    }

    // ---- config, fonts, session -----------------------------------------

    /// Applies a configuration. `save` is set for edits made in the settings
    /// page (they must persist); external reloads never write the file back.
    fn apply_config(&mut self, ctx: egui::Context, config: Config, save: bool) {
        let family_changed = config.font.family != self.config.font.family;
        let scheme_changed = config.color_scheme != self.config.color_scheme;
        let profiles_changed =
            config.profiles != self.config.profiles || config.default_profile != self.config.default_profile;
        let claude_disabled = self.config.claude_status.enabled && !config.claude_status.enabled;
        let claude_enabled = !self.config.claude_status.enabled && config.claude_status.enabled;
        let previous_integration = claude_disabled.then(|| self.config.claude_status.clone());
        let quota_switches_changed = config.quota.providers != self.config.quota.providers;
        let hotkeys_changed = config.hotkeys != self.config.hotkeys;
        self.config = config;
        if profiles_changed {
            self.profiles = self.build_profiles();
        }
        if claude_disabled {
            if let Some(previous) = previous_integration {
                self.config.claude_status.previous_status_line = previous.previous_status_line;
                self.config.claude_status.installed_command = previous.installed_command;
                self.config.claude_status.installed_settings_path = previous.installed_settings_path;
            }
            self.disable_claude();
        } else if claude_enabled {
            self.setup_claude();
        }
        if save {
            self.save_config_now();
        }
        // The keymap is a function of the hotkeys alone: rebuilding it (and
        // repeating its warnings) on every settings edit would report the same
        // problem again for each keystroke typed in an unrelated field.
        if hotkeys_changed {
            let (keymap, problems) = Keymap::with_overrides(&self.config.hotkeys);
            self.keymap = keymap;
            if !problems.is_empty() {
                for problem in &problems {
                    log::warn!("config hotkeys: {problem}");
                }
                self.toast(strings::UNKNOWN_HOTKEYS.to_owned());
            }
        }
        if family_changed {
            let report = fonts::install(&ctx, &self.config.font.family, &self.font_entries, self.fallbacks_loaded);
            for missing in report.missing {
                log::info!("font not found: {missing}");
            }
        }
        if scheme_changed {
            self.palette = scheme_palette(&self.config);
            // The chrome follows the scheme, as the terminal does.
            theme::set_scheme(&self.palette);
            theme::apply(&ctx);
        }
        let palette = self.palette.clone();
        let style = cursor_style(&self.config.terminal.cursor);
        let scrollback = self.config.terminal.scrollback;
        let word_separators = self.config.terminal.word_separators.clone();
        let font_size = self.config.font.size;
        for tab in &mut self.tabs {
            for entry in tab.panes.values_mut() {
                entry.view.font_size = font_size;
                if family_changed {
                    entry.view.font_changed();
                }
                if let Some(pane) = entry.live() {
                    if scheme_changed {
                        pane.set_palette(palette.clone());
                    }
                    pane.set_options(style, scrollback, &word_separators, self.config.terminal.allow_osc52);
                }
            }
        }
        self.sync_quota();
        if quota_switches_changed {
            if let Some(quota) = &self.quota {
                quota.refresh();
            }
        }
        ctx.request_repaint();
    }

    fn check_config(&mut self, ctx: &egui::Context) {
        // Typing that is not on disk yet is newer than any file there: it is
        // written first, and a reload would throw it away.
        if self.config_pending.is_some_and(|pending| pending.failures == 0) {
            return;
        }
        if self.config_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.config_checked = Instant::now();
        let path = self.config_path.clone();
        let mtime = file_mtime(&path);
        if mtime == self.config_mtime {
            return;
        }
        match Config::load_for_reload(&path) {
            Ok(config) => {
                if self.config_reload_log.recovered() {
                    log::info!("config.json reloaded");
                }
                self.config_mtime = mtime;
                // The file wins over an edit that could not be written.
                self.config_pending = None;
                self.apply_config(ctx.clone(), config, false);
            }
            Err(e) => {
                // Keep the active configuration and retry: the file may be
                // half-written by an editor right now. Said once per state of
                // the file, not once per tick while it stays broken.
                if self.config_reload_log.failed(mtime, &e) {
                    log::warn!("config.json not reloaded: {e}");
                }
            }
        }
    }

    fn absorb_wsl_profiles(&mut self) {
        let Some(rx) = &self.wsl_results else { return };
        match rx.try_recv() {
            Ok(found) => {
                for profile in found {
                    if !self.builtin_profiles.iter().any(|p| p.id == profile.id) {
                        self.builtin_profiles.push(profile);
                    }
                }
                self.wsl_results = None;
                self.profiles = self.build_profiles();
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.wsl_results = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Adopts a finished process enumeration and answers whether one was
    /// waiting. A failed enumeration is not adopted: the previous snapshot
    /// stays, and the caller retries on its next tick.
    #[must_use]
    fn absorb_proc_snapshot(&mut self) -> bool {
        match take_ready(&mut self.proc_results) {
            Some(Some(found)) => {
                self.proc_snapshot = found;
                true
            }
            _ => false,
        }
    }

    fn build_profiles(&self) -> Vec<Profile> {
        let mut profiles = self.builtin_profiles.clone();
        if let Some(distro) = self.config.default_profile.strip_prefix("wsl-").filter(|distro| !distro.is_empty()) {
            if !profiles.iter().any(|profile| profile.id == self.config.default_profile) {
                profiles.push(profiles::wsl_profile(distro));
            }
        }
        for custom in &self.config.profiles {
            let profile = Profile {
                id: custom.id.clone(),
                name: custom.name.clone(),
                command: custom.command.clone(),
                args: custom.args.clone(),
                cwd: custom.cwd.clone(),
                env: custom.env.clone(),
                kind: profiles::ProfileKind::Custom,
            };
            match profiles.iter_mut().find(|p| p.id == profile.id) {
                Some(slot) => *slot = profile,
                None => profiles.push(profile),
            }
        }
        profiles
    }

    fn default_profile(&self) -> Profile {
        self.profiles
            .iter()
            .find(|p| p.id == self.config.default_profile)
            .or_else(|| self.profiles.iter().find(|p| p.kind == profiles::ProfileKind::PowerShell))
            .or_else(|| self.profiles.first())
            .cloned()
            .unwrap_or_else(|| Profile {
                id: "cmd".into(),
                name: "cmd".into(),
                command: "cmd.exe".into(),
                args: Vec::new(),
                cwd: None,
                env: Default::default(),
                kind: profiles::ProfileKind::Cmd,
            })
    }

    fn mark_session_dirty(&mut self) {
        self.session_dirty = Some(Instant::now());
    }

    fn save_session(&mut self) {
        if self.extra_window {
            return;
        }
        let states: Vec<TabState> = {
            let tabs = &self.tabs;
            tabs.iter().map(|tab| self.tab_state(tab)).collect()
        };
        self.session.tabs = states;
        self.session.active_tab = self.active;
        if let Some(mut window) = self.window_state {
            window.maximized = self.window_maximized;
            self.session.window = Some(window);
        }
        if let Err(e) = self.session.save(&SessionState::path()) {
            log::warn!("cannot save the session: {e}");
        }
    }

    // ---- Claude status ---------------------------------------------------

    fn setup_claude_explicit(&mut self) {
        self.config.claude_status.declined_command = None;
        self.setup_claude();
    }

    /// Startup may inspect an existing opt-in, but never writes global settings.
    fn setup_claude(&mut self) {
        if !self.config.claude_status.enabled || self.ui.dialog.is_some() {
            return;
        }
        let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
            return;
        };
        if !exe_dir.join("anvil-claude-status.exe").is_file() {
            self.toast(strings::CLAUDE_HELPER_MISSING.to_owned());
            return;
        }
        let ours = claude_setup::status_command(&exe_dir);
        let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let Some(path) = claude_setup::settings_path(claude_dir.as_deref(), home.as_deref()) else { return };
        if self.config.claude_status.installed_command.is_some()
            && self.config.claude_status.installed_settings_path.as_ref().is_some_and(|installed| *installed != path)
        {
            self.toast(strings::CLAUDE_DIRECTORY_CHANGED.to_owned());
            return;
        }
        let settings = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("cannot read {}: {e}", path.display());
                self.toast(strings::CLAUDE_SETTINGS_BROKEN.to_owned());
                return;
            }
        };
        let (current, keep_previous) =
            match claude_setup::plan(settings.as_deref(), &ours, self.config.claude_status.declined_command.as_deref())
            {
                Plan::Install => (String::new(), false),
                Plan::Update => {
                    let current = settings
                        .as_deref()
                        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
                        .and_then(|value| value.get("statusLine")?.get("command")?.as_str().map(str::to_owned))
                        .unwrap_or_default();
                    (current, true)
                }
                Plan::AskReplace { current } => (current, false),
                Plan::Broken(message) => {
                    log::warn!("{message}");
                    self.toast(strings::CLAUDE_SETTINGS_BROKEN.to_owned());
                    return;
                }
                Plan::AlreadyInstalled | Plan::Declined => return,
            };
        if self.config.claude_status.declined_command.as_deref() == Some(current.as_str()) {
            return;
        }
        self.ui.dialog = Some(DialogState::ClaudeInstall { path, expected: settings, ours, current, keep_previous });
    }

    fn install_claude(
        &mut self,
        path: &Path,
        ours: &str,
        expected: Option<&str>,
        keep_previous: bool,
    ) -> std::io::Result<()> {
        if !Path::new(ours.trim_matches('"')).is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Claude helper was moved or removed before confirmation",
            ));
        }
        let (text, previous) = claude_setup::install(expected, ours).map_err(std::io::Error::other)?;
        claude_setup::write_confirmed(path, expected, &text)?;
        if !keep_previous {
            self.config.claude_status.previous_status_line = previous;
        }
        self.config.claude_status.installed_command = Some(ours.to_owned());
        self.config.claude_status.installed_settings_path = Some(path.to_path_buf());
        self.config.claude_status.declined_command = None;
        self.write_config()
    }

    fn disable_claude(&mut self) {
        // Consent belongs to the installed location, not today's executable or
        // CLAUDE_CONFIG_DIR. Moving ANVIL must not prevent restoring the original.
        let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let path = self
            .config
            .claude_status
            .installed_settings_path
            .clone()
            .or_else(|| claude_setup::settings_path(claude_dir.as_deref(), home.as_deref()));
        let Some(path) = path else { return };
        let Ok(fresh) = std::fs::read_to_string(&path) else { return };
        let ours = self.config.claude_status.installed_command.clone().or_else(|| {
            // Older opted-in configurations did not record their helper path.
            serde_json::from_str::<serde_json::Value>(&fresh)
                .ok()
                .and_then(|value| value.get("statusLine")?.get("command")?.as_str().map(str::to_owned))
                .filter(|command| claude_setup::is_anvil_status_command(command))
        });
        let Some(ours) = ours else { return };
        match claude_setup::uninstall(&fresh, &ours, self.config.claude_status.previous_status_line.as_ref()) {
            Ok(Some(text)) => {
                if let Err(e) = claude_setup::write_confirmed(&path, Some(&fresh), &text) {
                    // Keep the saved original: a later disable must be able to retry.
                    log::warn!("cannot restore {}: {e}", path.display());
                    self.toast(strings::CLAUDE_SETTINGS_CHANGED.to_owned());
                    return;
                }
                self.config.claude_status.previous_status_line = None;
                self.config.claude_status.installed_command = None;
                self.config.claude_status.installed_settings_path = None;
                self.save_config_now();
            }
            Ok(None) => {}
            Err(e) => log::warn!("{e}"),
        }
    }

    fn poll_statuses(&mut self) {
        if self.last_status_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_status_poll = Instant::now();
        // A finished enumeration is adopted first, so the pane walk below reads
        // the snapshot the `polled_processes` branch then annotates.
        let adopted = self.absorb_proc_snapshot();
        // Only a snapshot adopted in this very poll is current. The poll that
        // merely starts the next enumeration still reads the previous one (or
        // the empty startup one), which cannot know about an agent started since.
        let fresh_since = if adopted { self.proc_requested } else { None };
        let polled_processes = adopted || self.last_proc_poll.elapsed() >= Duration::from_secs(3);
        if polled_processes {
            if !adopted {
                self.last_proc_poll = Instant::now();
                // One enumeration at a time: a slow Win32 call must not stack up.
                if self.proc_results.is_none() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.proc_results = Some(rx);
                    self.proc_requested = Some(SystemTime::now());
                    std::thread::spawn(move || {
                        let _ = tx.send(crate::procs::snapshot());
                    });
                }
            }
            self.ai_commands.clear();
            for tab in &mut self.tabs {
                for (id, entry) in &mut tab.panes {
                    let Some(pane) = entry.live() else { continue };
                    let names = crate::procs::detected_cli_names(&self.proc_snapshot, pane.shell_pid);
                    let has_claude = names.contains("claude");
                    if let Some(command) = ["claude", "opencode", "codex", "gemini", "aider"]
                        .into_iter()
                        .find(|name| names.contains(*name))
                    {
                        self.ai_commands.insert(*id, command.to_owned());
                    }
                    entry.has_claude = has_claude;
                }
            }
        }
        let status_dir = self.status_dir.clone();
        for tab in &mut self.tabs {
            for entry in tab.panes.values_mut() {
                let pane_id = match &entry.content {
                    PaneContent::Live(pane) => pane.id,
                    PaneContent::Error(_) => continue,
                };
                let path = StatusRecord::file_path(&status_dir, &pane_id.to_string());
                let mtime = file_mtime(&path);
                match (mtime, entry.claude_mtime) {
                    (Some(mtime), current) if Some(mtime) != current => {
                        entry.claude = StatusRecord::read(&path);
                        entry.claude_mtime = Some(mtime);
                    }
                    (None, _) => {
                        entry.claude = None;
                        entry.claude_mtime = None;
                    }
                    _ => {}
                }
                if entry.claude.is_some() && status_outlived_agent(fresh_since, entry.has_claude, entry.claude_mtime) {
                    entry.claude = None;
                    entry.claude_mtime = None;
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    // ---- overlays --------------------------------------------------------

    /// Ctrl+= / Ctrl+- / Ctrl+0 resize the text of the focused terminal, or the
    /// font-size setting itself while the settings page is up; either way the
    /// new size is shown, so the change cannot go unnoticed.
    fn zoom_text(&mut self, delta: f32, ctx: &egui::Context) {
        let configured = self.config.font.size;
        let size = if self.settings_open {
            let size = if delta == 0.0 {
                Config::default().font.size
            } else {
                (configured + delta.signum()).clamp(crate::term::view::MIN_FONT_SIZE, crate::term::view::MAX_FONT_SIZE)
            };
            self.config.font.size = size;
            let next = self.config.clone();
            self.apply_config(ctx.clone(), next, true);
            size
        } else {
            let Some(view) = self.focused_view_mut() else { return };
            if delta == 0.0 {
                view.font_size = configured;
            } else {
                view.zoom(delta.signum());
            }
            view.font_size
        };
        self.ui.toasts.retain(|toast| !toast.text.starts_with(strings::ZOOM_TOAST_PREFIX));
        self.toast(strings::zoom_toast(size));
    }

    fn toast(&mut self, text: String) {
        // One toast per message: an event that repeats every frame (a held
        // Backspace ringing the bell at the key-repeat rate) renews it instead
        // of stacking dozens that run off the top of the window.
        self.ui.toasts.retain(|toast| toast.text != text);
        self.ui.toasts.push(Toast { text, at: Instant::now() });
    }

    fn expire_toasts(&mut self) {
        self.ui.toasts.retain(|toast| toast.at.elapsed() < Duration::from_millis(TOAST_MS));
    }

    fn show_toasts(&mut self, ctx: &egui::Context) {
        if self.ui.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("anvil-toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-12.0, -12.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for toast in &self.ui.toasts {
                    egui::Frame::popup(ui.style()).fill(theme::colors().tab_active_bg).show(ui, |ui| {
                        ui.label(egui::RichText::new(&toast.text).color(theme::colors().text).font(theme::font(12.0)));
                    });
                }
            });
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn open_picker(&mut self, ctx: &egui::Context) {
        // One centred list at a time: two of them stack in the same place and
        // each answers the same Enter and Escape.
        self.ui.collapsed_list = None;
        self.ui.picker = Some(PickerState {
            filter: String::new(),
            selected: 0,
            focus: true,
            opened_pass: ctx.cumulative_pass_nr(),
        });
    }

    fn show_picker(&mut self, ctx: &egui::Context) {
        let Some(picker) = self.ui.picker.as_mut() else { return };
        let names: Vec<(String, String)> = self.profiles.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        let outcome = profile_picker::show(ctx, picker, &names);
        match outcome {
            profile_picker::PickerOutcome::None => {}
            profile_picker::PickerOutcome::Closed => self.ui.picker = None,
            profile_picker::PickerOutcome::Selected(id) => {
                self.ui.picker = None;
                if let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() {
                    let cwd = self.focused_cwd();
                    self.new_tab_at_end(&profile, cwd);
                }
            }
        }
    }

    fn open_collapsed_list(&mut self, ctx: &egui::Context) {
        self.ui.picker = None;
        self.ui.collapsed_list = Some(CollapsedListState { selected: 0, opened_pass: ctx.cumulative_pass_nr() });
    }

    fn show_collapsed_list(&mut self, ctx: &egui::Context) {
        let Some(list) = self.ui.collapsed_list.as_mut() else { return };
        // Every tab's hidden panes, so a pane collapsed in a
        // background tab can be found and brought back too.
        let rows: Vec<collapsed_list::CollapsedRow> = self
            .tabs
            .iter()
            .enumerate()
            .flat_map(|(index, tab)| {
                let tab_title = tab.title().to_owned();
                tab.collapsed.iter().filter_map(move |(pane, _)| {
                    let entry = tab.panes.get(pane)?;
                    let preview = entry.live().map(crate::tabs::screen_tail).unwrap_or_default();
                    Some(collapsed_list::CollapsedRow {
                        tab: index,
                        pane: *pane,
                        tab_title: tab_title.clone(),
                        pane_title: entry.title_text().to_owned(),
                        preview,
                    })
                })
            })
            .collect();
        let outcome = collapsed_list::show(ctx, list, &rows);
        match outcome {
            collapsed_list::CollapsedListOutcome::None => {}
            collapsed_list::CollapsedListOutcome::Closed => self.ui.collapsed_list = None,
            collapsed_list::CollapsedListOutcome::Restore { tab, pane } => {
                self.ui.collapsed_list = None;
                self.restore_collapsed_in(tab, pane);
            }
        }
    }

    fn show_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.ui.dialog.take() else { return };
        let outcome = dialogs::show(ctx, &dialog);
        match outcome {
            dialogs::DialogOutcome::None => {
                self.ui.dialog = Some(dialog);
            }
            dialogs::DialogOutcome::Cancel => {
                if let DialogState::ClaudeInstall { current, .. } = dialog {
                    self.config.claude_status.declined_command = Some(current);
                    self.save_config_now();
                }
            }
            dialogs::DialogOutcome::Accept => match dialog {
                DialogState::Paste { pane_id, text } => self.paste_text(pane_id, text, true),
                DialogState::ClaudeInstall { path, expected, ours, keep_previous, .. } => {
                    if self.config.claude_status.enabled {
                        if let Err(e) = self.install_claude(&path, &ours, expected.as_deref(), keep_previous) {
                            log::warn!("cannot update {}: {e}", path.display());
                            self.toast(strings::CLAUDE_SETTINGS_CHANGED.to_owned());
                        }
                    }
                }
            },
        }
    }
}

/// A pane's status record outlived the agent that wrote it: a process snapshot
/// requested after the record was written (`requested`, `None` when no fresh one
/// was adopted) shows no agent under the pane. A record written after the
/// request proves nothing about this snapshot, and an agent that started and
/// wrote its first status since the previous snapshot would lose its badge.
fn status_outlived_agent(requested: Option<SystemTime>, has_agent: bool, written: Option<SystemTime>) -> bool {
    match (requested, written) {
        (Some(requested), Some(written)) => !has_agent && written <= requested,
        _ => false,
    }
}

fn cursor_style(cursor: &CursorConfig) -> CursorStyle {
    CursorStyle {
        shape: match cursor.shape {
            CursorShapeConfig::Block => CursorShape::Block,
            CursorShapeConfig::Bar => CursorShape::Beam,
            CursorShapeConfig::Underline => CursorShape::Underline,
        },
        blinking: cursor.blink,
    }
}

pub fn scheme_palette(config: &Config) -> Palette {
    match config.color_scheme.as_str() {
        strings::SCHEME_LIGHT => Palette::light(),
        strings::SCHEME_DARK => Palette::dark(),
        name => config
            .custom_color_schemes
            .iter()
            .find(|scheme| scheme.name == name)
            .and_then(|scheme| {
                let colors: Vec<&str> = scheme.colors.iter().map(String::as_str).collect();
                // A malformed scheme still has to be visible as broken: the
                // settings combo keeps showing it as selected, so a silent
                // fallback to dark would look like the setting did not apply.
                match Palette::from_hex(&scheme.foreground, &scheme.background, &scheme.cursor, &colors) {
                    Ok(palette) => Some(palette),
                    Err(e) => {
                        log::warn!("colour scheme `{name}` is malformed, drawing dark: {e}");
                        None
                    }
                }
            })
            .unwrap_or_else(Palette::dark),
    }
}

pub fn scheme_names(config: &Config) -> Vec<String> {
    let mut names = vec![strings::SCHEME_DARK.to_owned(), strings::SCHEME_LIGHT.to_owned()];
    names.extend(config.custom_color_schemes.iter().map(|s| s.name.clone()));
    names
}

fn font_families(entries: &[(String, String)]) -> Vec<String> {
    let mut families: Vec<String> = entries
        .iter()
        .filter_map(|(name, _)| {
            let name = name
                .trim_end_matches(" (TrueType)")
                .trim_end_matches(" (OpenType)")
                .trim_end_matches(" (TrueType)")
                .to_owned();
            let lower = name.to_lowercase();
            let mono = lower.contains("mono") || lower.contains("consol") || lower.contains("courier");
            mono.then_some(name)
        })
        .collect();
    families.sort();
    families.dedup();
    families
}

fn window_icon() -> Option<winit::window::Icon> {
    let bytes: &[u8] = include_bytes!("../icons/anvil-256.png");
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    winit::window::Icon::from_rgba(image.into_raw(), width, height).ok()
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// `%LOCALAPPDATA%\anvil\run\<pid>` — where pane statuses are written.
fn status_dir() -> PathBuf {
    run_base().join(std::process::id().to_string())
}

fn run_base() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("anvil").join("run")
}

/// Creates this process's status directory and removes the directories of
/// ANVIL processes that are gone.
fn prepare_status_dir() -> (PathBuf, Option<PathBuf>) {
    let base = run_base();
    if std::fs::create_dir_all(&base).is_err() {
        return (status_dir(), None);
    }
    let prefix = format!("anvil-{}-", std::process::id());
    let Ok(directory) = tempfile::Builder::new().prefix(&prefix).tempdir_in(&base) else {
        return (status_dir(), None);
    };
    let mine = directory.keep();
    let _ = std::fs::write(mine.join(".anvil-owner"), b"ANVIL status v1\n");
    std::thread::spawn(move || {
        // Only a successful enumeration may decide what is gone: a failed
        // snapshot would otherwise mark this very process dead and remove the
        // directory it is about to write its status records into.
        if let Some(found) = crate::procs::snapshot() {
            let alive = found.into_iter().map(|p| p.pid).collect();
            remove_stale_status_dirs(&base, &alive);
        }
    });
    (mine.clone(), Some(mine))
}

fn remove_stale_status_dirs(base: &Path, alive: &std::collections::HashSet<u32>) {
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let pid = name
                .to_str()
                .and_then(|name| name.strip_prefix("anvil-"))
                .and_then(|name| name.split_once('-'))
                .and_then(|(pid, _)| pid.parse::<u32>().ok());
            if pid.is_some_and(|pid| !alive.contains(&pid)) {
                remove_owned_status_dir(&entry.path());
            }
        }
    }
}

/// Never recursively delete a directory based on its name/PID. A private run
/// marker and exclusively ordinary numeric JSON files are required; unexpected
/// contents (including reparse points) leave the whole directory untouched.
fn remove_owned_status_dir(dir: &Path) {
    let ordinary = |path: &Path| {
        std::fs::symlink_metadata(path).is_ok_and(|meta| {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                !meta.file_type().is_symlink() && meta.file_attributes() & 0x400 == 0
            }
            #[cfg(not(windows))]
            {
                !meta.file_type().is_symlink()
            }
        })
    };
    let marker = dir.join(".anvil-owner");
    if !ordinary(dir)
        || !ordinary(&marker)
        || crate::fsutil::read_limited(&marker, 32).ok().as_deref() != Some(b"ANVIL status v1\n")
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { return };
        let name = entry.file_name();
        let ours = name == ".anvil-owner"
            || name
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|byte| byte.is_ascii_digit()));
        if !ours || !ordinary(&entry.path()) || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return;
        }
        files.push(entry.path());
    }
    for file in files {
        if std::fs::remove_file(file).is_err() {
            return;
        }
    }
    let _ = std::fs::remove_dir(dir); // A concurrent/unknown child is never traversed.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_run_cleanup_preserves_foreign_and_unexpected_contents() {
        let base = tempfile::tempdir().unwrap();
        for name in ["123", "anvil-123-owned", "anvil-123-foreign", "anvil-456-live"] {
            std::fs::create_dir(base.path().join(name)).unwrap();
        }
        for name in ["anvil-123-owned", "anvil-123-foreign", "anvil-456-live"] {
            std::fs::write(base.path().join(name).join(".anvil-owner"), b"ANVIL status v1\n").unwrap();
        }
        std::fs::write(base.path().join("anvil-123-owned/1.json"), b"{}").unwrap();
        std::fs::create_dir(base.path().join("anvil-123-foreign/foreign-directory")).unwrap();
        remove_stale_status_dirs(base.path(), &std::collections::HashSet::from([456]));
        assert!(!base.path().join("anvil-123-owned").exists());
        assert!(base.path().join("123").exists());
        assert!(base.path().join("anvil-123-foreign/foreign-directory").exists());
        assert!(base.path().join("anvil-456-live").exists());
    }

    #[test]
    fn disabling_claude_from_settings_restores_the_saved_status_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let claude_path = dir.path().join("claude-settings.json");
        let ours = "\"C:/test/anvil-claude-status.exe\"";
        let previous = serde_json::json!({ "type": "command", "command": "test-previous" });
        std::fs::write(
            &claude_path,
            serde_json::to_vec(&serde_json::json!({
                "statusLine": { "type": "command", "command": ours }, "unrelated": true,
            }))
            .unwrap(),
        )
        .unwrap();
        let mut config = Config::default();
        config.quota.enabled = false;
        config.claude_status.enabled = true;
        config.claude_status.previous_status_line = Some(previous.clone());
        config.claude_status.installed_settings_path = Some(claude_path.clone());
        config.claude_status.installed_command = Some(ours.to_owned());
        let mut next = config.clone();
        next.claude_status.enabled = false;
        let mut app =
            AnvilApp::from_state(config, path.clone(), None, SessionState::default(), false, dir.path().into());
        app.settle_settings(
            &egui::Context::default(),
            next,
            &crate::settings_ui::SettingsOutcome { changed: true, ..Default::default() },
        );
        let restored: serde_json::Value = serde_json::from_slice(&std::fs::read(&claude_path).unwrap()).unwrap();
        assert_eq!(restored["statusLine"], previous);
        assert_eq!(restored["unrelated"], true);
        assert!(!app.config.claude_status.enabled);
        assert!(app.config.claude_status.installed_command.is_none());
        assert_eq!(Config::load(&path).config, app.config, "cleanup performed by apply_config must survive settlement");
    }

    #[test]
    fn settings_edits_preserve_live_values_transitions_and_the_file_opened_in_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = Config::default();
        config.quota.enabled = false; // No worker, credentials or network in this fixture.
        config.font.size = 21.0;
        config.default_profile = "custom".to_owned();
        config.profiles.push(crate::config::ProfileConfig {
            id: "custom".into(),
            name: "test".into(),
            command: "test-shell".into(),
            args: Vec::new(),
            cwd: None,
            env: Default::default(),
        });
        let mut app =
            AnvilApp::from_state(config.clone(), path.clone(), None, SessionState::default(), false, dir.path().into());
        app.profiles = app.build_profiles();
        let ctx = egui::Context::default();
        let outcome = crate::settings_ui::SettingsOutcome { open_config: true, ..Default::default() };
        app.settle_settings(&ctx, config.clone(), &outcome);
        assert_eq!(Config::load(&path).config, config, "opening config must not save a default placeholder");
        assert_eq!(app.config, config);
        let mut edited = config;
        edited.font.size = 24.0;
        edited.profiles.clear();
        edited.default_profile = Config::default().default_profile;
        app.settle_settings(&ctx, edited.clone(), &crate::settings_ui::SettingsOutcome { changed: true, ..outcome });
        assert_eq!(app.config, edited, "the edit must not be overwritten after apply_config");
        assert_eq!(Config::load(&path).config, edited);
        assert!(app.profiles.is_empty(), "compare against the old live profiles, not defaults");
    }

    fn bare_app(dir: &Path) -> AnvilApp {
        let mut config = Config::default();
        config.quota.enabled = false; // No worker, credentials or network in this fixture.
        AnvilApp::from_state(config, dir.join("config.json"), None, SessionState::default(), true, dir.into())
    }

    /// The profile list and the hidden-panes list share one place on screen and
    /// both answer Enter and Escape: opening one closes the other.
    #[test]
    fn the_centred_lists_do_not_stack() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = bare_app(dir.path());
        let ctx = egui::Context::default();
        app.open_picker(&ctx);
        app.open_collapsed_list(&ctx);
        assert!(app.ui.picker.is_none() && app.ui.collapsed_list.is_some());
        app.open_picker(&ctx);
        assert!(app.ui.picker.is_some() && app.ui.collapsed_list.is_none());
    }

    /// The hotkey problems are reported when the hotkeys change, not again by
    /// every unrelated settings edit (a keystroke in a text field applies the
    /// whole config).
    #[test]
    fn hotkey_problems_are_reported_when_the_hotkeys_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.quota.enabled = false;
        config.hotkeys.insert("new-tab".to_owned(), vec!["Ctrl-Hyper-T".to_owned()]);
        let mut app = AnvilApp::from_state(
            config.clone(),
            dir.path().join("config.json"),
            None,
            SessionState::default(),
            true,
            dir.path().into(),
        );
        let ctx = egui::Context::default();
        let reported = |app: &AnvilApp| app.ui.toasts.iter().any(|toast| toast.text == strings::UNKNOWN_HOTKEYS);
        assert!(reported(&app), "the problem is reported at startup");
        app.ui.toasts.clear();
        let mut edited = config.clone();
        edited.terminal.word_separators.push('/');
        app.apply_config(ctx.clone(), edited.clone(), false);
        assert!(!reported(&app), "an unrelated edit must not repeat it");
        edited.hotkeys.insert("close-tab".to_owned(), vec!["Ctrl-Hyper-W".to_owned()]);
        app.apply_config(ctx, edited, false);
        assert!(reported(&app), "a change of the hotkeys is checked again");
    }

    /// A message that repeats (a held Backspace rings the bell at the key-repeat
    /// rate) renews its toast: the old code stacked one per frame.
    #[test]
    fn a_repeated_message_renews_its_toast_instead_of_stacking() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = bare_app(dir.path());
        for _ in 0..50 {
            app.toast(strings::BELL.to_owned());
        }
        app.toast(strings::COPIED.to_owned());
        app.toast(strings::BELL.to_owned());
        let texts: Vec<_> = app.ui.toasts.iter().map(|toast| toast.text.as_str()).collect();
        assert_eq!(texts, [strings::COPIED, strings::BELL]);
    }

    #[test]
    fn a_status_record_is_stale_only_against_a_snapshot_requested_after_it_was_written() {
        let at = |secs| std::time::UNIX_EPOCH + Duration::from_secs(secs);
        assert!(status_outlived_agent(Some(at(10)), false, Some(at(9))), "the agent is gone");
        assert!(!status_outlived_agent(Some(at(10)), true, Some(at(9))), "the agent is still there");
        assert!(!status_outlived_agent(Some(at(10)), false, Some(at(11))), "written after the snapshot was asked for");
        assert!(!status_outlived_agent(None, false, Some(at(9))), "no fresh snapshot, no verdict");
        assert!(!status_outlived_agent(Some(at(10)), false, None));
    }

    /// The poll on the 3 s timer only *requests* the next enumeration and still
    /// reads the previous snapshot (empty at startup). That snapshot cannot list
    /// an agent started since, so the poll deleted the status record the agent
    /// had just written and its tab lost the badge until the next update.
    #[test]
    fn a_status_record_survives_the_poll_that_only_requests_a_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = bare_app(dir.path());
        app.repaint = Some(Arc::new(|| {}));
        let profile = app.default_profile();
        let id = app.alloc_pane_id();
        let entry = app.spawn_entry(id, &profile, Some(dir.path().to_path_buf()));
        assert!(entry.live().is_some(), "the fixture needs a running pane");
        app.tabs.push(Tab::new(SplitTree::new(id), HashMap::from([(id, entry)]), id));
        let path = StatusRecord::file_path(dir.path(), &id.to_string());
        let record = StatusRecord { model: Some("Opus".into()), ..StatusRecord::default() };
        record.write(dir.path(), &id.to_string()).unwrap();
        let poll = |app: &mut AnvilApp, since_process_poll: u64| {
            app.last_status_poll = Instant::now() - Duration::from_secs(5);
            app.last_proc_poll = Instant::now() - Duration::from_secs(since_process_poll);
            app.poll_statuses();
        };
        poll(&mut app, 0);
        assert!(app.tabs[0].panes[&id].claude.is_some(), "the record is read");
        poll(&mut app, 10);
        assert!(path.exists(), "a poll without a fresh snapshot must keep the record");
        assert!(app.tabs[0].panes[&id].claude.is_some());
        // A snapshot requested after the record was written and showing no agent:
        // now the record really is stale.
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Some(Vec::new())).unwrap();
        app.proc_results = Some(rx);
        app.proc_requested = Some(SystemTime::now() + Duration::from_secs(60));
        poll(&mut app, 0);
        assert!(!path.exists(), "the agent is gone, so is its record");
        assert!(app.tabs[0].panes[&id].claude.is_none());
    }

    /// What a new pane would start in when it has no usable folder of its own.
    fn fallback_cwd(profile: &Profile) -> Option<PathBuf> {
        profile.cwd.clone().or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
    }

    /// A pane's folder is whatever the program in it printed (OSC 7 and
    /// friends) or a session file said, and a split or a new tab hands it
    /// straight to the next shell's ConPTY. A network folder there makes the
    /// start itself connect to that host and authenticate to it.
    #[test]
    fn a_pane_never_starts_in_a_network_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = bare_app(dir.path());
        app.repaint = Some(Arc::new(|| {}));
        let profile = app.default_profile();
        for remote in [r"\\127.0.0.1\ANVIL-denied", "//127.0.0.1/ANVIL-denied", r"\\?\UNC\127.0.0.1\ANVIL-denied"] {
            let id = app.alloc_pane_id();
            let entry = app.spawn_entry(id, &profile, Some(PathBuf::from(remote)));
            assert_eq!(entry.start_cwd, fallback_cwd(&profile), "{remote}");
        }
        let id = app.alloc_pane_id();
        let entry = app.spawn_entry(id, &profile, Some(dir.path().to_path_buf()));
        assert_eq!(entry.start_cwd.as_deref(), Some(dir.path()), "a local folder is kept as it was reported");
    }

    /// The lexical check on the reported path sees `C:\repo\link` and nothing
    /// else: a symlink inside a checkout that points at a share passes it, and
    /// the shell would start on that share.
    #[cfg(windows)]
    #[test]
    fn a_link_to_a_network_share_is_not_a_start_folder() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        if let Err(error) = std::os::windows::fs::symlink_dir(r"\\127.0.0.1\ANVIL-denied", &link) {
            assert_eq!(error.raw_os_error(), Some(1314), "create link: {error}");
            eprintln!("symlink privilege is unavailable; skipping reparse fixture");
            return;
        }
        let mut app = bare_app(dir.path());
        app.repaint = Some(Arc::new(|| {}));
        let profile = app.default_profile();
        let id = app.alloc_pane_id();
        let entry = app.spawn_entry(id, &profile, Some(link.clone()));
        assert_eq!(entry.start_cwd, fallback_cwd(&profile));
        assert_eq!(session::usable_cwd(Some(&link)), None);
    }

    fn typing(app: &mut AnvilApp, ctx: &egui::Context, text: &str) {
        let mut next = app.config.clone();
        next.terminal.word_separators = text.to_owned();
        app.settle_settings(ctx, next, &crate::settings_ui::SettingsOutcome { typed: true, ..Default::default() });
    }

    fn separators_on_disk(path: &Path) -> String {
        Config::load(path).config.terminal.word_separators
    }

    /// A series of keystrokes in a text field is applied at once and reaches the
    /// disk in one write, when the field is left. Each of them used to write
    /// (and fsync) config.json on the UI thread.
    #[test]
    fn typing_applies_at_once_and_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut app = bare_app(dir.path());
        let ctx = egui::Context::default();
        let mut text = String::new();
        for key in "ab /cd".chars() {
            text.push(key);
            typing(&mut app, &ctx, &text);
            assert_eq!(app.config.terminal.word_separators, text, "applied at once");
        }
        assert!(!path.exists(), "a keystroke is not a write");
        let leave = crate::settings_ui::SettingsOutcome { commit: true, ..Default::default() };
        app.settle_settings(&ctx, app.config.clone(), &leave);
        assert_eq!(separators_on_disk(&path), "ab /cd", "the last value is what lands");
        std::fs::remove_file(&path).unwrap();
        app.flush_config();
        assert!(!path.exists(), "nothing is pending after the write");
    }

    /// Whatever ends the typing writes it: the page closing, the typing resting
    /// (or going on for too long), the window losing the focus, the app exiting.
    #[test]
    fn deferred_typing_is_written_wherever_the_file_has_to_be_current() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut app = bare_app(dir.path());
        app.settings_open = true;
        typing(&mut app, &ctx, "a ");
        app.flush_config_if_due(&ctx);
        assert!(!path.exists(), "typing on the open page waits");
        app.settings_open = false;
        app.flush_config_if_due(&ctx);
        assert_eq!(separators_on_disk(&path), "a ", "closing the page");

        app.settings_open = true;
        typing(&mut app, &ctx, "b");
        app.config_pending.as_mut().unwrap().last -= Duration::from_secs(2);
        app.flush_config_if_due(&ctx);
        assert_eq!(separators_on_disk(&path), "b", "the typing rested");

        typing(&mut app, &ctx, "c");
        app.config_pending.as_mut().unwrap().first -= Duration::from_secs(10);
        app.flush_config_if_due(&ctx);
        assert_eq!(separators_on_disk(&path), "c", "typing that never rests is still written");

        typing(&mut app, &ctx, "d");
        app.window_focus_changed(false);
        assert_eq!(separators_on_disk(&path), "d", "focus lost");

        typing(&mut app, &ctx, "e");
        app.on_exit(None);
        assert_eq!(separators_on_disk(&path), "e", "exit");
    }

    /// A write that fails keeps the edit, which is tried again; a reload does
    /// not throw typing away that is waiting for the disk.
    #[test]
    fn a_failed_or_waiting_write_is_not_lost_to_a_reload() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut app = bare_app(dir.path());
        std::fs::write(&path, "{}").unwrap();
        app.config_mtime = file_mtime(&path);
        typing(&mut app, &ctx, "typed");
        // Someone else saves the file meanwhile.
        std::fs::write(&path, r#"{"terminal":{"wordSeparators":"other"}}"#).unwrap();
        app.config_checked = Instant::now() - Duration::from_secs(5);
        app.check_config(&ctx);
        assert_eq!(app.config.terminal.word_separators, "typed", "no reload over waiting typing");
        app.flush_config();
        assert_eq!(separators_on_disk(&path), "typed");

        // A write that cannot happen (the target is a directory) stays pending.
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        app.config_path = blocked;
        typing(&mut app, &ctx, "kept");
        app.flush_config();
        assert!(app.config_pending.is_some_and(|pending| pending.failures == 1), "the edit is kept for a retry");
        app.config_path = path.clone();
        app.config_pending.as_mut().unwrap().last -= Duration::from_secs(10);
        app.flush_config_if_due(&ctx);
        assert_eq!(separators_on_disk(&path), "kept");
        assert!(app.config_pending.is_none());
    }

    #[test]
    fn a_pending_write_asks_for_a_frame_when_it_falls_due() {
        let now = Instant::now();
        let at = |ms: u64| now + Duration::from_millis(ms);
        let pending = PendingSave::new(now);
        assert_eq!(pending.wait(true, at(100)), Some(CONFIG_SAVE_IDLE - Duration::from_millis(100)));
        assert_eq!(pending.wait(true, at(700)), None, "rested");
        assert_eq!(pending.wait(false, at(1)), None, "the page is closed");
        // Typing goes on: the idle time keeps moving, the ceiling does not.
        let typing_on = pending.touched(at(2_500));
        assert_eq!(typing_on.wait(true, at(2_600)), Some(Duration::from_millis(400)), "the ceiling comes first");
        assert_eq!(typing_on.wait(true, at(3_000)), None, "too long behind the keys");
        // A failed write waits for the retry whatever the page does.
        let failed = PendingSave::failed(Some(pending), now, false);
        assert_eq!(failed.wait(false, at(1_000)), Some(CONFIG_SAVE_RETRY - Duration::from_secs(1)));
        assert_eq!(failed.wait(true, at(5_000)), None);
        assert_eq!(failed.touched(at(10)).failures, 0, "new typing is a new attempt");
        assert!(failed.touched(at(10)).reported, "but the series was reported");
    }

    /// The timer's retries of a write that keeps failing thin out, to a ceiling,
    /// and start over with new typing or an attempt of the user's.
    #[test]
    fn retries_of_a_failing_write_back_off_to_a_ceiling_and_start_over() {
        let secs = |n: u64| Duration::from_secs(n);
        let pauses: Vec<_> = (1..=9).map(PendingSave::retry_pause).collect();
        assert_eq!(
            pauses,
            [secs(5), secs(10), secs(20), secs(40), secs(80), secs(160), secs(300), secs(300), secs(300)]
        );
        assert_eq!(PendingSave::retry_pause(u32::MAX), CONFIG_SAVE_RETRY_MAX, "no overflow however long it fails");

        let now = Instant::now();
        let ms = Duration::from_millis;
        let mut pending = PendingSave::failed(None, now, false);
        let mut at = now;
        for pause in [5, 10, 20, 40, 80, 160, 300, 300] {
            let pause = secs(pause);
            assert_eq!(pending.wait(false, at + pause - ms(1)), Some(ms(1)), "not due before its pause");
            assert_eq!(pending.wait(false, at + pause), None, "due at it");
            at += pause;
            pending = PendingSave::failed(Some(pending), at, false);
        }
        // The user's attempt (focus lost, page closed, exit) starts the pauses over.
        let forced = PendingSave::failed(Some(pending), pending.last + secs(1), true);
        assert_eq!(forced.failures, 1);
        assert_eq!(forced.wait(false, forced.last), Some(CONFIG_SAVE_RETRY));
        assert_eq!(forced.first, now, "it is still the same edit");
        // And so does more typing, as an edit with a window of its own: the
        // failed edit's start must not make every keystroke due at once.
        let typed_at = pending.last + secs(10);
        let typed = pending.touched(typed_at);
        assert_eq!(typed.failures, 0);
        assert_eq!(typed.wait(true, typed_at), Some(CONFIG_SAVE_IDLE), "the idle rest applies again");
        assert_eq!(typed.wait(true, typed_at + CONFIG_SAVE_IDLE), None);
    }

    /// A write that cannot succeed is not retried on the timer before its pause,
    /// but the page closing, the window being deactivated and the exit always
    /// try, and a success ends the series.
    #[test]
    fn a_backed_off_write_is_still_made_when_the_user_leaves() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let mut app = bare_app(dir.path());
        app.config_path = blocked;
        app.settings_open = true;
        app.flush_config_if_due(&ctx);
        typing(&mut app, &ctx, "x");
        app.flush_config();
        assert_eq!(app.config_pending.map(|pending| pending.failures), Some(1));
        // The file becomes writable, but the timer is still backing off.
        app.config_path = path.clone();
        app.settings_open = true;
        app.flush_config_if_due(&ctx);
        assert!(!path.exists(), "the timer waits out its pause");
        // The page closes: the write is made without waiting for the pause.
        app.settings_open = false;
        app.flush_config_if_due(&ctx);
        assert_eq!(separators_on_disk(&path), "x");
        assert!(app.config_pending.is_none());
        // Deactivation and exit do not wait either.
        let leaves: [fn(&mut AnvilApp); 2] = [|app| app.window_focus_changed(false), |app| app.on_exit(None)];
        for (index, leave) in leaves.into_iter().enumerate() {
            let blocked = dir.path().join(format!("blocked-{index}"));
            std::fs::create_dir(&blocked).unwrap();
            app.config_path = blocked;
            typing(&mut app, &ctx, "y");
            app.flush_config();
            assert_eq!(app.config_pending.map(|pending| pending.failures), Some(1));
            app.config_path = path.clone();
            leave(&mut app);
            assert_eq!(separators_on_disk(&path), "y");
            assert!(app.config_pending.is_none());
        }
    }

    /// A config.json that stays broken is said once per state of the file, not on
    /// every tick: the first time, again when the error or the file changes, and
    /// the end of the failure once.
    #[test]
    fn a_broken_config_is_logged_when_its_state_changes() {
        let at = |secs| Some(std::time::UNIX_EPOCH + Duration::from_secs(secs));
        let mut log = ReloadLog::default();
        assert!(!log.recovered(), "nothing to recover from");
        assert!(log.failed(at(1), "expected value"), "first sight");
        for _ in 0..100 {
            assert!(!log.failed(at(1), "expected value"), "the same state is not news");
        }
        assert!(log.failed(at(1), "trailing comma"), "another error");
        assert!(log.failed(at(2), "trailing comma"), "the file changed and is still broken");
        assert!(log.failed(None, "no such file"), "the file vanished");
        assert!(log.recovered(), "fixed");
        assert!(!log.recovered(), "said once");
        assert!(log.failed(at(2), "trailing comma"), "broken again counts as new");
    }

    /// A finished enumeration is taken exactly once. The old code asked the
    /// channel whether a value was ready and then tried to read it: `try_recv`
    /// consumes, so the second read found nothing and every pane kept the
    /// empty startup snapshot, which in turn made the poll delete the status
    /// file of a running agent.
    #[test]
    fn a_finished_enumeration_is_consumed_exactly_once() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut slot = Some(rx);
        assert_eq!(take_ready(&mut slot), None, "nothing sent yet");
        tx.send(vec![1u64, 2]).unwrap();
        assert_eq!(take_ready(&mut slot), Some(vec![1u64, 2]));
        assert_eq!(take_ready(&mut slot), None, "the value must not be read twice");
        assert!(slot.is_none(), "a sent value finishes the slot");
    }

    /// A worker that panicked leaves nothing to read: the slot empties instead
    /// of claiming a value forever.
    #[test]
    fn a_disconnected_worker_clears_the_slot() {
        let (tx, rx) = std::sync::mpsc::channel::<u64>();
        let mut slot = Some(rx);
        drop(tx);
        assert_eq!(take_ready(&mut slot), None);
        assert!(slot.is_none(), "nothing more can arrive: the slot is finished");
    }
}
