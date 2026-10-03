//! One tab: the split layout, its panes and the drawing/interaction code.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Align2, FontId, Pos2, Rect, Sense, Vec2};

use crate::claude_status::StatusRecord;
use crate::config::RightClick;
use crate::layout::split_tree::{Anchor, Dir, PaneId, SplitTree};
use crate::strings;
use crate::term::pane::Pane;
use crate::term::style::Palette;
use crate::term::view::{PaneCommand, TerminalView, ViewInput};
use crate::theme;

const COLLAPSED_STRIP_HEIGHT: f32 = 24.0;

pub struct PaneEntry {
    pub content: PaneContent,
    pub view: TerminalView,
    pub workspace: crate::workspace::Workspace,
    /// Folder the pane was started in; the workspace panel uses it until the
    /// shell reports a live directory over OSC.
    pub start_cwd: Option<std::path::PathBuf>,
    pub profile_id: String,
    pub profile_name: String,
    pub title: String,
    /// The process exited and asked for a key press before closing.
    pub exited: bool,
    pub claude: Option<StatusRecord>,
    pub claude_mtime: Option<SystemTime>,
    pub has_claude: bool,
}

pub enum PaneContent {
    Live(Pane),
    /// The process could not be started; the pane shows the message.
    Error(String),
}

impl PaneEntry {
    pub fn live(&self) -> Option<&Pane> {
        match &self.content {
            PaneContent::Live(pane) => Some(pane),
            PaneContent::Error(_) => None,
        }
    }
    pub fn live_mut(&mut self) -> Option<&mut Pane> {
        match &mut self.content {
            PaneContent::Live(pane) => Some(pane),
            PaneContent::Error(_) => None,
        }
    }
    pub fn title_text(&self) -> &str {
        if self.title.is_empty() {
            &self.profile_name
        } else {
            &self.title
        }
    }
    pub fn cwd(&self) -> Option<PathBuf> {
        self.live().and_then(Pane::current_dir)
    }
}

/// Everything the terminal widget needs that lives outside the tab.
pub struct FrameEnv<'a> {
    pub palette: &'a Palette,
    pub active: bool,
    pub cursor_blink: bool,
    pub right_click: RightClick,
    pub paste_on_middle: bool,
    pub copy_on_select: bool,
    pub min_pane_width: f32,
    pub min_pane_height: f32,
    pub fallbacks_loaded: bool,
    /// CLI for the AI commit message (config or auto-detected), if any.
    pub ai_command: Option<String>,
}

/// Actions a tab asks the application to perform.
#[derive(Debug)]
pub enum TabAction {
    Focus(PaneId),
    ClosePane(PaneId),
    Split(PaneId, Dir),
    ToggleMaximized(PaneId),
    Collapse(PaneId),
    RestoreCollapsed(PaneId),
    Clipboard(String),
    Pane(PaneId, PaneCommand),
    Bell,
    /// A glyph outside the installed fonts appeared: load the system fallbacks.
    NeedsFallbacks,
}

pub struct Tab {
    pub tree: SplitTree,
    pub panes: HashMap<PaneId, PaneEntry>,
    pub focused: PaneId,
    pub maximized: Option<PaneId>,
    /// Collapsed panes with the place they were removed from.
    pub collapsed: Vec<(PaneId, Anchor)>,
    pub custom_title: Option<String>,
    pub has_activity: bool,
    /// Cursor rectangle of the focused pane, for the IME candidate window.
    pub ime_area: Option<Rect>,
}

impl Tab {
    /// The focused pane's workspace panel (open/closed).
    pub fn workspace_mut(&mut self) -> Option<&mut crate::workspace::Workspace> {
        self.panes.get_mut(&self.focused).map(|entry| &mut entry.workspace)
    }

    pub fn new(tree: SplitTree, panes: HashMap<PaneId, PaneEntry>, focused: PaneId) -> Tab {
        Tab {
            tree,
            panes,
            focused,
            maximized: None,
            collapsed: Vec::new(),
            custom_title: None,
            has_activity: false,
            ime_area: None,
        }
    }

    /// The title shown in the tab list: custom, else the focused pane's.
    pub fn title(&self) -> String {
        if let Some(title) = &self.custom_title {
            return title.clone();
        }
        self.panes.get(&self.focused).map(|p| p.title_text().to_owned()).unwrap_or_default()
    }

    pub fn pane(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(&id)
    }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut PaneEntry> {
        self.panes.get_mut(&id)
    }

    pub fn focused_entry(&self) -> Option<&PaneEntry> {
        self.panes.get(&self.focused)
    }

    pub fn focused_entry_mut(&mut self) -> Option<&mut PaneEntry> {
        self.panes.get_mut(&self.focused)
    }

    /// The focused pane waits for a key press after a failed exit.
    pub fn focused_exited(&self) -> bool {
        self.panes.get(&self.focused).is_some_and(|p| p.exited)
    }

