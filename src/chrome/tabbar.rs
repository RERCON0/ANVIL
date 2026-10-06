//! Vertical tab list on the left: numbers, titles, the Claude badge, activity
//! dots, drag & drop, rename in place and the buttons under the list.

use egui::{Align2, Color32, FontId, Pos2, Rect, ScrollArea, Sense, Stroke, Vec2};

use crate::claude_status::{clamp_pct, js_round, StatusRecord};
use crate::strings;
use crate::theme;

#[derive(Default)]
pub struct TabbarState {
    pub rename: Option<RenameEdit>,
    drag_from: Option<usize>,
    hover_index: Option<usize>,
}

impl TabbarState {
    fn remap(&mut self, map: impl Fn(usize) -> usize) {
        if let Some(rename) = &mut self.rename {
            rename.tab = map(rename.tab);
        }
        self.drag_from = self.drag_from.map(&map);
        self.hover_index = self.hover_index.map(map);
    }

    pub fn tab_inserted(&mut self, index: usize) {
        self.remap(|slot| if slot >= index { slot + 1 } else { slot });
    }

    pub fn tab_moved(&mut self, from: usize, to: usize) {
        self.remap(|slot| {
            if slot == from {
                to
            } else if from < slot && slot <= to {
                slot - 1
            } else if to <= slot && slot < from {
                slot + 1
            } else {
                slot
            }
        });
    }

    /// Tab `index` was closed: a rename or drag in progress follows its tab
    /// (indices after it shift down) or ends when its own tab is gone.
    pub fn tab_removed(&mut self, index: usize) {
        let shift = |slot: usize| match slot.cmp(&index) {
            std::cmp::Ordering::Less => Some(slot),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(slot - 1),
        };
        if let Some(rename) = self.rename.as_mut() {
            match shift(rename.tab) {
                Some(tab) => rename.tab = tab,
                None => self.rename = None,
            }
        }
        match self.drag_from.and_then(shift) {
            Some(from) => {
                self.drag_from = Some(from);
                self.hover_index = self.hover_index.and_then(shift);
            }
            None => {
                self.drag_from = None;
                self.hover_index = None;
            }
        }
    }
}

pub struct RenameEdit {
    pub tab: usize,
    pub text: String,
    pub focus: bool,
}

pub struct TabInfo<'a> {
    pub title: std::borrow::Cow<'a, str>,
    pub active: bool,
    pub activity: bool,
    pub claude: Option<&'a StatusRecord>,
    /// The tab's colour mark, painted as a bar on the row's leading edge.
    pub color: Option<theme::TabColor>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TabbarAction {
    Select(usize),
    Close(usize),
    Duplicate(usize),
    CloseOthers(usize),
    Rename(usize, String),
    SetColor(usize, Option<theme::TabColor>),
    Move(usize, usize),
    NewTab,
    Profiles,
    CollapsedList,
    Settings,
}

