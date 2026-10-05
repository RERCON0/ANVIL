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

use crate::chrome::dialogs::{self, DialogState};
use crate::chrome::profile_picker;
use crate::chrome::tabbar::{self, TabInfo};
use crate::chrome::titlebar::{resize_borders, title_bar};
use crate::claude_setup::{self, Plan};
use crate::claude_status::StatusRecord;
use crate::config::{Bell, Config, CursorConfig, CursorShapeConfig};
use crate::fonts;
use crate::hotkeys::{Action, Keymap};
use crate::host::route::KeyFocus;
use crate::host::WindowCommand;
use crate::layout::split_tree::{Dir, PaneId, SplitTree};
use crate::profiles::{self, Profile};
use crate::session::{self, PaneState, SessionState, TabState, WindowState};
use crate::strings;
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

use crate::term::pane::PaneEvent;
use crate::tabs::{FrameEnv, PaneContent, PaneEntry, Tab, TabAction};
use crate::term::input::{encode, InputModes, KeyPress};
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

#[derive(Default)]
pub struct UiState {
    pub toasts: Vec<Toast>,
    pub picker: Option<PickerState>,
    pub dialog: Option<DialogState>,
}

pub struct ClosedTab {
    pub state: TabState,
}

/// The quota block's rows with the inputs they were built from, so an unchanged
/// minute reuses them instead of relaying out the same text.
#[derive(Default)]
struct QuotaRows {
    version: u64,
    minute: i64,
    config: crate::config::QuotaConfig,
    rows: Vec<crate::quota::view::Row>,
}

/// Command-line flag of a window opened with Ctrl+Shift+N.
pub const NEW_WINDOW_ARG: &str = "--new-window";

pub struct AnvilApp {
    config: Config,
    config_mtime: Option<SystemTime>,
    config_checked: Instant,
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
    /// Block rows, rebuilt when the snapshot, the quota settings or the minute
    /// changes (relative times are printed to the minute).
    quota_rows: QuotaRows,
    run_dir: Option<PathBuf>,
    repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    inherited_prompt_command: Option<String>,
    proc_snapshot: Vec<crate::procs::ProcInfo>,
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
        let (keymap, problems) = Keymap::with_overrides(&config.hotkeys);
        let has_problems = !problems.is_empty();
        let palette = scheme_palette(&config);
        theme::set_scheme(&palette);
        let status_dir = status_dir();
        // A window opened with Ctrl+Shift+N: the first window owns the session.
        let extra_window = std::env::args().skip(1).any(|arg| arg == NEW_WINDOW_ARG);
        let mut session = SessionState::load(&SessionState::path());
        if extra_window {
            session = session.for_extra_window();
        }
        let session_window = session.window;
        let mut app = AnvilApp {
            config,
            config_mtime,
            config_checked: Instant::now(),
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
            quota_rows: QuotaRows::default(),
            run_dir: None,
            repaint: None,
            inherited_prompt_command: std::env::var("PROMPT_COMMAND").ok(),
            proc_snapshot: Vec::new(),
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
        if let Some(notice) = notice {
            app.toast(notice);
        }
        for problem in &problems {
            log::warn!("config hotkeys: {problem}");
        }
        if has_problems {
            app.toast(strings::UNKNOWN_HOTKEYS.to_owned());
        }
        app
    }