    /// The Claude badge on the tab: the focused pane's status, else the newest.
    pub fn claude_status(&self) -> Option<&StatusRecord> {
        let focused = self.panes.get(&self.focused)?;
        if focused.claude.is_some() {
            return focused.claude.as_ref();
        }
        self.panes
            .values()
            .filter_map(|p| p.claude.as_ref())
            .max_by_key(|record| record.updated_at_ms)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, env: &FrameEnv) -> Vec<TabAction> {
        let mut actions = Vec::new();
        let mut layout_rect = rect;
        let focused = self.focused;

        if !self.collapsed.is_empty() {
            let strip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), COLLAPSED_STRIP_HEIGHT));
            ui.painter().rect_filled(strip, 0.0, theme::CHROME_BG);
            let mut x = strip.min.x + 6.0;
            let mut chips = Vec::new();
            for (id, _) in &self.collapsed {
                let title = self.panes.get(id).map(|p| p.title_text().to_owned()).unwrap_or_default();
                chips.push((*id, title));
            }
            for (id, title) in chips {
                let galley = ui.painter().layout_no_wrap(title, FontId::proportional(12.0), theme::TAB_TEXT);
                let chip = Rect::from_min_size(
                    Pos2::new(x, strip.min.y + 3.0),
                    Vec2::new(galley.size().x + 18.0, strip.height() - 6.0),
                );
                let response = ui.interact(chip, ui.id().with(("collapsed", id)), Sense::click());
                let fill = if response.hovered() { theme::TAB_ACTIVE_BG } else { theme::TAB_HOVER_BG };
                ui.painter().rect_filled(chip, 0.0, fill);
                ui.painter().galley(
                    Pos2::new(chip.min.x + 9.0, chip.center().y - galley.size().y / 2.0),
                    galley,
                    theme::TAB_TEXT,
                );
                if response.hovered() {
                    if let Some(preview) = self.panes.get(&id).and_then(PaneEntry::live).map(screen_tail) {
                        if !preview.trim().is_empty() {
                            let _ = response.clone().on_hover_text(preview);
                        }
                    }
                }
                if response.clicked() {
                    actions.push(TabAction::RestoreCollapsed(id));
                }
                x = chip.max.x + 6.0;
            }
            layout_rect = Rect::from_min_max(Pos2::new(rect.min.x, strip.max.y), rect.max);
        }

        let visible: Vec<(PaneId, Rect)> = match self.maximized {
            Some(id) if self.panes.contains_key(&id) => vec![(id, layout_rect)],
            _ => self
                .tree
                .layout(tree_rect(layout_rect), theme::DIVIDER_WIDTH)
                .into_iter()
                .map(|(id, rect)| (id, egui_rect(rect)))
                .collect(),
        };

        for (id, pane_rect) in &visible {
            let Some(entry) = self.panes.get_mut(id) else { continue };
            let input = ViewInput {
                palette: env.palette,
                focused: focused == *id,
                cursor_blink: env.cursor_blink,
                right_click: env.right_click,
                paste_on_middle: env.paste_on_middle,
                copy_on_select: env.copy_on_select,
                fallbacks_loaded: env.fallbacks_loaded,
            };
            match &mut entry.content {
                PaneContent::Live(pane) => {
                    let cwd = pane.current_dir();
                    let (terminal_rect, panel_rect) = if entry.workspace.open {
                        let width = crate::workspace::clamp_width(entry.workspace.width)
                            .min((pane_rect.width() - 140.0).max(crate::workspace::MIN_WIDTH));
                        let split = pane_rect.max.x - width;
                        (
                            Rect::from_min_max(pane_rect.min, Pos2::new(split, pane_rect.max.y)),
                            Some(Rect::from_min_max(Pos2::new(split, pane_rect.min.y), pane_rect.max)),
                        )
                    } else {
                        (*pane_rect, None)
                    };
                    let output = entry.view.show(ui, terminal_rect, pane, &input);
                    // The pane that is not focused is dimmed so the eye lands on
                    // the one being typed into.
                    if focused != *id {
                        ui.painter().rect_filled(terminal_rect, 0.0, theme::PANE_DIM);
                    }
                    if focused == *id {
                        self.ime_area = output.cursor_rect;
                    }
                    if output.pressed {
                        actions.push(TabAction::Focus(*id));
                    }
                    for command in output.commands {
                        actions.push(TabAction::Pane(*id, command));
                    }
                    if output.needs_fallbacks {
                        actions.push(TabAction::NeedsFallbacks);
                    }
                    // Pane events and the output flag are pumped for every pane
                    // by AnvilApp::poll_panes, not only for the visible tab.
                    // Per-pane panel toggle, revealed while hovering the pane.
                    let hovered = ui.input(|i| i.pointer.hover_pos()).is_some_and(|pos| pane_rect.contains(pos));
                    if hovered && !entry.workspace.open {
                        let button = Rect::from_min_size(
                            Pos2::new(pane_rect.right() - 26.0, pane_rect.top() + 4.0),
                            Vec2::splat(22.0),
                        );
                        let response = ui.interact(button, ui.id().with(("workspace-toggle", *id)), Sense::click());
                        let painter = ui.painter_at(button);
                        let color = if response.hovered() { theme::ICON_HOVER } else { theme::ICON };
                        if response.hovered() {
                            painter.rect_filled(button, 0.0, theme::TAB_HOVER_BG);
                        }
                        painter.text(button.center(), Align2::CENTER_CENTER, "≡", theme::font(14.0), color);
                        let _ = response.clone().on_hover_text(strings::WORKSPACE_TOGGLE_HINT);
                        if response.clicked() {
                            entry.workspace.open = true;
                            entry.workspace.refresh_soon();
                        }
                    }
                    if let Some(panel_rect) = panel_rect {
                        let cwd = cwd.clone().or_else(|| entry.start_cwd.clone());
                        if let Some(cwd) = cwd {
                            entry.workspace.poll(cwd);
                            if entry.workspace.absorb() {
                                ui.ctx().request_repaint();
                            }
                        }
                        let actions = entry.workspace.show(ui, panel_rect, *id, env.ai_command.as_deref());
                        for action in actions {
                            match action {
                                crate::workspace::WorkspaceAction::Close => entry.workspace.open = false,
                            }
                        }
                        let handle = Rect::from_min_size(
                            Pos2::new(panel_rect.min.x - 3.0, panel_rect.min.y),
                            Vec2::new(6.0, panel_rect.height()),
                        );
                        let response = ui.interact(handle, ui.id().with(("workspace-handle", *id)), Sense::click_and_drag());
                        if response.hovered() || response.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        if response.dragged() {
                            entry.workspace.width = crate::workspace::clamp_width(entry.workspace.width - response.drag_delta().x);
                        }
                    }
                }
                PaneContent::Error(message) => {
                    let painter = ui.painter_at(*pane_rect);
                    painter.rect_filled(*pane_rect, 0.0, env.palette.background);
                    painter.text(
                        Pos2::new(pane_rect.min.x + theme::PANE_PADDING, pane_rect.min.y + theme::PANE_PADDING),
                        Align2::LEFT_TOP,
                        &*message,
                        theme::font(12.5),
                        theme::TAB_TEXT,
                    );
                    let button = Rect::from_min_size(
                        Pos2::new(pane_rect.min.x + theme::PANE_PADDING, pane_rect.min.y + theme::PANE_PADDING + 24.0),
                        Vec2::new(90.0, 24.0),
                    );
                    let response = ui.interact(button, ui.id().with(("pane-error-close", id)), Sense::click());
                    let fill = if response.hovered() { theme::TAB_ACTIVE_BG } else { theme::TAB_HOVER_BG };
                    painter.rect_filled(button, 0.0, fill);
                    painter.text(button.center(), Align2::CENTER_CENTER, strings::PANE_CLOSE, theme::font(12.5), theme::TAB_ACTIVE_TEXT);
                    if response.clicked() {
                        actions.push(TabAction::ClosePane(*id));
                    }
                }
            }
        }

        if self.maximized.is_none() {
            for divider in self.tree.dividers(tree_rect(layout_rect), theme::DIVIDER_WIDTH) {
                let divider_rect = egui_rect(divider.rect);
                let response = ui.interact(
                    divider_rect,
                    ui.id().with(("divider", divider.path.clone(), divider.index)),
                    Sense::click_and_drag(),
                );
                let color = if response.hovered() || response.dragged() { theme::DIVIDER_HOVER } else { theme::DIVIDER };
                ui.painter().rect_filled(divider_rect, 0.0, color);
                if response.hovered() {
                    ui.ctx().set_cursor_icon(match divider.dir {
                        Dir::Row => egui::CursorIcon::ResizeHorizontal,
                        Dir::Column => egui::CursorIcon::ResizeVertical,
                    });
                }
                if response.dragged() {
                    let delta = match divider.dir {
                        Dir::Row => response.drag_delta().x,
                        Dir::Column => response.drag_delta().y,
                    };
                    let min = match divider.dir {
                        Dir::Row => env.min_pane_width,
                        Dir::Column => env.min_pane_height,
                    };
                    self.tree.drag_divider(tree_rect(layout_rect), theme::DIVIDER_WIDTH, &divider, delta, min);
                }
            }
        }
        actions
    }
}

/// The last few non-empty screen lines of a pane, for the collapsed-chip hover.
fn screen_tail(pane: &Pane) -> String {
    const LINES: usize = 14;
    let term = pane.term.lock();
    let mut lines: Vec<String> = Vec::new();
    let mut current = None;
    for cell in term.renderable_content().display_iter {
        if current != Some(cell.point.line) {
            lines.push(String::new());
            current = Some(cell.point.line);
        }
        if let Some(line) = lines.last_mut() {
            line.push(cell.c);
        }
    }
    for line in &mut lines {
        *line = line.trim_end().to_owned();
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(LINES);
    lines[start..].join("\n")
}

fn tree_rect(rect: Rect) -> crate::layout::split_tree::Rect {
    crate::layout::split_tree::Rect::new(rect.min.x, rect.min.y, rect.width(), rect.height())
}

fn egui_rect(rect: crate::layout::split_tree::Rect) -> Rect {
    Rect::from_min_size(Pos2::new(rect.x, rect.y), Vec2::new(rect.w, rect.h))
}
