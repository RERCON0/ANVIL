//! The quota line at the bottom of the window. Painting and clicks only:
//! what is shown, dimmed or folded is decided by `quota::view`.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Pos2, Rangef, Rect, Sense, Stroke, Vec2};

use crate::quota::view::{self, Piece, Segment, Style};
use crate::strings;
use crate::theme;

pub const HEIGHT: f32 = 22.0;
const FONT_SIZE: f32 = 11.5;
const PAD: f32 = 10.0;
/// Room between two segments; a hairline sits in its middle.
const GAP: f32 = 22.0;
const REFRESH_WIDTH: f32 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAction {
    Refresh,
}

/// Colour of a run. An exhausted balance is an alarm and stays red even when
/// the segment is dimmed; everything else dims with it.
fn color(style: Style, dim: bool) -> Color32 {
    let c = theme::colors();
    match style {
        Style::Value { exhausted: true, .. } => c.status_red,
        _ if dim => c.dim,
        Style::Name => c.tab_text,
        Style::Value { pct: Some(pct), .. } => theme::threshold_color(pct),
        Style::Value { pct: None, .. } => c.tab_text,
        Style::Mark | Style::Separator | Style::Hint => c.dim,
    }
}

fn job(pieces: &[Piece], dim: bool, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    for piece in pieces {
        let format = TextFormat { font_id: font.clone(), color: color(piece.style, dim), ..Default::default() };
        if let Some((before, after)) = piece.text.split_once('↺') {
            job.append(before, 0.0, format.clone());
            let mut icon = format.clone();
            icon.font_id.size *= 0.82;
            icon.valign = egui::Align::Center;
            job.append("↺", 0.0, icon);
            job.append(after, 0.0, format);
        } else {
            job.append(&piece.text, 0.0, format);
        }
    }
    job
}

