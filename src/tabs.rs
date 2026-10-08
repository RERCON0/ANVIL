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
    /// The pointer is on a window resize border: the press belongs to the
    /// window, so panes must not start a selection under it.
    pub window_edge: bool,
}

/// Actions a tab asks the application to perform.
#[derive(Debug)]
pub enum TabAction {
    Focus(PaneId),
    ClosePane(PaneId),
    Split(PaneId, Dir),
    ToggleMaximized(PaneId),
    Relocate {
        pane: PaneId,
        target: Option<PaneId>,
        dir: Dir,
        after: bool,
    },
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
    /// The tab's colour from the context menu; None: no mark.
    pub color: Option<theme::TabColor>,
    pub has_activity: bool,
    /// Cursor rectangle of the focused pane, for the IME candidate window.
    pub ime_area: Option<Rect>,
    /// Terminal area of every live pane as last drawn, for `pane_at`.
    pub terminal_rects: Vec<(PaneId, Rect)>,
    /// A label drag changes only the split tree; pane entries keep running.
    dragging_pane: Option<PaneId>,
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
            color: None,
            has_activity: false,
            ime_area: None,
            terminal_rects: Vec::new(),
            dragging_pane: None,
        }
    }

    /// Moves the focus to `id`. The focused pane must be on screen, so a
    /// maximized view of another pane ends.
    pub fn set_focus(&mut self, id: PaneId) {
        self.focused = id;
        if self.maximized.is_some_and(|maximized| maximized != id) {
            self.maximized = None;
        }
    }

    /// Removes pane `id`; true when it was the tab's last pane, so the caller
    /// closes the tab instead. The layout cannot lose its last leaf: when the
    /// pane is the only one left in it while others sit collapsed, a collapsed
    /// one is brought back first, or the tab would keep a leaf without a pane.
    pub fn remove_pane(&mut self, id: PaneId) -> bool {
        if !self.panes.contains_key(&id) {
            return false;
        }
        if self.panes.len() <= 1 {
            return true;
        }
        if self.tree.pane_count() == 1 && self.tree.contains(id) && !self.collapsed.is_empty() {
            let (restored, anchor) = self.collapsed.remove(0);
            self.tree.restore(restored, anchor, id);
        }
        let anchor = self.tree.remove(id);
        self.panes.remove(&id);
        self.collapsed.retain(|(pane, _)| *pane != id);
        if self.focused == id {
            self.focused = anchor
                .map(|a| a.neighbor)
                .filter(|neighbor| self.panes.contains_key(neighbor) && self.tree.contains(*neighbor))
                .or_else(|| self.tree.panes().first().copied())
                .unwrap_or(id);
        }
        if self.maximized == Some(id) {
            self.maximized = None;
        }
        false
    }

    /// The pane whose terminal area contains `pos` (as drawn last frame).
    pub fn pane_at(&self, pos: Pos2) -> Option<PaneId> {
        self.terminal_rects.iter().find(|(_, rect)| rect.contains(pos)).map(|(id, _)| *id)
    }

    /// The title shown in the tab list: custom, else the focused pane's.
    pub fn title(&self) -> &str {
        if let Some(title) = &self.custom_title {
            return title;
        }
        self.panes.get(&self.focused).map(|p| p.title_text()).unwrap_or_default()
    }

    pub fn pane(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(&id)
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
        self.panes.values().filter_map(|p| p.claude.as_ref()).max_by_key(|record| record.updated_at_ms)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, env: &FrameEnv) -> Vec<TabAction> {
        let mut actions = Vec::new();
        let mut layout_rect = rect;
        let focused = self.focused;
        self.terminal_rects.clear();

        if !self.collapsed.is_empty() {
            let strip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), COLLAPSED_STRIP_HEIGHT));
            ui.painter().rect_filled(strip, 0.0, theme::colors().chrome_bg);
            let mut x = strip.min.x + 6.0;
            let mut chips = Vec::new();
            for (id, _) in &self.collapsed {
                let title = self.panes.get(id).map(|p| p.title_text().to_owned()).unwrap_or_default();
                chips.push((*id, title));
            }
            for (id, title) in chips {
                let galley = ui.painter().layout_no_wrap(title, FontId::proportional(12.0), theme::colors().tab_text);
                let chip = Rect::from_min_size(
                    Pos2::new(x, strip.min.y + 3.0),
                    Vec2::new(galley.size().x + 18.0, strip.height() - 6.0),
                );
                let response = ui.interact(chip, ui.id().with(("collapsed", id)), Sense::click());
                let fill =
                    if response.hovered() { theme::colors().tab_active_bg } else { theme::colors().tab_hover_bg };
                ui.painter().rect_filled(chip, 0.0, fill);
                ui.painter().galley(
                    Pos2::new(chip.min.x + 9.0, chip.center().y - galley.size().y / 2.0),
                    galley,
                    theme::colors().tab_text,
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

        let held = ui.is_enabled()
            && self.maximized.is_none()
            && visible.len() > 1
            && ui.input(|i| {
                i.focused && i.modifiers.ctrl && i.modifiers.shift && !i.modifiers.alt && !i.modifiers.mac_cmd
            });
        let was_dragging = self.dragging_pane.is_some();
        if !held
            || self.dragging_pane.is_some_and(|id| !self.tree.contains(id))
            || ui.input(|i| !i.pointer.primary_down() && !i.pointer.primary_released())
        {
            self.dragging_pane = None;
        }
        let label_hovered = held
            && ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|pos| visible.iter().any(|(_, rect)| pane_label_rect(*rect).contains(pos)));
        let block_pointer = was_dragging || label_hovered;
        let builder = if block_pointer { egui::UiBuilder::new().disabled() } else { egui::UiBuilder::new() };
        ui.scope_builder(builder, |ui| {
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
                    window_edge: env.window_edge || block_pointer,
                };
                match &mut entry.content {
                    PaneContent::Live(pane) => {
                        // Only the open panel needs the directory, so the shell's cwd query and
                        // its PathBuf are not paid for a closed panel every frame.
                        let cwd = entry.workspace.open.then(|| pane.current_dir());
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
                        self.terminal_rects.push((*id, terminal_rect));
                        let output = entry.view.show(ui, terminal_rect, pane, &input);
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
                            let color =
                                if response.hovered() { theme::colors().icon_hover } else { theme::colors().icon };
                            if response.hovered() {
                                painter.rect_filled(button, 0.0, theme::colors().tab_hover_bg);
                            }
                            painter.text(button.center(), Align2::CENTER_CENTER, "≡", theme::font(14.0), color);
                            let _ = response.clone().on_hover_text(strings::WORKSPACE_TOGGLE_HINT());
                            if response.clicked() {
                                entry.workspace.open = true;
                                entry.workspace.refresh_soon();
                            }
                        }
                        if let Some(panel_rect) = panel_rect {
                            let cwd = cwd.clone().flatten().or_else(|| entry.start_cwd.clone());
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
                            let response =
                                ui.interact(handle, ui.id().with(("workspace-handle", *id)), Sense::click_and_drag());
                            if response.hovered() || response.dragged() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            }
                            if response.dragged() {
                                entry.workspace.width =
                                    crate::workspace::clamp_width(entry.workspace.width - response.drag_delta().x);
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
                            theme::colors().tab_text,
                        );
                        let button = Rect::from_min_size(
                            Pos2::new(
                                pane_rect.min.x + theme::PANE_PADDING,
                                pane_rect.min.y + theme::PANE_PADDING + 24.0,
                            ),
                            Vec2::new(90.0, 24.0),
                        );
                        let response = ui.interact(button, ui.id().with(("pane-error-close", id)), Sense::click());
                        let fill = if response.hovered() {
                            theme::colors().tab_active_bg
                        } else {
                            theme::colors().tab_hover_bg
                        };
                        painter.rect_filled(button, 0.0, fill);
                        painter.text(
                            button.center(),
                            Align2::CENTER_CENTER,
                            strings::PANE_CLOSE(),
                            theme::font(12.5),
                            theme::colors().tab_active_text,
                        );
                        if response.clicked() {
                            actions.push(TabAction::ClosePane(*id));
                        }
                    }
                }
                // Terminal and its Git panel are one focus surface. Paint once,
                // after both, so neither project looks active in an unfocused pane.
                if focused != *id {
                    ui.painter().rect_filled(*pane_rect, 0.0, theme::colors().pane_dim);
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
                    let color = if response.hovered() || response.dragged() {
                        theme::colors().divider_hover
                    } else {
                        theme::colors().divider
                    };
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
        });
        if held {
            self.show_rearrange_labels(ui, layout_rect, &visible, env.window_edge, &mut actions);
        }
        actions
    }

    fn show_rearrange_labels(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        visible: &[(PaneId, Rect)],
        window_edge: bool,
        actions: &mut Vec<TabAction>,
    ) {
        let c = theme::colors();
        for (id, rect) in visible {
            let label = pane_label_rect(*rect);
            egui::Area::new(ui.id().with(("pane-rearrange-label", id)))
                .order(egui::Order::Foreground)
                .fixed_pos(label.min)
                .constrain(false)
                .movable(false)
                .fade_in(false)
                .show(ui.ctx(), |ui| {
                    let (label, response) = ui.allocate_exact_size(label.size(), Sense::click_and_drag());
                    let dragging = self.dragging_pane == Some(*id);
                    ui.painter().rect_filled(
                        label,
                        0.0,
                        if response.hovered() || dragging { c.field } else { c.chrome_bg },
                    );
                    ui.painter().rect_stroke(label, 0.0, egui::Stroke::new(1.0, c.accent), egui::StrokeKind::Inside);
                    let title = self.panes.get(id).map(PaneEntry::title_text).unwrap_or_default();
                    ui.painter_at(label.shrink(6.0)).text(
                        label.center(),
                        Align2::CENTER_CENTER,
                        title,
                        theme::field_font(12.5),
                        c.text,
                    );
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    if !window_edge && response.drag_started_by(egui::PointerButton::Primary) {
                        self.dragging_pane = Some(*id);
                    }
                    let _ = response.on_hover_text(strings::PANE_REARRANGE_HINT());
                });
        }
        let Some(source) = self.dragging_pane else { return };
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let drop = pointer.and_then(|pos| pane_drop_at(area, visible, source, pos));
        ui.ctx().set_cursor_icon(if drop.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::NotAllowed
        });
        if let Some(drop) = drop {
            let painter =
                ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, ui.id().with("pane-drop-preview")));
            painter.rect_filled(drop.preview, 0.0, c.accent.gamma_multiply(0.18));
            painter.rect_stroke(drop.preview, 0.0, egui::Stroke::new(2.0, c.accent), egui::StrokeKind::Inside);
        }
        if ui.input(|i| i.pointer.primary_released()) {
            if let Some(drop) = drop {
                actions.push(TabAction::Relocate {
                    pane: source,
                    target: drop.target,
                    dir: drop.dir,
                    after: drop.after,
                });
            }
            self.dragging_pane = None;
        }
    }
}