    pub fn window_attributes(&self) -> WindowAttributes {
        let mut attributes = Window::default_attributes()
            .with_title(strings::APP_TITLE)
            .with_decorations(false)
            .with_visible(false)
            .with_min_inner_size(winit::dpi::LogicalSize::new(640.0, 400.0))
            .with_window_icon(window_icon());
        if let Some(window) = self.session.window {
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
        self.check_config(&ctx);
        self.absorb_wsl_profiles();
        self.poll_panes(&ctx);
        self.poll_statuses();
        self.expire_toasts();
        resize_borders(&ctx, maximized, &mut commands);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::colors().chrome_bg))
            .show_inside(ui, |ui| {
                if self.ui.dialog.is_some() {
                    ui.disable();
                }
                let full = ui.max_rect();
                let title = Rect::from_min_size(full.min, Vec2::new(full.width(), theme::TITLEBAR_HEIGHT));
                title_bar(ui, title, maximized, &mut commands);
                let body = Rect::from_min_max(Pos2::new(full.min.x, title.max.y), full.max);
                let tabbar_rect = Rect::from_min_size(body.min, Vec2::new(theme::TABBAR_WIDTH, body.height()));
                let area = Rect::from_min_max(Pos2::new(body.min.x + theme::TABBAR_WIDTH, body.min.y), body.max);
                ui.painter().vline(tabbar_rect.max.x - 0.5, tabbar_rect.y_range(), egui::Stroke::new(1.0, theme::colors().border));

                let badge = &self.config.claude_status;
                let show_badge = badge.badge && badge.badge_fields.any();
                let infos: Vec<TabInfo> = (0..self.tabs.len())
                    .map(|i| TabInfo {
                        title: self.tabs[i].title(),
                        active: i == self.active && !self.settings_open,
                        activity: self.tabs[i].has_activity && i != self.active,
                        // Hidden badge: no extra row height either.
                        claude: show_badge.then(|| self.tabs[i].claude_status().cloned()).flatten(),
                    })
                    .collect();
                let badge_fields = self.config.claude_status.badge_fields;
                let quota_rows = self.current_quota_rows();
                let quota_block = (!quota_rows.is_empty()).then(|| crate::chrome::quota_block::QuotaBlock {
                    rows: &quota_rows,
                    collapsed: self.config.quota.collapsed,
                });
                let tabbar_actions = tabbar::show(
                    ui, tabbar_rect, &mut self.tabbar, &infos, self.settings_open, &badge_fields, quota_block.as_ref(),
                );
                for action in tabbar_actions {
                    self.apply_tabbar_action(action, &ctx);
                }

                if self.settings_open {
                    self.ime_area = None;
                    self.show_settings(ui, area);
                } else {
                    self.show_active_tab(ui, area, &ctx);
                }
            });

