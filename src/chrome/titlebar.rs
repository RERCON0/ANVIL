//! Title bar of the borderless window and its invisible resize borders.

use egui::{Align2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Vec2};

use crate::chrome::edges::{edge_at, Edge};
use crate::host::WindowCommand;
use crate::strings;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Button {
    Minimize,
    Maximize,
    Close,
}

pub fn title_bar(ui: &mut egui::Ui, rect: Rect, maximized: bool, commands: &mut Vec<WindowCommand>) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CHROME_BG);
    let buttons_width = theme::WINDOW_BUTTON_WIDTH * 3.0;
    let drag_rect = Rect::from_min_max(rect.min, Pos2::new(rect.max.x - buttons_width, rect.max.y));
    let drag = ui.interact(drag_rect, ui.id().with("titlebar-drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        commands.push(WindowCommand::ToggleMaximize);
    } else if drag.drag_started_by(egui::PointerButton::Primary) {
        commands.push(WindowCommand::Drag);
    }
    painter.text(
        Pos2::new(rect.min.x + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        strings::APP_TITLE,
        theme::title_font(12.5),
        theme::TITLE_TEXT,
    );
    painter.hline(rect.x_range(), rect.max.y - 0.5, Stroke::new(1.0, theme::BORDER));

    let mut x = rect.max.x - buttons_width;
    for kind in [Button::Minimize, Button::Maximize, Button::Close] {
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(theme::WINDOW_BUTTON_WIDTH, rect.height()));
        x += theme::WINDOW_BUTTON_WIDTH;
        let response = ui.interact(r, ui.id().with(("window-button", kind)), Sense::click());
        let hovered = response.hovered();
        if hovered {
            painter.rect_filled(r, 0.0, if kind == Button::Close { theme::CLOSE_HOVER } else { theme::WINDOW_BUTTON_HOVER });
        }
        let color = if hovered && kind == Button::Close { Color32::WHITE } else { theme::WINDOW_ICON };
        // Plain glyphs, as in the owner's other apps: they need no icon font.
        let (label, tooltip) = match kind {
            Button::Minimize => ("─", strings::WINDOW_MINIMIZE),
            Button::Maximize if maximized => ("❐", strings::WINDOW_MAXIMIZE),
            Button::Maximize => ("□", strings::WINDOW_MAXIMIZE),
            Button::Close => ("×", strings::WINDOW_CLOSE),
        };
        painter.text(r.center(), Align2::CENTER_CENTER, label, theme::font(14.0), color);
        let _ = response.clone().on_hover_text(tooltip);
        if response.clicked() {
            commands.push(match kind {
                Button::Minimize => WindowCommand::Minimize,
                Button::Maximize => WindowCommand::ToggleMaximize,
                Button::Close => WindowCommand::Close,
            });
        }
    }
}

/// Resize cursors and drags on the window edges. Returns true while the
/// pointer is on an edge (the caller should not start a selection then).
pub fn resize_borders(ctx: &egui::Context, maximized: bool, commands: &mut Vec<WindowCommand>) -> bool {
    if maximized {
        return false;
    }
    let screen = ctx.screen_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return false };
    let local = (pos.x - screen.min.x, pos.y - screen.min.y);
    let Some(edge) = edge_at(local, (screen.width(), screen.height()), theme::RESIZE_BORDER) else { return false };
    ctx.set_cursor_icon(match edge {
        Edge::North | Edge::South => CursorIcon::ResizeVertical,
        Edge::East | Edge::West => CursorIcon::ResizeHorizontal,
        Edge::NorthWest | Edge::SouthEast => CursorIcon::ResizeNwSe,
        Edge::NorthEast | Edge::SouthWest => CursorIcon::ResizeNeSw,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        commands.push(WindowCommand::Resize(edge));
    }
    true
}
