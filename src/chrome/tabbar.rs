//! Vertical tab list on the left: numbers, titles, the Claude badge, activity
//! dots, drag & drop, rename in place and the buttons under the list.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Vec2};

use crate::claude_status::{js_round, StatusRecord};
use crate::strings;
use crate::theme;

#[derive(Default)]
pub struct TabbarState {
    pub rename: Option<RenameEdit>,
    drag_from: Option<usize>,
    hover_index: Option<usize>,
}

impl TabbarState {
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

pub struct TabInfo {
    pub title: String,
    pub active: bool,
    pub activity: bool,
    pub claude: Option<StatusRecord>,
}

pub enum TabbarAction {
    Select(usize),
    Close(usize),
    Duplicate(usize),
    CloseOthers(usize),
    Rename(usize, String),
    Move(usize, usize),
    NewTab,
    Profiles,
    Settings,
    QuotaToggle,
    QuotaRefresh,
}

pub fn show(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &mut TabbarState,
    tabs: &[TabInfo],
    settings_open: bool,
    badge_fields: &ClaudeBadgeFields,
    quota: Option<&crate::chrome::quota_block::QuotaBlock>,
) -> Vec<TabbarAction> {
    let mut actions = Vec::new();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::colors().chrome_bg);
    let mut y = rect.min.y;
    state.hover_index = None;

    for (index, tab) in tabs.iter().enumerate() {
        let height = theme::TAB_ROW_HEIGHT + if tab.claude.is_some() { theme::CLAUDE_ROW_HEIGHT } else { 0.0 };
        let row = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), height));
        y += height;
        if row.max.y > rect.max.y {
            break;
        }
        let response = ui.interact(row, ui.id().with(("tab", index)), Sense::click_and_drag());
        if response.hovered() {
            state.hover_index = Some(index);
        }
        if tab.active {
            painter.rect_filled(row, 0.0, theme::colors().tab_active_bg);
        } else if response.hovered() {
            painter.rect_filled(row, 0.0, theme::colors().tab_hover_bg);
        }

        if let Some(rename) = state.rename.as_mut().filter(|rename| rename.tab == index) {
            let field_rect = Rect::from_min_size(Pos2::new(row.min.x + 30.0, row.min.y + 4.0), Vec2::new(row.width() - 40.0, row.height() - 8.0));
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
                    actions.push(TabbarAction::Rename(index, tab.title.clone()));
                }
            });
            if !actions.is_empty() {
                state.rename = None;
            }
            continue;
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
        let title_rect = Rect::from_min_size(Pos2::new(row.min.x + 36.0, row.min.y), Vec2::new(row.width() - 58.0, theme::TAB_ROW_HEIGHT));
        let title_font = theme::font(12.5);
        let title = elide(&painter, &tab.title, title_font.clone(), title_rect.width());
        painter.with_clip_rect(title_rect).text(
            Pos2::new(title_rect.min.x, title_rect.center().y),
            Align2::LEFT_CENTER,
            title,
            title_font,
            text_color,
        );
        if let Some(record) = &tab.claude {
            paint_claude_line(&painter, row, record, badge_fields);
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
            state.rename = Some(RenameEdit { tab: index, text: tab.title.clone(), focus: true });
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
                state.rename = Some(RenameEdit { tab: index, text: tab.title.clone(), focus: true });
                ui.close_kind(egui::UiKind::Menu);
            }
            if ui.button(strings::TAB_DUPLICATE).clicked() {
                actions.push(TabbarAction::Duplicate(index));
                ui.close_kind(egui::UiKind::Menu);
            }
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

    // Drag & drop reordering.
    if let Some(from) = state.drag_from {
        if ui.input(|i| i.pointer.any_released()) {
            if let Some(to) = state.hover_index {
                if from != to {
                    actions.push(TabbarAction::Move(from, to));
                }
            }
            state.drag_from = None;
        } else if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            let target = tabs
                .iter()
                .enumerate()
                .find(|(index, tab)| {
                    let height = theme::TAB_ROW_HEIGHT + if tab.claude.is_some() { theme::CLAUDE_ROW_HEIGHT } else { 0.0 };
                    let top = rect.min.y + (0..*index).map(|i| row_height(&tabs[i])).sum::<f32>();
                    pos.y >= top && pos.y < top + height
                })
                .map(|(index, _)| index);
            if let Some(target) = target {
                state.hover_index = Some(target);
            }
        }
    }

    // Buttons under the list.
    let y = y.max(rect.min.y);
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

    let settings_rect =
        Rect::from_min_size(Pos2::new(rect.min.x + 8.0, rect.max.y - 30.0), Vec2::new(rect.width() - 16.0, 24.0));

    // Tabs and their buttons get the space first; the quota block takes what is
    // left above Settings and collapses or hides itself when that is too little.
    if let Some(block) = quota {
        let bottom = Pos2::new(rect.max.x, settings_rect.min.y - 4.0);
        let area = Rect::from_min_max(Pos2::new(rect.min.x, y + 40.0), bottom);
        if area.height() > 0.0 {
            use crate::chrome::quota_block::{show as show_quota, QuotaAction};
            match show_quota(ui, area, block) {
                Some(QuotaAction::ToggleCollapsed) => actions.push(TabbarAction::QuotaToggle),
                Some(QuotaAction::Refresh) => actions.push(TabbarAction::QuotaRefresh),
                None => {}
            }
        }
    }
    let settings = ui.interact(settings_rect, ui.id().with("tab-settings"), Sense::click());
    if settings_open {
        painter.rect_filled(settings_rect, 0.0, theme::colors().tab_active_bg);
    }
    let settings_color = if settings_open || settings.hovered() { theme::colors().icon_hover } else { theme::colors().icon };
    painter.text(
        Pos2::new(settings_rect.min.x + 10.0, settings_rect.center().y),
        Align2::LEFT_CENTER,
        format!("› {}", strings::TAB_SETTINGS),
        theme::font(12.5),
        settings_color,
    );
    if settings.clicked() {
        actions.push(TabbarAction::Settings);
    }
    actions
}