/// Paints the line into `rect` and reports a click on its refresh glyph.
pub fn show(ui: &mut egui::Ui, rect: Rect, segments: &[Segment]) -> Option<BarAction> {
    let painter = ui.painter_at(rect);
    let c = theme::colors();
    painter.rect_filled(rect, 0.0, c.chrome_bg);
    painter.hline(rect.x_range(), rect.min.y + 0.5, Stroke::new(1.0_f32, c.border));
    let font = FontId::proportional(FONT_SIZE);
    let y = rect.center().y;

    // Refresh at the right edge, always in the same place.
    let refresh = Rect::from_min_max(Pos2::new(rect.max.x - REFRESH_WIDTH, rect.min.y), rect.max);
    let response = ui.interact(refresh, ui.id().with("quota-refresh"), Sense::click());
    let icon = if ui.fonts_mut(|f| f.has_glyphs(&font, "↻")) { "↻" } else { strings::QUOTA_REFRESH_FALLBACK() };
    let icon_color = if response.hovered() { c.icon_hover } else { c.icon };
    painter.text(refresh.center(), Align2::CENTER_CENTER, icon, font.clone(), icon_color);
    let response = response.on_hover_text(strings::QUOTA_REFRESH_HINT());
    let action = response.clicked().then_some(BarAction::Refresh);

    if segments.is_empty() {
        painter.text(Pos2::new(rect.min.x + PAD, y), Align2::LEFT_CENTER, strings::QUOTA_NO_DATA(), font, c.dim);
        return action;
    }
    let reset_icon = ui.fonts_mut(|f| f.has_glyphs(&font, "↺"));
    let measure = |text: &str| painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x;
    let width = refresh.min.x - rect.min.x - 2.0 * PAD;
    let layout = view::layout(segments, width, GAP, reset_icon, measure);
    let separator = Stroke::new(1.0_f32, c.line);
    let rule = Rangef::new(rect.min.y + 6.0, rect.max.y - 6.0);
    let mut x = rect.min.x + PAD;
    for (index, segment) in segments.iter().take(layout.shown).enumerate() {
        if index > 0 {
            painter.vline(x - GAP / 2.0, rule, separator);
        }
        let galley = painter.layout_job(job(&view::pieces(segment, layout.detail, reset_icon), segment.dim, &font));
        let size = galley.size();
        let hover = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(size.x, rect.height()));
        painter.galley(Pos2::new(x, y - size.y / 2.0), galley, c.tab_text);
        let _ = ui.interact(hover, ui.id().with(("quota-segment", index)), Sense::hover()).on_hover_ui(|ui| {
            ui.label(&segment.tooltip);
        });
        x += size.x + GAP;
    }
    if layout.hidden > 0 {
        if layout.shown > 0 {
            painter.vline(x - GAP / 2.0, rule, separator);
        }
        let galley = painter.layout_no_wrap(view::more_text(layout.hidden), font.clone(), c.dim);
        let size = galley.size();
        let hover = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(size.x, rect.height()));
        painter.galley(Pos2::new(x, y - size.y / 2.0), galley, c.dim);
        let _ = ui.interact(hover, ui.id().with("quota-more"), Sense::hover()).on_hover_ui(|ui| {
            let folded: Vec<&str> = segments[layout.shown..].iter().map(|s| s.tooltip.as_str()).collect();
            ui.label(folded.join("\n\n"));
        });
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::QuotaConfig;
    use crate::quota::model::{
        Balance, BalanceKind, ProviderId, ProviderSnapshot, ProviderState, Snapshot, Unit, Window,
    };
    use crate::quota::view;

    const NOW: i64 = 1_791_210_000;

    fn segments() -> Vec<Segment> {
        let provider = |id, windows, balances| ProviderSnapshot {
            id,
            plan: None,
            source: "ключ ANVIL".into(),
            state: ProviderState::Ok,
            windows,
            balances,
            fetched_at: Some(NOW - 60),
            checked_at: NOW,
        };
        let snapshot = Snapshot {
            version: Snapshot::VERSION,
            providers: vec![
                provider(
                    ProviderId::Claude,
                    vec![Window::new("5h", "5ч", 34.0, None), Window::new("7d", "7д", 84.0, Some(NOW + 3_600))],
                    vec![],
                ),
                provider(
                    ProviderId::ChatGpt,
                    vec![Window::new("5h", "5ч", 0.0, None)],
                    vec![Balance::new("credits", "кредиты", 120.0, Unit::Credits, BalanceKind::Remaining)],
                ),
                provider(
                    ProviderId::DeepSeek,
                    vec![],
                    vec![Balance::new("balance:cny", "баланс", 0.0, Unit::Cny, BalanceKind::Remaining)],
                ),
                provider(
                    ProviderId::OpenRouter,
                    vec![],
                    vec![Balance::new("spend", "траты", 3.1, Unit::Usd, BalanceKind::Spent).with_limit(Some(10.0))],
                ),
            ],
        };
        view::segments(&snapshot, &QuotaConfig::default(), NOW)
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), true);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    fn painted(ctx: &egui::Context, rect: Rect, segments: &[Segment], events: Vec<egui::Event>) -> Option<BarAction> {
        let mut seen = None;
        let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), events, ..Default::default() }, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                seen = show(ui, rect, segments);
            });
        });
        seen
    }

    /// From a collapsed sliver to a wide monitor, with and without data: the
    /// line paints (folding as it must) and never panics.
    #[test]
    fn reset_icons_are_smaller_without_shrinking_the_surrounding_text() {
        let font = FontId::proportional(FONT_SIZE);
        let pieces = [Piece { text: " ↺ 2д4ч".into(), style: Style::Hint }];
        let layout = job(&pieces, false, &font);
        assert_eq!(layout.text, " ↺ 2д4ч");
        let icon = layout.sections.iter().find(|section| &layout.text[section.byte_range.clone()] == "↺").unwrap();
        assert!(icon.format.font_id.size < FONT_SIZE);
        for section in layout.sections.iter().filter(|section| section.byte_range != icon.byte_range) {
            assert_eq!(section.format.font_id.size, FONT_SIZE);
        }
    }

    #[test]
    fn it_paints_at_every_width() {
        let ctx = context();
        let segments = segments();
        for width in [0.0_f32, 1.0, 40.0, 200.0, 600.0, 900.0, 2000.0] {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, HEIGHT));
            assert_eq!(painted(&ctx, rect, &segments, vec![]), None);
            assert_eq!(painted(&ctx, rect, &[], vec![]), None);
        }
    }

    /// The refresh glyph at the right edge asks for a cycle; a click on a
    /// segment does nothing.
    #[test]
    fn the_right_edge_refreshes() {
        let ctx = context();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, HEIGHT));
        let segments = segments();
        let click = |pos, pressed| {
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            }]
        };
        for (x, expected) in [(900.0 - REFRESH_WIDTH / 2.0, Some(BarAction::Refresh)), (40.0, None)] {
            let pos = Pos2::new(x, HEIGHT / 2.0);
            let _ = painted(&ctx, rect, &segments, vec![]);
            let mut seen = None;
            // egui answers `clicked()` after a move, a press frame and a release frame.
            for events in [vec![egui::Event::PointerMoved(pos)], click(pos, true), click(pos, false)] {
                seen = painted(&ctx, rect, &segments, events);
            }
            assert_eq!(seen, expected, "x = {x}");
        }
    }

    /// The painted text must stop short of the refresh zone at every width: a
    /// run that reaches it looks glued to the glyph, and the fold arithmetic in
    /// `view::layout` budgets in separators the painter never draws.
    #[test]
    fn no_run_reaches_the_refresh_zone() {
        let ctx = context();
        let segments = segments();
        let font = FontId::proportional(FONT_SIZE);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(2000.0, HEIGHT));
        let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
            let painter = ui.painter_at(rect);
            let measure = |text: &str| painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x;
            for width in 200..2000 {
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, HEIGHT));
                let budget = rect.width() - REFRESH_WIDTH - 2.0 * PAD;
                let layout = view::layout(&segments, budget, GAP, true, measure);
                // Walk the paint loop of `show`: every shown segment plus its
                // trailing gap, then the fold when there is one.
                let mut x = PAD;
                for segment in segments.iter().take(layout.shown) {
                    x += view::pieces(segment, layout.detail, true).iter().map(|p| measure(&p.text)).sum::<f32>() + GAP;
                }
                let drawn = match layout.hidden {
                    0 => x - GAP, // the gap after the last segment is not painted
                    hidden => x + measure(&view::more_text(hidden)),
                };
                let limit = rect.width() - REFRESH_WIDTH - PAD;
                assert!(drawn <= limit, "width {width}: text ends at {drawn:.1}, limit {limit:.1}");
            }
        });
    }

    /// Review Focus 4: an empty prepaid balance stays red even in a dimmed segment.
    #[test]
    fn exhausted_balances_stay_red() {
        let red = theme::colors().status_red;
        assert_eq!(color(Style::Value { pct: None, exhausted: true }, true), red);
        assert_eq!(color(Style::Value { pct: None, exhausted: true }, false), red);
        assert_eq!(color(Style::Value { pct: Some(90.0), exhausted: false }, true), theme::colors().dim);
        assert_eq!(color(Style::Value { pct: Some(90.0), exhausted: false }, false), theme::threshold_color(90.0));
    }
}