        self.show_toasts(&ctx);
        self.show_picker(&ctx);
        self.show_dialog(&ctx);
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
        let overlay = self.ui.picker.is_some() || self.ui.dialog.is_some();
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
                if index != active && pane.take_output_flag() {
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

    fn show_active_tab(&mut self, ui: &mut egui::Ui, rect: Rect, ctx: &egui::Context) {
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
            self.quota_rows = QuotaRows::default();
            return;
        }
        if self.quota.is_some() {
            return;
        }
        let repaint = self.repaint.clone();
        let config_path = Config::path();
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

    fn current_quota_rows(&mut self) -> Vec<crate::quota::view::Row> {
        let Some(handle) = &self.quota else { return Vec::new() };
        let now = crate::quota::time::now_unix();
        let version = handle.version();
        let cache = &mut self.quota_rows;
        if cache.version != version || cache.minute != now / 60 || cache.config != self.config.quota {
            cache.rows = crate::quota::view::rows(&handle.snapshot(), &self.config.quota, now);
            cache.version = version;
            cache.minute = now / 60;
            cache.config = self.config.quota.clone();
        }
        cache.rows.clone()
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
        // Edit a copy: apply_config must compare the new values against the
        // configuration that is actually in effect, or nothing would ever
        // re-install fonts, switch the palette or rebuild the profile list.
        let mut next = self.config.clone();
        let outcome = {
            let claude_line = self.claude_line.1.clone();
            let mut context = crate::settings_ui::SettingsContext {
                config: &mut next,
                keymap_rows: rows,
                profiles: self.profiles.iter().map(|p| (p.id.clone(), p.name.clone())).collect(),
                fonts: &self.font_families,
                claude_line: &claude_line,
                quota: quota_snapshot.as_ref(),
            };
            crate::settings_ui::show(ui, rect, &mut context, &mut self.settings)
        };
        if outcome.open_config {
            let path = Config::path();
            if let Err(e) = self.config.save(&path) {
                log::warn!("cannot save {}: {e}", path.display());
            }
            self.config_mtime = file_mtime(&path);
            crate::settings_ui::open_path(&path);
        }
        if outcome.changed {
            self.apply_config(ui.ctx().clone(), next, true);
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
            if let Some(quota) = &self.quota {
                quota.refresh();
            }
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
            tabbar::TabbarAction::Move(from, to) => self.move_tab(from, to),
            tabbar::TabbarAction::NewTab => {
                let profile = self.default_profile();
                let cwd = self.focused_cwd();
                self.new_tab_at_end(&profile, cwd);
            }
            tabbar::TabbarAction::Profiles => self.open_picker(ctx),
            tabbar::TabbarAction::Settings => {
                self.settings_open = true;
                ctx.request_repaint();
            }
            tabbar::TabbarAction::QuotaToggle => {
                let mut next = self.config.clone();
                next.quota.collapsed = !next.quota.collapsed;
                self.apply_config(ctx.clone(), next, true);
            }
            tabbar::TabbarAction::QuotaRefresh => {
                if let Some(quota) = &self.quota {
                    quota.refresh();
                }
            }
        }
    }

    fn apply_tab_action(&mut self, action: TabAction, ctx: &egui::Context) {
        match action {
            TabAction::Focus(id) => self.focus_pane(id),
            TabAction::ClosePane(id) => self.close_pane(id),
            TabAction::Split(id, dir) => self.split_pane(id, dir),
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
    /// They cost ~100 MB of working set, so panes start without them.
    fn load_fallbacks(&mut self, ctx: &egui::Context) {
        if self.fallbacks_loaded {
            return;
        }
        self.fallbacks_loaded = true;
        let report = fonts::install(ctx, &self.config.font.family, &self.font_entries, true);
        for missing in &report.missing {
            log::info!("font not found: {missing}");
        }
        log::info!("system fallback fonts installed");
        ctx.request_repaint();
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
        if self.ui.dialog.is_some() { return; }
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
        if self.ui.dialog.is_some() { return; }
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
        if let Some(pane) = self.focused_pane() {
            if pane.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
                pane.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
            }
        }
    }

    pub fn run_action(&mut self, action: &Action, ctx: &egui::Context) -> Vec<WindowCommand> {
        if self.ui.dialog.is_some() { return Vec::new(); }
        let mut commands = Vec::new();
        match action {
            Action::NewTab => {
                let profile = self.default_profile();
                let cwd = self.focused_cwd();
                self.new_tab_at_end(&profile, cwd);
            }
            Action::NewWindow => {
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
                self.tabbar.rename = Some(tabbar::RenameEdit { tab: self.active, text: title, focus: true });
            }
            Action::NextTab => self.cycle_tab(1),
            Action::PreviousTab => self.cycle_tab(-1),
            Action::MoveTabLeft => self.move_tab(self.active, self.active.saturating_sub(1)),
            Action::MoveTabRight => self.move_tab(self.active, (self.active + 1).min(self.tabs.len().saturating_sub(1))),
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
            Action::Settings => self.settings_open = true,
            Action::ToggleFullscreen => commands.push(WindowCommand::ToggleFullscreen),
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
        if let Some(window) = window {
            self.window_geometry(window.inner_size(), window.outer_position().ok(), window.is_maximized());
        }
        self.save_session();
        for tab in &mut self.tabs {
            tab.panes.clear();
        }
        if let Some(dir) = self.run_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
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
        let command = self.config.workspace.ai_commit_command.clone()
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
        let Some(profile) = self.profiles.iter().find(|p| p.id == profile_id).cloned() else { return };
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
        if let Some(id) = tab.tree.navigate(tab.focused, nav, crate::layout::split_tree::Rect::new(area.min.x, area.min.y, area.width(), area.height()), theme::DIVIDER_WIDTH) {
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
        let version = env!("CARGO_PKG_VERSION");
        let env = profiles::pane_env(profile, id, Some(&self.status_dir), version, self.inherited_prompt_command.as_deref());
        let options = SpawnOptions {
            pane_id: id,
            program: profile.command.clone(),
            args: profile.args.clone(),
            // Unknown pane folder: the profile's, else the user profile (11.2).
            cwd: cwd
                .or_else(|| profile.cwd.clone())
                .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from)),
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
        let repaint = self.repaint.clone().unwrap_or_else(|| Arc::new(|| {}));
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
                    .or_else(|| pane_state.profile_id.strip_prefix("wsl-").filter(|distro| !distro.is_empty()).map(profiles::wsl_profile))
                    .unwrap_or_else(|| self.default_profile());
                let cwd = session::usable_cwd(pane_state.cwd.as_deref());
                let mut entry = self.spawn_entry(id, &profile, cwd);
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
                .unwrap_or(PaneState { profile_id: String::new(), cwd: None, workspace_open: false, workspace_width: None, workspace_tab: None })
        };
        let focused = tab.tree.panes().iter().position(|id| *id == tab.focused).unwrap_or(0);
        TabState {
            layout: crate::session::SavedNode::from_node(tab.tree.root(), &describe),
            focused,
            custom_title: tab.custom_title.clone(),
        }
    }

    // ---- config, fonts, session -----------------------------------------

    /// Applies a configuration. `save` is set for edits made in the settings
    /// page (they must persist); external reloads never write the file back.
    fn apply_config(&mut self, ctx: egui::Context, config: Config, save: bool) {
        let family_changed = config.font.family != self.config.font.family;
        let scheme_changed = config.color_scheme != self.config.color_scheme;
        let profiles_changed = config.profiles != self.config.profiles || config.default_profile != self.config.default_profile;
        let claude_disabled = self.config.claude_status.enabled && !config.claude_status.enabled;
        let claude_enabled = !self.config.claude_status.enabled && config.claude_status.enabled;
        let previous_integration = claude_disabled.then(|| self.config.claude_status.clone());
        let quota_switches_changed = config.quota.providers != self.config.quota.providers;
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
            let path = Config::path();
            if let Err(e) = self.config.save(&path) {
                log::warn!("cannot save {}: {e}", path.display());
            } else {
                self.config_mtime = file_mtime(&path);
            }
        }
        let (keymap, problems) = Keymap::with_overrides(&self.config.hotkeys);
        self.keymap = keymap;
        if !problems.is_empty() {
            for problem in &problems {
                log::warn!("config hotkeys: {problem}");
            }
            self.toast(strings::UNKNOWN_HOTKEYS.to_owned());
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
        if self.config_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.config_checked = Instant::now();
        let path = Config::path();
        let mtime = file_mtime(&path);
        if mtime == self.config_mtime {
            return;
        }
        match Config::load_for_reload(&path) {
            Ok(config) => {
                self.config_mtime = mtime;
                self.apply_config(ctx.clone(), config, false);
            }
            Err(e) => {
                // Keep the active configuration and retry: the file may be
                // half-written by an editor right now.
                log::warn!("config.json not reloaded: {e}");
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
        let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else { return };
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
        let (current, keep_previous) = match claude_setup::plan(settings.as_deref(), &ours, self.config.claude_status.declined_command.as_deref()) {
            Plan::Install => (String::new(), false),
            Plan::Update => {
                let current = settings.as_deref().and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
                    .and_then(|value| value.get("statusLine")?.get("command")?.as_str().map(str::to_owned)).unwrap_or_default();
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

    fn install_claude(&mut self, path: &Path, ours: &str, expected: Option<&str>, keep_previous: bool) -> std::io::Result<()> {
        if !Path::new(ours.trim_matches('"')).is_file() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Claude helper was moved or removed before confirmation"));
        }
        let (text, previous) = claude_setup::install(expected, ours).map_err(std::io::Error::other)?;
        claude_setup::write_confirmed(path, expected, &text)?;
        if !keep_previous {
            self.config.claude_status.previous_status_line = previous;
        }
        self.config.claude_status.installed_command = Some(ours.to_owned());
        self.config.claude_status.installed_settings_path = Some(path.to_path_buf());
        self.config.claude_status.declined_command = None;
        let config_path = Config::path();
        self.config.save(&config_path)?;
        self.config_mtime = file_mtime(&config_path);
        Ok(())
    }

    fn disable_claude(&mut self) {
        // Consent belongs to the installed location, not today's executable or
        // CLAUDE_CONFIG_DIR. Moving ANVIL must not prevent restoring the original.
        let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let path = self.config.claude_status.installed_settings_path.clone()
            .or_else(|| claude_setup::settings_path(claude_dir.as_deref(), home.as_deref()));
        let Some(path) = path else { return };
        let Ok(fresh) = std::fs::read_to_string(&path) else { return };
        let ours = self.config.claude_status.installed_command.clone().or_else(|| {
            // Older opted-in configurations did not record their helper path.
            serde_json::from_str::<serde_json::Value>(&fresh).ok()
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
                let config_path = Config::path();
                if let Err(e) = self.config.save(&config_path) {
                    log::warn!("cannot save {}: {e}", config_path.display());
                }
                self.config_mtime = file_mtime(&config_path);
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
        let polled_processes = self.last_proc_poll.elapsed() >= Duration::from_secs(3);
        if polled_processes {
            self.last_proc_poll = Instant::now();
            self.proc_snapshot = crate::procs::snapshot();
            self.ai_commands.clear();
            for tab in &mut self.tabs {
                for (id, entry) in &mut tab.panes {
                    let Some(pane) = entry.live() else { continue };
                    let names = crate::procs::detected_cli_names(&self.proc_snapshot, pane.shell_pid);
                    let has_claude = names.contains("claude");
                    if let Some(command) = ["claude", "opencode", "codex", "gemini", "aider"].into_iter().find(|name| names.contains(*name)) {
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
                if polled_processes && entry.claude.is_some() && !entry.has_claude {
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
        self.ui.picker = Some(PickerState { filter: String::new(), selected: 0, focus: true, opened_pass: ctx.cumulative_pass_nr() });
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
                    let path = Config::path();
                    if let Err(e) = self.config.save(&path) {
                        log::warn!("cannot save {}: {e}", path.display());
                    }
                    self.config_mtime = file_mtime(&path);
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
        "3024 Day" => Palette::day_3024(),
        "Hardcore" => Palette::hardcore(),
        name => config
            .custom_color_schemes
            .iter()
            .find(|scheme| scheme.name == name)
            .and_then(|scheme| {
                let colors: Vec<&str> = scheme.colors.iter().map(String::as_str).collect();
                Palette::from_hex(&scheme.foreground, &scheme.background, &scheme.cursor, &colors).ok()
            })
            .unwrap_or_else(Palette::hardcore),
    }
}

pub fn scheme_names(config: &Config) -> Vec<String> {
    let mut names = vec!["Hardcore".to_owned(), "3024 Day".to_owned()];
    names.extend(config.custom_color_schemes.iter().map(|s| s.name.clone()));
    names
}

fn font_families(entries: &[(String, String)]) -> Vec<String> {
    let mut families: Vec<String> = entries.iter()
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
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("anvil")
        .join("run")
}

/// Creates this process's status directory and removes the directories of
/// ANVIL processes that are gone.
fn prepare_status_dir() -> (PathBuf, Option<PathBuf>) {
    let base = run_base();
    let mine = base.join(std::process::id().to_string());
    if std::fs::create_dir_all(&mine).is_err() {
        return (mine, None);
    }
    let alive: Vec<u32> = crate::procs::snapshot().into_iter().map(|p| p.pid).collect();
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
            if pid != std::process::id() && !alive.contains(&pid) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let run_dir = mine.clone();
    (mine, Some(run_dir))
}