pub fn show(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &mut TabbarState,
    tabs: &[TabInfo],
    settings_open: bool,
    badge_fields: &ClaudeBadgeFields,
    collapsed: usize,
) -> Vec<TabbarAction> {
    let mut actions = Vec::new();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::colors().chrome_bg);
    state.hover_index = None;

    // More tabs than fit must stay reachable: the list scrolls, so a tab past
    // the fold can be clicked, renamed, dragged and closed like any other.
    let content_height: f32 = tabs.iter().map(row_height).sum();
    // Reserve separate rows for the action buttons and bottom settings link.
    // A short list ends at its last tab, not at the bottom of the sidebar.
    let list_height = content_height.min((rect.height() - 70.0).max(0.0));
    let list = Rect::from_min_size(rect.min, Vec2::new(rect.width(), list_height));
    let mut target = None;
    // The title bar only paints; it does not advance the parent's cursor.
    ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical().id_salt("tabbar-tabs").auto_shrink([false, false]).max_height(list.height()).show(
            ui,
            |ui| {
                ui.set_height(content_height);
                let painter = ui.painter().clone();
                let pointer = ui.input(|i| i.pointer.interact_pos()).filter(|pos| list.contains(*pos));
                for (index, tab) in tabs.iter().enumerate() {
                    let height = row_height(tab);
                    let top = ui.next_widget_position().y;
                    let row = Rect::from_min_size(Pos2::new(list.min.x, top), Vec2::new(list.width(), height));
                    if pointer.is_some_and(|pos| row.contains(pos)) {
                        target = Some(index);
                    }
                    let response = ui.interact(row, ui.id().with(("tab", index)), Sense::click_and_drag());
                    tab_row(ui, &painter, state, index, tab, row, response, badge_fields, &mut actions);
                    ui.allocate_space(Vec2::new(0.0, height));
                }
            },
        );
    });
    // Buttons follow the visible list; scrolling never hides them.
    let y = list.max.y;

    // Drag & drop reordering.
    if let Some(from) = state.drag_from {
        // Use the actual row rects, including their scroll translation, even
        // on the release frame (hovered() does not identify drop targets).
        state.hover_index = target;
        if ui.input(|i| i.pointer.any_released()) {
            if let Some(to) = state.hover_index {
                if from != to {
                    actions.push(TabbarAction::Move(from, to));
                }
            }
            state.drag_from = None;
        }
    }

    // Buttons under the list.
    let plus_rect = Rect::from_min_size(Pos2::new(rect.min.x + 8.0, y + 8.0), Vec2::new(28.0, 24.0));
    let plus = ui.interact(plus_rect, ui.id().with("tab-new"), Sense::click());
    let plus_color = if plus.hovered() { theme::colors().icon_hover } else { theme::colors().icon };
    painter.text(plus_rect.center(), Align2::CENTER_CENTER, "+", theme::font(17.0), plus_color);
    if plus.on_hover_text(strings::TAB_NEW).clicked() {
        actions.push(TabbarAction::NewTab);
    }
    let profile_rect = Rect::from_min_size(Pos2::new(rect.min.x + 44.0, y + 8.0), Vec2::new(28.0, 24.0));
    let profile = ui.interact(profile_rect, ui.id().with("tab-profiles"), Sense::click());
    let profile_color = if profile.hovered() { theme::colors().icon_hover } else { theme::colors().icon };
    painter.text(profile_rect.center(), Align2::CENTER_CENTER, "»", theme::font(14.0), profile_color);
    if profile.on_hover_text(strings::TAB_PROFILES).clicked() {
        actions.push(TabbarAction::Profiles);
    }

    // The list of panes hidden by Ctrl+Alt+C. The glyph is
    // drawn with strokes: the icon font has no symbol for
    // "hidden panes", and a text label would not fit.
    let collapsed_rect = Rect::from_min_size(Pos2::new(rect.min.x + 80.0, y + 8.0), Vec2::new(28.0, 24.0));
    let collapsed_button = ui.interact(collapsed_rect, ui.id().with("tab-collapsed"), Sense::click());
    let collapsed_color = if collapsed_button.hovered() { theme::colors().icon_hover } else { theme::colors().icon };
    let center = collapsed_rect.center();
    let stroke = Stroke::new(1.5, collapsed_color);
    for dy in [-4.0, 0.0, 4.0] {
        painter
            .line_segment([Pos2::new(center.x - 5.0, center.y + dy), Pos2::new(center.x + 5.0, center.y + dy)], stroke);
    }
    if collapsed > 0 {
        let count = painter.layout_no_wrap(collapsed.to_string(), theme::font(11.0), collapsed_color);
        painter.galley(Pos2::new(collapsed_rect.max.x + 4.0, center.y - count.size().y / 2.0), count, collapsed_color);
    }
    if collapsed_button.on_hover_text(strings::TAB_COLLAPSED).clicked() {
        actions.push(TabbarAction::CollapsedList);
    }

    let settings_rect =
        Rect::from_min_size(Pos2::new(rect.min.x + 8.0, rect.max.y - 30.0), Vec2::new(rect.width() - 16.0, 24.0));
    let settings = ui.interact(settings_rect, ui.id().with("tab-settings"), Sense::click());
    if settings_open {
        painter.rect_filled(settings_rect, 0.0, theme::colors().tab_active_bg);
    }
    let settings_color =
        if settings_open || settings.hovered() { theme::colors().icon_hover } else { theme::colors().icon };
    let label = painter.layout_no_wrap(strings::TAB_SETTINGS.to_owned(), theme::font(12.5), settings_color);
    let label_pos = Pos2::new(settings_rect.min.x + 24.0, settings_rect.center().y - label.size().y / 2.0);
    // Align to the visible letters, not the font's ascent/descent box: the
    // chevron glyph's optical center was lower than the Cyrillic label.
    let center = Pos2::new(settings_rect.min.x + 12.0, label_pos.y + label.mesh_bounds.center().y);
    let stroke = Stroke::new(1.0, settings_color);
    painter.line_segment([center + Vec2::new(-1.5, -3.0), center + Vec2::new(1.5, 0.0)], stroke);
    painter.line_segment([center + Vec2::new(1.5, 0.0), center + Vec2::new(-1.5, 3.0)], stroke);
    painter.galley(label_pos, label, settings_color);
    if settings.clicked() {
        actions.push(TabbarAction::Settings);
    }
    actions
}

