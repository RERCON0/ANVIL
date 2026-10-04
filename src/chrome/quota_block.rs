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
        painter.text(Pos2::new(header.max.x - RIGHT, y), Align2::RIGHT_CENTER, chevron, font.clone(), colors.icon_hover);
    }
    let pointer_on_refresh = response.hover_pos().is_some_and(|p| refresh_rect.contains(p));
    let response = if pointer_on_refresh { response.on_hover_text(strings::QUOTA_REFRESH_HINT) } else { response };
    let action = response.clicked().then(|| {
        let on_refresh = response.interact_pointer_pos().is_some_and(|p| refresh_rect.contains(p));
        if on_refresh { QuotaAction::Refresh } else { QuotaAction::ToggleCollapsed }
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
            let note_y = if row.lines.is_empty() {
                first_y
            } else {
                top + (row.lines.len() as f32 + 0.5) * view::LINE_HEIGHT
            };
            painter.text(Pos2::new(rect.max.x - RIGHT, note_y), Align2::RIGHT_CENTER, note, font.clone(), colors.dim);
        }
        let _ = ui.interact(row_rect, ui.id().with(("quota-row", index)), Sense::hover()).on_hover_text(&row.tooltip);
        top = row_rect.max.y;
    }
    action
}