/// The last few non-empty screen lines of a pane, for the
/// collapsed-chip hover and the collapsed-panes list.
pub(crate) fn screen_tail(pane: &Pane) -> String {
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

fn pane_label_rect(rect: Rect) -> Rect {
    Rect::from_center_size(rect.center(), Vec2::new((rect.width() - 16.0).clamp(1.0, 240.0), rect.height().min(32.0)))
}

#[derive(Clone, Copy, Debug)]
struct PaneDrop {
    target: Option<PaneId>,
    dir: Dir,
    after: bool,
    preview: Rect,
}

/// The outer rim targets the whole tab; elsewhere the nearest pane side wins.
fn pane_drop_at(area: Rect, visible: &[(PaneId, Rect)], source: PaneId, pos: Pos2) -> Option<PaneDrop> {
    if !area.contains(pos) {
        return None;
    }
    let edges = [
        (pos.x - area.left(), Dir::Row, false),
        (area.right() - pos.x, Dir::Row, true),
        (pos.y - area.top(), Dir::Column, false),
        (area.bottom() - pos.y, Dir::Column, true),
    ];
    let &(distance, dir, after) = edges.iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
    let rim = (area.width().min(area.height()) * 0.08).min(24.0);
    let (target, rect, dir, after) = if distance <= rim {
        (None, area, dir, after)
    } else {
        let &(target, rect) = visible.iter().find(|(id, rect)| *id != source && rect.contains(pos))?;
        let x = (pos.x - rect.left()) / rect.width();
        let y = (pos.y - rect.top()) / rect.height();
        let sides =
            [(x, Dir::Row, false), (1.0 - x, Dir::Row, true), (y, Dir::Column, false), (1.0 - y, Dir::Column, true)];
        let &(_, dir, after) = sides.iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
        (Some(target), rect, dir, after)
    };
    let mut preview = rect;
    match (dir, after) {
        (Dir::Row, false) => preview.max.x = rect.center().x,
        (Dir::Row, true) => preview.min.x = rect.center().x,
        (Dir::Column, false) => preview.max.y = rect.center().y,
        (Dir::Column, true) => preview.min.y = rect.center().y,
    }
    Some(PaneDrop { target, dir, after, preview })
}

fn tree_rect(rect: Rect) -> crate::layout::split_tree::Rect {
    crate::layout::split_tree::Rect::new(rect.min.x, rect.min.y, rect.width(), rect.height())
}

fn egui_rect(rect: crate::layout::split_tree::Rect) -> Rect {
    Rect::from_min_size(Pos2::new(rect.x, rect.y), Vec2::new(rect.w, rect.h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> PaneEntry {
        PaneEntry {
            content: PaneContent::Error(String::new()),
            view: TerminalView::new(14.0),
            workspace: crate::workspace::Workspace::default(),
            start_cwd: None,
            profile_id: String::new(),
            profile_name: String::new(),
            title: String::new(),
            exited: false,
            claude: None,
            claude_mtime: None,
            has_claude: false,
        }
    }

    fn two_panes() -> Tab {
        let mut tree = SplitTree::new(1);
        assert!(tree.insert(1, 2, Dir::Row, true));
        Tab::new(tree, HashMap::from([(1, entry()), (2, entry())]), 1)
    }

    /// Uneven panes, so a restore that quietly redistributes the
    /// width shows up as a changed root.
    fn three_panes() -> Tab {
        use crate::layout::split_tree::Node;
        let tree = SplitTree::from_root(Node::Split {
            dir: Dir::Row,
            children: vec![(0.5, Node::Leaf(1)), (0.25, Node::Leaf(2)), (0.25, Node::Leaf(3))],
        });
        Tab::new(tree, HashMap::from([(1, entry()), (2, entry()), (3, entry())]), 1)
    }

    #[test]
    fn drop_sides_distinguish_a_nested_pane_from_the_whole_tab() {
        let area = Rect::from_min_size(Pos2::new(100.0, 40.0), Vec2::new(1000.0, 600.0));
        let target = Rect::from_min_size(Pos2::new(600.0, 40.0), Vec2::new(500.0, 600.0));
        let visible = [(1, Rect::from_min_max(area.min, Pos2::new(595.0, 640.0))), (2, target)];
        for (pos, dir, after) in [
            (Pos2::new(620.0, 340.0), Dir::Row, false),
            (Pos2::new(1060.0, 340.0), Dir::Row, true),
            (Pos2::new(850.0, 80.0), Dir::Column, false),
            (Pos2::new(850.0, 600.0), Dir::Column, true),
        ] {
            let drop = pane_drop_at(area, &visible, 1, pos).unwrap();
            assert_eq!((drop.target, drop.dir, drop.after), (Some(2), dir, after));
            assert!(target.contains_rect(drop.preview));
            assert_eq!(drop.preview.area(), target.area() / 2.0);
        }
        for (pos, dir, after) in [
            (Pos2::new(110.0, 340.0), Dir::Row, false),
            (Pos2::new(1090.0, 340.0), Dir::Row, true),
            (Pos2::new(850.0, 50.0), Dir::Column, false),
            (Pos2::new(850.0, 630.0), Dir::Column, true),
        ] {
            let drop = pane_drop_at(area, &visible, 1, pos).unwrap();
            assert_eq!((drop.target, drop.dir, drop.after), (None, dir, after));
            assert_eq!(drop.preview.area(), area.area() / 2.0);
        }
        for pos in [Pos2::new(350.0, 340.0), Pos2::new(598.0, 340.0), Pos2::new(99.0, 340.0)] {
            assert!(
                pane_drop_at(area, &visible, 1, pos).is_none(),
                "self, divider and outside drops must not move a pane"
            );
        }
    }

    #[test]
    fn changing_focus_dims_the_whole_other_pane_including_its_workspace_area() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Test", &[], false);
        let mut tab = two_panes();
        let palette = Palette::dark();
        let env = FrameEnv {
            palette: &palette,
            active: true,
            cursor_blink: false,
            right_click: RightClick::Menu,
            paste_on_middle: false,
            copy_on_select: false,
            min_pane_width: 80.0,
            min_pane_height: 60.0,
            fallbacks_loaded: true,
            ai_command: None,
            window_edge: false,
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 600.0));
        let panes = tab.tree.layout(tree_rect(rect), theme::DIVIDER_WIDTH);
        for focus in [1, 2, 1] {
            tab.set_focus(focus);
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
                tab.show(ui, rect, &env);
            });
            let dimmed: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(shape) if shape.fill == theme::colors().pane_dim => Some(shape.rect),
                    _ => None,
                })
                .collect();
            let expected = egui_rect(panes.iter().find(|(id, _)| *id != focus).unwrap().1);
            assert_eq!(dimmed, [expected], "the terminal and right-hand workspace share one overlay");
        }
    }

    #[test]
    fn removing_a_pane_keeps_the_tab_consistent() {
        let mut tab = two_panes();
        assert!(!tab.remove_pane(1), "one pane is left");
        assert_eq!(tab.tree.panes(), vec![2]);
        assert_eq!(tab.focused, 2);
        assert!(tab.remove_pane(2), "the last pane closes the tab");
        assert!(!tab.remove_pane(99), "an unknown pane changes nothing");
    }

    /// Splitting or moving between panes while one is maximized put the focus
    /// on a pane that was not drawn: typing went to an invisible terminal.
    #[test]
    fn focusing_another_pane_ends_the_maximized_view() {
        let mut tab = two_panes();
        tab.maximized = Some(1);
        tab.set_focus(1);
        assert_eq!(tab.maximized, Some(1), "focusing the maximized pane keeps it maximized");
        tab.set_focus(2);
        assert_eq!((tab.focused, tab.maximized), (2, None));
    }

    /// The end-to-end shape of Ctrl+Alt+C / Ctrl+Alt+R on the leftmost
    /// pane of three: every cycle must land on the same layout. The
    /// fractions used to be dropped on restore, so the right-hand panes
    /// grew on each pass and the collapsed one shrank.
    #[test]
    fn collapse_and_restore_cycles_leave_the_layout_untouched() {
        let mut tab = three_panes();
        let before = tab.tree.root().clone();
        for _ in 0..5 {
            let anchor = tab.tree.remove(1).expect("pane 1 is not the last");
            tab.collapsed.push((1, anchor));
            tab.focused = tab.tree.panes().first().copied().unwrap_or(1);
            let (_, anchor) = tab.collapsed.pop().expect("the pane is there");
            assert!(tab.tree.restore(1, anchor, 1));
            tab.set_focus(1);
            assert_eq!(tab.tree.root(), &before, "a cycle changed the layout");
        }
    }

    /// Closing the only visible pane while another one sat collapsed left a
    /// leaf without a pane: the layout cannot lose its last leaf, but the
    /// pane was dropped anyway, so the tab showed nothing and typing went
    /// nowhere. The collapsed pane comes back instead.
    #[test]
    fn closing_the_last_visible_pane_brings_a_collapsed_one_back() {
        let mut tab = two_panes();
        let anchor = tab.tree.remove(2).expect("pane 2 collapses");
        tab.collapsed.push((2, anchor));
        assert!(!tab.remove_pane(1), "pane 2 still lives");
        assert_eq!(tab.tree.panes(), vec![2], "the collapsed pane is back in the layout");
        assert!(tab.collapsed.is_empty());
        assert_eq!(tab.focused, 2);
        assert!(tab.panes.contains_key(&2) && !tab.panes.contains_key(&1));
    }
}