fn row_height(tab: &TabInfo) -> f32 {
    theme::TAB_ROW_HEIGHT + if tab.claude.is_some() { theme::CLAUDE_ROW_HEIGHT } else { 0.0 }
}

/// One tab's row: background, number, activity dot, title, Claude line, close
/// button and the click, drag and context-menu handling.
#[allow(clippy::too_many_arguments)]
fn tab_row(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    state: &mut TabbarState,
    index: usize,
    tab: &TabInfo,
    row: Rect,
    response: egui::Response,
    badge_fields: &ClaudeBadgeFields,
    actions: &mut Vec<TabbarAction>,
) {
    if response.hovered() {
        state.hover_index = Some(index);
    }
    if tab.active {
        painter.rect_filled(row, 0.0, theme::colors().tab_active_bg);
    } else if response.hovered() {
        painter.rect_filled(row, 0.0, theme::colors().tab_hover_bg);
    }
    if let Some(color) = tab.color {
        // The reference marks a tab with a 3px bar along its edge; in this
        // vertical list the nearest edge to the content is the left one, and
        // the bar sits above the row's background so it reads on every state.
        painter.rect_filled(Rect::from_min_max(row.min, Pos2::new(row.min.x + 3.0, row.max.y)), 0.0, color.color());
    }

    if let Some(rename) = state.rename.as_mut().filter(|rename| rename.tab == index) {
        let field_rect = Rect::from_min_size(
            Pos2::new(row.min.x + 30.0, row.min.y + 4.0),
            Vec2::new(row.width() - 40.0, row.height() - 8.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(field_rect), |ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut rename.text).desired_width(field_rect.width()));
            if rename.focus {
                field.request_focus();
                rename.focus = false;
            }
            let commit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if commit {
                actions.push(TabbarAction::Rename(index, rename.text.trim().to_owned()));
            }
            if escape {
                actions.push(TabbarAction::Rename(index, tab.title.to_string()));
            }
        });
        if !actions.is_empty() {
            state.rename = None;
        }
        return;
    }

    let number_color = if tab.active { theme::colors().tab_active_number } else { theme::colors().tab_number };
    painter.text(
        Pos2::new(row.min.x + 14.0 + 11.0, row.min.y + 17.0),
        Align2::CENTER_CENTER,
        (index + 1).to_string(),
        theme::field_font(12.0),
        number_color,
    );
    if tab.activity {
        painter.circle_filled(Pos2::new(row.min.x + 7.0, row.min.y + 17.0), 2.0, theme::colors().accent);
    }
    let text_color = if tab.active { theme::colors().tab_active_text } else { theme::colors().tab_text };
    let title_rect = Rect::from_min_size(
        Pos2::new(row.min.x + 36.0, row.min.y),
        Vec2::new(row.width() - 58.0, theme::TAB_ROW_HEIGHT),
    );
    let title_font = theme::font(12.5);
    let title = elide(painter, &tab.title, title_font.clone(), title_rect.width());
    painter.with_clip_rect(title_rect).text(
        Pos2::new(title_rect.min.x, title_rect.center().y),
        Align2::LEFT_CENTER,
        title,
        title_font,
        text_color,
    );
    if let Some(record) = &tab.claude {
        paint_claude_line(painter, row, record, badge_fields);
    }

    // Close button: the row's own response owns the click (a nested widget
    // registered later would never win the press), so hit-test by position.
    let close_rect = Rect::from_min_size(Pos2::new(row.max.x - 22.0, row.min.y + 8.0), Vec2::splat(18.0));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let close_hovered = response.hovered() && pointer.is_some_and(|pos| close_rect.contains(pos));
    if response.hovered() {
        let color = if close_hovered { theme::colors().tab_active_text } else { theme::colors().tab_text };
        painter.text(close_rect.center(), Align2::CENTER_CENTER, "×", theme::font(14.0), color);
    }

    if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Middle)) && response.hovered() {
        actions.push(TabbarAction::Close(index));
    }
    if response.double_clicked() {
        state.rename = Some(RenameEdit { tab: index, text: tab.title.to_string(), focus: true });
    } else if response.drag_started() {
        state.drag_from = Some(index);
    } else if response.clicked() && state.drag_from.is_none() {
        let clicked_close = response.interact_pointer_pos().is_some_and(|pos| close_rect.contains(pos));
        if clicked_close {
            actions.push(TabbarAction::Close(index));
        } else {
            actions.push(TabbarAction::Select(index));
        }
    }
    response.context_menu(|ui| {
        if ui.button(strings::TAB_RENAME).clicked() {
            state.rename = Some(RenameEdit { tab: index, text: tab.title.to_string(), focus: true });
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button(strings::TAB_DUPLICATE).clicked() {
            actions.push(TabbarAction::Duplicate(index));
            ui.close_kind(egui::UiKind::Menu);
        }
        // The current colour rides on the submenu label, as in the reference:
        // the menu answers "which colour is this" without opening it.
        let mut submenu = egui::text::LayoutJob::default();
        submenu.append(
            strings::TAB_COLOR,
            0.0,
            egui::TextFormat { font_id: theme::field_font(13.0), color: theme::colors().text, ..Default::default() },
        );
        submenu.append(
            &format!("  {}", tab.color.map_or(strings::TAB_COLOR_NONE, theme::TabColor::label)),
            0.0,
            egui::TextFormat { font_id: theme::field_font(11.5), color: theme::colors().faint, ..Default::default() },
        );
        ui.menu_button(submenu, |ui| {
            if color_item(ui, tab.color.is_none(), None) {
                actions.push(TabbarAction::SetColor(index, None));
                ui.close();
            }
            for color in theme::TabColor::ALL {
                if color_item(ui, tab.color == Some(color), Some(color)) {
                    actions.push(TabbarAction::SetColor(index, Some(color)));
                    ui.close();
                }
            }
        });
        if ui.button(strings::TAB_CLOSE).clicked() {
            actions.push(TabbarAction::Close(index));
            ui.close_kind(egui::UiKind::Menu);
        }
        if ui.button(strings::TAB_CLOSE_OTHERS).clicked() {
            actions.push(TabbarAction::CloseOthers(index));
            ui.close_kind(egui::UiKind::Menu);
        }
    });
}