fn row_height(tab: &TabInfo) -> f32 {
    theme::TAB_ROW_HEIGHT + if tab.claude.is_some() { theme::CLAUDE_ROW_HEIGHT } else { 0.0 }
}

pub(crate) fn elide(painter: &egui::Painter, text: &str, font: FontId, max_width: f32) -> String {
    let measure = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
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

use crate::config::ClaudeBadgeFields;

/// Text pieces of the Claude line under a tab, each with the share that
/// colours it (None: plain tab text).
fn badge_parts(record: &StatusRecord, fields: &ClaudeBadgeFields) -> Vec<(String, Option<f64>)> {
    let mut parts = Vec::new();
    if let Some(model) = record.model.as_ref().filter(|_| fields.model) {
        parts.push((model.clone(), None));
    }
    if let Some(pct) = record.context_pct.filter(|_| fields.context) {
        let filled = js_round(pct / 10.0).clamp(0, 10) as usize;
        let bar = format!("{}{} {}%", "▓".repeat(filled), "░".repeat(10 - filled), js_round(pct));
        parts.push((bar, Some(pct)));
    }
    if let Some(pct) = record.five_hour_pct.filter(|_| fields.five_hour) {
        parts.push((format!("5h {}", js_round(pct)), Some(pct)));
    }
    if let Some(pct) = record.seven_day_pct.filter(|_| fields.seven_day) {
        parts.push((format!("7d {}", js_round(pct)), Some(pct)));
    }
    if let Some(agent) = record.agent.as_ref().filter(|_| fields.agent) {
        parts.push((agent.clone(), None));
    }
    parts
}

/// The Claude badge line: `Opus 5 · ▓▓▓▓░░░░░░ 37% · 5h 17 · 7d 64` with the
/// percentage colours of the Hardcore scheme, limited to the chosen pieces.
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
        assert_eq!(texts(&ClaudeBadgeFields::default()), vec!["Opus 5", "▓▓▓▓░░░░░░ 37%", "5h 17", "7d 64", "reviewer"]);
        let fields = ClaudeBadgeFields { model: false, five_hour: false, ..ClaudeBadgeFields::default() };
        assert_eq!(texts(&fields), vec!["▓▓▓▓░░░░░░ 37%", "7d 64", "reviewer"]);
        assert_eq!(badge_parts(&record, &fields)[0].1, Some(37.4), "percent parts carry their value for the colour");
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
