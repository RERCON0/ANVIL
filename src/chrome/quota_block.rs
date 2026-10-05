//! The quota block at the bottom of the tab column. Painting and clicks only:
//! what is shown, dimmed or hidden is decided by `quota::view`.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::chrome::tabbar::elide;
use crate::quota::view::{self, Fit, Row};
use crate::strings;
use crate::theme;

pub struct QuotaBlock<'a> {
    pub rows: &'a [Row],
    pub collapsed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaAction {
    ToggleCollapsed,
    Refresh,
}

const FONT_SIZE: f32 = 11.0;
const LEFT: f32 = 12.0;
const RIGHT: f32 = 10.0;

fn segment(job: &mut LayoutJob, text: &str, font: &FontId, color: Color32) {
    job.append(text, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
}

/// Paints the block anchored to the bottom of `area` and reports a header click.
pub fn show(ui: &mut egui::Ui, area: Rect, block: &QuotaBlock) -> Option<QuotaAction> {
    let fit = view::fit(area.height(), block.rows, block.collapsed);
    let height = match fit {
        Fit::Hidden => return None,
        Fit::Collapsed => view::HEADER_HEIGHT,
        Fit::Expanded => view::expanded_height(block.rows),
    };
    let rect = Rect::from_min_max(Pos2::new(area.min.x, area.max.y - height), area.max);
    let painter = ui.painter_at(rect);
    let font = FontId::proportional(FONT_SIZE);
    let colors = theme::colors();
    painter.hline(rect.x_range().shrink(8.0), rect.min.y + 0.5, Stroke::new(1.0, colors.line));

    // Header: title (or the hottest window when collapsed), refresh and chevron on hover.
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), view::HEADER_HEIGHT));
    let response = ui.interact(header, ui.id().with("quota-header"), Sense::click());
    let refresh_rect =
        Rect::from_min_size(Pos2::new(header.max.x - RIGHT - 30.0, header.min.y), Vec2::new(16.0, header.height()));
    let y = header.center().y;
    let title_galley = painter.layout_no_wrap(strings::QUOTA_TITLE.to_owned(), font.clone(), colors.faint);
    let title_width = title_galley.size().x;
    painter.galley(Pos2::new(header.min.x + LEFT, y - title_galley.size().y / 2.0), title_galley, colors.faint);
    if let (Fit::Collapsed, Some((text, pct))) = (fit, view::summary(block.rows)) {
        let x = header.min.x + LEFT + title_width;
        let text = elide(&painter, &format!(" · {text}"), font.clone(), refresh_rect.min.x - x - 4.0);
        painter.text(Pos2::new(x, y), Align2::LEFT_CENTER, text, font.clone(), theme::threshold_color(pct));
    }
    if response.hovered() {
        if ui.fonts_mut(|f| f.has_glyphs(&font, "↻")) {
            painter.text(refresh_rect.center(), Align2::CENTER_CENTER, "↻", font.clone(), colors.icon_hover);
        }
        let down = if ui.fonts_mut(|f| f.has_glyphs(&font, "˅")) { "˅" } else { "v" };
        let chevron = if fit == Fit::Expanded { down } else { "›" };
        painter.text(
            Pos2::new(header.max.x - RIGHT, y),
            Align2::RIGHT_CENTER,
            chevron,
            font.clone(),
            colors.icon_hover,
        );
    }
    let pointer_on_refresh = response.hover_pos().is_some_and(|p| refresh_rect.contains(p));
    let response = if pointer_on_refresh { response.on_hover_text(strings::QUOTA_REFRESH_HINT) } else { response };
    let action = response.clicked().then(|| {
        let on_refresh = response.interact_pointer_pos().is_some_and(|p| refresh_rect.contains(p));
        if on_refresh {
            QuotaAction::Refresh
        } else {
            QuotaAction::ToggleCollapsed
        }
    });

    if fit != Fit::Expanded {
        return action;
    }
    let reset_icon = ui.fonts_mut(|f| f.has_glyphs(&font, "↺"));
    let mut top = header.max.y;
    for (index, row) in block.rows.iter().enumerate() {
        let lines = view::row_lines(row);
        let row_rect =
            Rect::from_min_size(Pos2::new(rect.min.x, top), Vec2::new(rect.width(), lines as f32 * view::LINE_HEIGHT));
        let name_color = if row.dim { colors.dim } else { colors.tab_text };
        let name_galley = painter.layout_no_wrap(row.name.to_owned(), font.clone(), name_color);
        let name_width = name_galley.size().x;
        let first_y = top + view::LINE_HEIGHT / 2.0;
        painter.galley(Pos2::new(rect.min.x + LEFT, first_y - name_galley.size().y / 2.0), name_galley, name_color);
        let room = rect.width() - LEFT - RIGHT - name_width - 8.0;
        for (i, line) in row.lines.iter().enumerate() {
            let line_y = top + (i as f32 + 0.5) * view::LINE_HEIGHT;
            let pct_color = if row.dim { colors.dim } else { theme::threshold_color(line.used_pct) };
            let mut job = LayoutJob::default();
            segment(&mut job, &format!("{} ", line.label), &font, if row.dim { colors.dim } else { colors.tab_text });
            segment(&mut job, &view::percent(line.used_pct), &font, pct_color);
            let mut galley = painter.layout_job(job.clone());
            if let Some(hint) = &line.reset_hint {
                let hint = if reset_icon { format!("  ↺ {hint}") } else { format!("  {hint}") };
                let mut with_hint = job;
                segment(&mut with_hint, &hint, &font, colors.dim);
                let candidate = painter.layout_job(with_hint);
                if candidate.size().x <= room {
                    galley = candidate;
                }
            }
            let pos = Pos2::new(rect.max.x - RIGHT - galley.size().x, line_y - galley.size().y / 2.0);
            painter.galley(pos, galley, colors.tab_text);
        }
        if let Some(note) = row.note {
            let note_y =
                if row.lines.is_empty() { first_y } else { top + (row.lines.len() as f32 + 0.5) * view::LINE_HEIGHT };
            painter.text(Pos2::new(rect.max.x - RIGHT, note_y), Align2::RIGHT_CENTER, note, font.clone(), colors.dim);
        }
        let _ = ui.interact(row_rect, ui.id().with(("quota-row", index)), Sense::hover()).on_hover_text(&row.tooltip);
        top = row_rect.max.y;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::QuotaConfig;
    use crate::quota::model::{ProviderId, ProviderSnapshot, ProviderState, Snapshot, Window};
    use crate::quota::view::HEADER_HEIGHT;

    const NOW: i64 = 1_791_210_000;

    fn rows() -> Vec<Row> {
        let provider = |id, state, pct| ProviderSnapshot {
            id,
            plan: Some("Max".into()),
            source: "Claude Code".into(),
            state,
            windows: vec![Window::new("5h", "5ч", pct, Some(NOW + 3_600))],
            fetched_at: Some(NOW - 60),
            checked_at: NOW,
        };
        let snapshot = Snapshot {
            version: Snapshot::VERSION,
            providers: vec![
                provider(ProviderId::Claude, ProviderState::Ok, 42.0),
                provider(ProviderId::Kimi, ProviderState::RateLimited { retry_at: NOW + 300 }, 91.0),
            ],
        };
        view::rows(&snapshot, &QuotaConfig::default(), NOW)
    }

    fn painted(ctx: &egui::Context, area: Rect, block: &QuotaBlock) -> Option<QuotaAction> {
        let mut seen = None;
        let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(area), ..Default::default() }, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                seen = show(ui, area, block);
            });
        });
        seen
    }

    /// Column widths are the owner's narrowest, and the column grows shorter
    /// as tabs pile up: the block must paint (or cleanly hide) at every size
    /// rather than panic or spill over Settings.
    #[test]
    fn it_paints_at_every_column_height_in_both_states() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), true);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let rows = rows();
        for collapsed in [false, true] {
            for height in [0.0_f32, 1.0, 10.0, HEADER_HEIGHT, 40.0, 120.0, 600.0] {
                let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(theme::TABBAR_WIDTH, height));
                let block = QuotaBlock { rows: &rows, collapsed };
                let _ = painted(&ctx, area, &block);
            }
        }
    }

    /// The header does two jobs: anywhere it collapses, its right edge asks for
    /// a cycle. The glyph zone is 16 px wide, 40 px from the right border.
    #[test]
    fn the_header_collapses_wherever_clicked_and_refreshes_at_the_right_edge() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), true);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let rows = rows();
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(theme::TABBAR_WIDTH, 600.0));
        let height = view::expanded_height(&rows);
        for (x, expected) in [(40.0, QuotaAction::ToggleCollapsed), (160.0, QuotaAction::Refresh)] {
            let pos = Pos2::new(x, area.max.y - height + HEADER_HEIGHT / 2.0);
            let block = QuotaBlock { rows: &rows, collapsed: false };
            let click = |pressed| {
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }]
            };
            // egui answers `clicked()` only after a press frame and a release
            // frame, with a move before them.
            let _ = painted(&ctx, area, &block);
            let mut seen = None;
            for events in [vec![egui::Event::PointerMoved(pos)], click(true), click(false)] {
                let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(area), events, ..Default::default() }, |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                        seen = show(ui, area, &block);
                    });
                });
            }
            assert_eq!(seen, Some(expected), "x = {x}");
        }
    }
}