/// One row of the colour submenu: the chosen colour is marked by the radio,
/// and every row carries the colour itself as a swatch. A list of colour
/// names would describe the choices without showing any of them.
fn color_item(ui: &mut egui::Ui, selected: bool, color: Option<theme::TabColor>) -> bool {
    let mut job = egui::text::LayoutJob::default();
    match color {
        Some(color) => job.append(
            "■   ",
            0.0,
            egui::TextFormat { font_id: theme::field_font(12.0), color: color.color(), ..Default::default() },
        ),
        // "No colour" wears the same swatch shape, hollow, so the names align.
        None => job.append(
            "□   ",
            0.0,
            egui::TextFormat { font_id: theme::field_font(12.0), color: theme::colors().faint, ..Default::default() },
        ),
    }
    job.append(
        color.map_or(strings::TAB_COLOR_NONE, theme::TabColor::label),
        0.0,
        egui::TextFormat { font_id: theme::field_font(13.0), color: theme::colors().text, ..Default::default() },
    );
    ui.radio(selected, job).clicked()
}

/// One implementation for both callers: a byte-identical copy of this drifted
/// from the workspace panel's. Binary search over char boundaries keeps it at a
/// handful of layouts instead of one per character.
pub(crate) fn elide(painter: &egui::Painter, text: &str, font: FontId, max_width: f32) -> String {
    let measure = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    if measure(text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let fits = |count: usize| {
        let cut = chars.get(count).map_or(text.len(), |(offset, _)| *offset);
        measure(&format!("{}…", &text[..cut])) <= max_width
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
    // `low` is the first prefix that does NOT fit, not the last fitting one.
    let cut = chars.get(low.saturating_sub(1)).map_or(0, |(offset, _)| *offset);
    format!("{}…", &text[..cut])
}

use crate::config::ClaudeBadgeFields;

/// Text pieces of the Claude line under a tab, each with the share that
/// colours it (None: plain tab text).
fn badge_parts(record: &StatusRecord, fields: &ClaudeBadgeFields) -> Vec<(String, Option<f64>)> {
    let mut parts = Vec::new();
    if let Some(model) = record.model.as_ref().filter(|_| fields.model) {
        parts.push((model.clone(), None));
    }
    if let Some(pct) = record.context_pct.map(clamp_pct).filter(|_| fields.context) {
        let filled = js_round(pct / 10.0).clamp(0, 10) as usize;
        let bar = format!("{}{} {}%", "▓".repeat(filled), "░".repeat(10 - filled), js_round(pct));
        parts.push((bar, Some(pct)));
    }
    if let Some(pct) = record.five_hour_pct.map(clamp_pct).filter(|_| fields.five_hour) {
        parts.push((format!("5h {}", js_round(pct)), Some(pct)));
    }
    if let Some(pct) = record.seven_day_pct.map(clamp_pct).filter(|_| fields.seven_day) {
        parts.push((format!("7d {}", js_round(pct)), Some(pct)));
    }
    if let Some(agent) = record.agent.as_ref().filter(|_| fields.agent) {
        parts.push((agent.clone(), None));
    }
    parts
}

/// The Claude badge line: `Opus 5 · ▓▓▓▓░░░░░░ 37% · 5h 17 · 7d 64` with the
/// percentage colours of the dark scheme, limited to the chosen pieces.
fn paint_claude_line(painter: &egui::Painter, row: Rect, record: &StatusRecord, fields: &ClaudeBadgeFields) {
    let mut x = row.min.x + 36.0;
    let y = row.min.y + theme::TAB_ROW_HEIGHT + theme::CLAUDE_ROW_HEIGHT / 2.0;
    let font = FontId::proportional(11.0);
    let limit = row.max.x - 10.0;
    for (index, (text, pct)) in badge_parts(record, fields).into_iter().enumerate() {
        let color = pct.map(theme::threshold_color).unwrap_or(theme::colors().tab_text);
        let prefix = if index == 0 { "" } else { " · " };
        let galley = painter.layout_no_wrap(format!("{prefix}{text}"), font.clone(), color);
        let width = galley.size().x;
        if x + width > limit {
            break;
        }
        painter.galley(Pos2::new(x, y - galley.size().y / 2.0), galley, color);
        x += width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_frame(
        ctx: &egui::Context,
        state: &mut TabbarState,
        tabs: &[TabInfo],
        rect: Rect,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<TabbarAction>) {
        let mut actions = Vec::new();
        let output = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), events, ..Default::default() }, |ui| {
            actions = show(ui, rect, state, tabs, false, &Default::default(), 0);
        });
        (output, actions)
    }

    #[test]
    fn elision_fits_and_egui_reuses_unchanged_layouts() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Test", &[], false);
        let mut previous = None;
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                let painter = ui.painter();
                let font = theme::font(12.5);
                let layout = painter.layout_no_wrap("cached".into(), font.clone(), Color32::WHITE);
                if let Some(old) = &previous {
                    assert!(std::sync::Arc::ptr_eq(old, &layout), "egui already caches identical layout jobs");
                }
                previous = Some(layout);
                for width in [25.0, 40.0, 80.0] {
                    let text = elide(painter, "длинное название вкладки", font.clone(), width);
                    let measured = painter.layout_no_wrap(text, font.clone(), Color32::WHITE).size().x;
                    assert!(measured <= width, "elision overflows: {measured} > {width}");
                }
            });
        }
    }

    #[test]
    fn rows_start_below_the_title_and_drop_targets_follow_actual_scrolled_rects() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Test", &[], false);
        let tabs: Vec<_> = (0..6)
            .map(|index| TabInfo {
                title: format!("tab {index}").into(),
                active: false,
                activity: false,
                claude: None,
                color: Some(theme::TabColor::ALL[index]),
            })
            .collect();
        let rect = Rect::from_min_size(Pos2::new(0.0, 30.0), Vec2::new(180.0, 200.0));
        let mut state = TabbarState::default();
        let (output, _) = list_frame(&ctx, &mut state, &tabs, rect, Vec::new());
        let bars: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(r) if r.rect.width() == 3.0 => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(bars[0].min.y, rect.min.y, "the title bar must not be covered by the first tab");
        assert_eq!(bars[1].min.y, bars[0].max.y, "no hidden item_spacing between rows");
        let pointer = Pos2::new(70.0, rect.min.y + 45.0);
        let scroll = vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -90.0),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            },
        ];
        let _ = list_frame(&ctx, &mut state, &tabs, rect, scroll);
        for _ in 0..30 {
            let _ = list_frame(&ctx, &mut state, &tabs, rect, Vec::new());
        }
        let (scrolled, _) = list_frame(&ctx, &mut state, &tabs, rect, Vec::new());
        let bars: Vec<_> = scrolled
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(r) if r.rect.width() == 3.0 => Some((r.rect, r.fill)),
                _ => None,
            })
            .collect();
        let (row, color) = bars.iter().find(|(row, _)| row.min.y <= pointer.y && pointer.y < row.max.y).unwrap();
        let target = theme::TabColor::ALL.iter().position(|c| c.color() == *color).unwrap();
        assert!(row.min.y < rect.min.y + target as f32 * row_height(&tabs[0]), "the fixture actually scrolled");
        state.drag_from = Some(0);
        let release = egui::Event::PointerButton {
            pos: pointer,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        };
        let (_, actions) = list_frame(&ctx, &mut state, &tabs, rect, vec![release]);
        assert!(actions.contains(&TabbarAction::Move(0, target)), "wrong drop target: {actions:?}");
    }

    #[test]
    fn settings_chevron_is_centered_on_the_visible_label_at_each_scale() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Test", &[], false);
        let rect = Rect::from_min_size(Pos2::new(0.0, 30.0), Vec2::new(195.0, 400.0));
        let mut state = TabbarState::default();
        for scale in [1.0, 1.25, 1.5] {
            ctx.set_pixels_per_point(scale);
            let (output, _) = list_frame(&ctx, &mut state, &[], rect, Vec::new());
            let label = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == strings::TAB_SETTINGS => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap();
            let arrows: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::LineSegment { points, .. }
                        if points[0].x < label.min.x && points[1].x < label.min.x =>
                    {
                        Some(points)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(arrows.len(), 2);
            let center_y = (arrows[0][0].y + arrows[1][1].y) / 2.0;
            assert!((center_y - label.center().y).abs() < 0.1);
        }
    }

    #[test]
    fn action_buttons_follow_short_lists_and_stay_above_settings_when_scrolled() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Test", &[], false);
        let rect = Rect::from_min_size(Pos2::new(0.0, 30.0), Vec2::new(195.0, 400.0));
        for count in [1, 20] {
            let tabs: Vec<_> = (0..count)
                .map(|_| TabInfo { title: "tab".into(), active: false, activity: false, claude: None, color: None })
                .collect();
            let mut state = TabbarState::default();
            let (output, _) = list_frame(&ctx, &mut state, &tabs, rect, Vec::new());
            let plus = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "+" => Some(text.visual_bounding_rect()),
                    _ => None,
                })
                .unwrap();
            let expected = rect.min.y + (count as f32 * row_height(&tabs[0])).min(rect.height() - 70.0);
            assert!((plus.center().y - (expected + 20.0)).abs() < 1.0);
            assert!(plus.max.y < rect.max.y - 30.0, "buttons overlap settings: {plus:?}");
            let pos = Pos2::new(rect.min.x + 22.0, expected + 20.0);
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let _ = list_frame(&ctx, &mut state, &tabs, rect, vec![egui::Event::PointerMoved(pos), button(true)]);
            let (_, actions) = list_frame(&ctx, &mut state, &tabs, rect, vec![button(false)]);
            assert!(actions.contains(&TabbarAction::NewTab), "plus is not clickable: {actions:?}");
        }
    }

    #[test]
    fn insertion_and_reordering_keep_edits_bound_to_the_same_tab() {
        for from in 0..5 {
            for to in 0..5 {
                for slot in 0..5 {
                    let mut identities: Vec<_> = (0..5).collect();
                    let moved = identities.remove(from);
                    identities.insert(to, moved);
                    let mut state = TabbarState {
                        rename: Some(RenameEdit { tab: slot, text: "test".into(), focus: false }),
                        drag_from: Some(slot),
                        hover_index: Some(slot),
                    };
                    state.tab_moved(from, to);
                    let expected = identities.iter().position(|id| *id == slot).unwrap();
                    assert_eq!(state.rename.as_ref().unwrap().tab, expected);
                    assert_eq!(state.drag_from, Some(expected));
                    state.tab_inserted(0);
                    assert_eq!(state.rename.as_ref().unwrap().tab, expected + 1);
                    assert_eq!(state.hover_index, Some(expected + 1));
                }
            }
        }
    }

    #[test]
    fn badge_parts_follow_the_fields() {
        use crate::config::ClaudeBadgeFields;
        let record = StatusRecord {
            model: Some("Opus 5".into()),
            context_pct: Some(37.4),
            five_hour_pct: Some(17.0),
            seven_day_pct: Some(64.0),
            agent: Some("reviewer".into()),
            ..StatusRecord::default()
        };
        let texts =
            |fields: &ClaudeBadgeFields| badge_parts(&record, fields).into_iter().map(|(t, _)| t).collect::<Vec<_>>();
        assert_eq!(
            texts(&ClaudeBadgeFields::default()),
            vec!["Opus 5", "▓▓▓▓░░░░░░ 37%", "5h 17", "7d 64", "reviewer"]
        );
        let fields = ClaudeBadgeFields { model: false, five_hour: false, ..ClaudeBadgeFields::default() };
        assert_eq!(texts(&fields), vec!["▓▓▓▓░░░░░░ 37%", "7d 64", "reviewer"]);
        assert_eq!(badge_parts(&record, &fields)[0].1, Some(37.4), "percent parts carry their value for the colour");
        // A broken or hostile payload is clamped here exactly as the status
        // line clamps it, so the badge cannot print `150%` beside `100%`.
        let wild = StatusRecord { context_pct: Some(150.0), five_hour_pct: Some(-20.0), ..StatusRecord::default() };
        let parts = badge_parts(&wild, &ClaudeBadgeFields::default());
        assert_eq!(parts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["▓▓▓▓▓▓▓▓▓▓ 100%", "5h 0"]);
        assert_eq!(parts[0].1, Some(100.0), "the colour follows the clamped value, not the raw one");
    }

    /// More tabs than fit must stay reachable: the list scrolls, and a tab past the
    /// fold is still painted and still hit-testable where the scroll put it.
    #[test]
    fn a_scrolled_list_still_reaches_the_tabs_past_the_fold() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), true);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let blank = || TabInfo { title: "".into(), active: false, activity: false, claude: None, color: None };
        let many: Vec<TabInfo> =
            (0..12).map(|index| TabInfo { title: format!("tab {index}").into(), ..blank() }).collect();
        let mut state = TabbarState::default();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(180.0, 120.0));
        let mut actions = Vec::new();
        let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
            actions = show(ui, rect, &mut state, &many, false, &Default::default(), 0);
        });
        assert!(actions.is_empty(), "painting alone reports no actions");
        // Every row is laid out inside the scrolled content, not clipped away.
        let heights: f32 = many.iter().map(row_height).sum();
        assert!(heights > rect.height(), "the fixture really does overflow: {heights} vs {}", rect.height());
        assert_eq!(row_height(&many[0]), theme::TAB_ROW_HEIGHT);
    }

    /// A coloured tab is marked with a bar in that colour on its leading edge,
    /// and an uncoloured one paints no bar at all. The colour is the whole
    /// point of the menu, so it has to reach the row.
    #[test]
    fn a_coloured_tab_paints_a_bar_in_that_colour() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), true);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(180.0, 120.0));
        let bars = |color: Option<theme::TabColor>| {
            let tab = TabInfo { title: "t".into(), active: false, activity: false, claude: None, color };
            let mut state = TabbarState::default();
            let mut actions = Vec::new();
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
                actions = show(ui, rect, &mut state, std::slice::from_ref(&tab), false, &Default::default(), 0);
            });
            assert!(actions.is_empty());
            output
                .shapes
                .iter()
                .filter(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape) => {
                        shape.rect.width() == 3.0 && color.is_some_and(|color| shape.fill == color.color())
                    }
                    _ => false,
                })
                .count()
        };
        assert_eq!(bars(Some(theme::TabColor::Red)), 1, "one bar in the colour");
        assert_eq!(bars(None), 0, "no colour, no bar");
    }

    /// A rename or drag in progress names a tab by index: when an earlier tab
    /// closes (a pane exiting in the background), it must follow its tab, not
    /// land on the neighbour that slid into its place.
    #[test]
    fn closing_a_tab_keeps_rename_and_drag_on_their_tabs() {
        let mut state = TabbarState {
            rename: Some(RenameEdit { tab: 3, text: "x".to_owned(), focus: false }),
            drag_from: Some(2),
            hover_index: Some(1),
        };
        state.tab_removed(0);
        assert_eq!(state.rename.as_ref().map(|r| r.tab), Some(2));
        assert_eq!((state.drag_from, state.hover_index), (Some(1), Some(0)));
        state.tab_removed(2);
        assert!(state.rename.is_none(), "the renamed tab itself closed");
        state.tab_removed(1);
        assert_eq!((state.drag_from, state.hover_index), (None, None), "the dragged tab closed");
    }
}
