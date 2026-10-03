//! Terminal grid -> egui shapes. `snapshot` copies what is visible while the
//! terminal lock is held (keep it short); `paint` draws without the lock.

use std::collections::HashMap;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::text::LayoutJob;
use egui::{Color32, Painter, Pos2, Rect, Stroke, TextFormat, Vec2};

use crate::fonts::TermFonts;
use crate::term::style::{bg_spans, cell_style, text_runs, Palette, RenderCell, Underline};

pub const SELECTION: Color32 = Color32::from_rgba_premultiplied(77, 77, 77, 77);
pub const MATCH: Color32 = Color32::from_rgba_premultiplied(89, 53, 11, 89);
pub const MATCH_CURRENT: Color32 = Color32::from_rgba_premultiplied(177, 106, 22, 178);

/// Size of one cell in points, snapped to whole physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
}

pub fn snap_to_pixels(points: f32, pixels_per_point: f32) -> f32 {
    (points * pixels_per_point).round().max(1.0) / pixels_per_point
}

pub fn cell_metrics(ctx: &egui::Context, fonts: &TermFonts) -> CellMetrics {
    let ppp = ctx.pixels_per_point();
    let (advance, row) = ctx.fonts(|f| (f.glyph_width(&fonts.regular, 'M'), f.row_height(&fonts.regular)));
    CellMetrics { width: snap_to_pixels(advance, ppp), height: snap_to_pixels(row, ppp) }
}

/// Remembers which non-ASCII characters the primary font can draw.
#[derive(Default)]
pub struct GlyphCache {
    known: HashMap<char, bool>,
}

impl GlyphCache {
    pub fn in_primary(&mut self, c: char, has_glyph: &mut dyn FnMut(char) -> bool) -> bool {
        c.is_ascii() || *self.known.entry(c).or_insert_with(|| has_glyph(c))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorDraw {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub ch: char,
    pub wide: bool,
}

/// Everything `paint` needs, detached from the terminal.
pub struct Frame {
    pub rows: Vec<Vec<RenderCell>>,
    pub columns: usize,
    pub lines: usize,
    pub cursor: Option<CursorDraw>,
    /// (row, first column, end column exclusive)
    pub selection: Vec<(usize, usize, usize)>,
    pub display_offset: usize,
    pub history_size: usize,
    pub default_bg: Color32,
}

pub fn snapshot<L: EventListener>(
    term: &Term<L>,
    palette: &Palette,
    glyphs: &mut GlyphCache,
    has_glyph: &mut dyn FnMut(char) -> bool,
) -> Frame {
    let lines = term.screen_lines();
    let columns = term.columns();
    let history_size = term.grid().history_size();
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let colors = content.colors;
    let mut rows: Vec<Vec<RenderCell>> = (0..lines).map(|_| Vec::with_capacity(columns)).collect();
    for indexed in content.display_iter {
        let row = indexed.point.line.0 + offset;
        if row < 0 || row as usize >= lines {
            continue;
        }
        let cell = indexed.cell;
        let flags = cell.flags;
        rows[row as usize].push(RenderCell {
            ch: cell.c,
            // An empty slice means "extra data" (a hyperlink, a custom underline
            // colour), not combining marks: treating it as one would make every
            // such cell its own standalone text run.
            combining: cell
                .zerowidth()
                .filter(|marks| !marks.is_empty())
                .map(|marks| marks.iter().collect::<String>().into_boxed_str()),
            style: cell_style(cell.fg, cell.bg, flags, colors, palette),
            wide: flags.contains(Flags::WIDE_CHAR),
            spacer: flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            in_primary_font: glyphs.in_primary(cell.c, has_glyph),
            hyperlink: cell.hyperlink().map(|h| h.uri().to_owned().into_boxed_str()),
        });
    }

    let mut selection = Vec::new();
    if let Some(range) = content.selection {
        let first = (range.start.line.0 + offset).max(0);
        let last = (range.end.line.0 + offset).min(lines as i32 - 1);
        for row in first..=last {
            let line = row - offset;
            let (a, b) = if range.is_block {
                (range.start.column.0, range.end.column.0 + 1)
            } else {
                let a = if line == range.start.line.0 { range.start.column.0 } else { 0 };
                let b = if line == range.end.line.0 { range.end.column.0 + 1 } else { columns };
                (a, b)
            };
            if a < b {
                selection.push((row as usize, a, b.min(columns)));
            }
        }
    }

    let cursor = {
        let c = content.cursor;
        let row = c.point.line.0 + offset;
        let col = c.point.column.0;
        (c.shape != CursorShape::Hidden && row >= 0 && (row as usize) < lines && col < columns).then(|| {
            let cell = rows[row as usize].get(col);
            CursorDraw {
                row: row as usize,
                col,
                shape: c.shape,
                ch: cell.map(|c| c.ch).unwrap_or(' '),
                wide: cell.is_some_and(|c| c.wide),
            }
        })
    };

    Frame {
        rows,
        columns,
        lines,
        cursor,
        selection,
        display_offset: content.display_offset,
        history_size,
        default_bg: palette.background,
    }
}

/// A search match on screen: (row, first column, end column exclusive, current).
pub type Highlight = (usize, usize, usize, bool);

pub struct PaintOptions<'a> {
    pub metrics: CellMetrics,
    pub fonts: &'a TermFonts,
    pub palette: &'a Palette,
    pub focused: bool,
    /// False during the "off" half of a blink.
    pub cursor_on: bool,
    pub highlights: &'a [Highlight],
}

pub fn paint(painter: &Painter, origin: Pos2, frame: &Frame, opt: &PaintOptions) {
    let (cw, ch) = (opt.metrics.width, opt.metrics.height);
    let cell_rect = |row: usize, col: usize, cells: usize| {
        Rect::from_min_size(origin + Vec2::new(col as f32 * cw, row as f32 * ch), Vec2::new(cw * cells as f32, ch))
    };

    for (r, row) in frame.rows.iter().enumerate() {
        for (col, len, color) in bg_spans(row, frame.default_bg) {
            painter.rect_filled(cell_rect(r, col, len), 0.0, color);
        }
    }
    for &(r, a, b) in &frame.selection {
        painter.rect_filled(cell_rect(r, a, b - a), 0.0, SELECTION);
    }
    for &(r, a, b, current) in opt.highlights {
        painter.rect_filled(cell_rect(r, a, b.saturating_sub(a)), 0.0, if current { MATCH_CURRENT } else { MATCH });
    }

    let advance = |bold: bool, italic: bool| painter.ctx().fonts(|f| f.glyph_width(opt.fonts.for_style(bold, italic), 'M'));
    for (r, row) in frame.rows.iter().enumerate() {
        for run in text_runs(row) {
            let s = run.style;
            let font = opt.fonts.for_style(s.bold, s.italic).clone();
            let span = cell_rect(r, run.col, run.cells);
            if run.standalone {
                let galley = painter.layout_no_wrap(run.text.clone(), font, s.fg);
                let pos = Pos2::new(
                    span.min.x + ((span.width() - galley.size().x) / 2.0).max(0.0),
                    span.min.y + (ch - galley.size().y) / 2.0,
                );
                painter.with_clip_rect(span).galley(pos, galley, s.fg);
            } else {
                let format = TextFormat {
                    font_id: font,
                    color: s.fg,
                    extra_letter_spacing: cw - advance(s.bold, s.italic),
                    strikethrough: if s.strike { Stroke::new(1.0, s.fg) } else { Stroke::NONE },
                    ..Default::default()
                };
                let galley = painter.layout_job(LayoutJob::single_section(run.text.clone(), format));
                painter.galley(span.min, galley, s.fg);
            }
            paint_underline(painter, span, s.underline, s.fg);
        }
    }

    if let Some(c) = frame.cursor {
        let rect = cell_rect(c.row, c.col, if c.wide { 2 } else { 1 });
        let color = opt.palette.cursor;
        let shape = if opt.focused { c.shape } else { CursorShape::HollowBlock };
        match shape {
            _ if !opt.cursor_on && opt.focused => {}
            CursorShape::Block => {
                painter.rect_filled(rect, 0.0, color);
                if c.ch != ' ' {
                    let font = opt.fonts.regular.clone();
                    let galley = painter.layout_no_wrap(c.ch.to_string(), font, frame.default_bg);
                    painter.galley(rect.min, galley, frame.default_bg);
                }
            }
            CursorShape::Beam => {
                painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, ch)), 0.0, color);
            }
            CursorShape::Underline => {
                painter.rect_filled(Rect::from_min_size(Pos2::new(rect.min.x, rect.max.y - 2.0), Vec2::new(rect.width(), 2.0)), 0.0, color);
            }
            CursorShape::HollowBlock => {
                painter.rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(1.0, color));
            }
            CursorShape::Hidden => {}
        }
    }
}

fn paint_underline(painter: &Painter, span: Rect, kind: Underline, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let y = span.max.y - 2.0;
    let line = |y: f32| {
        painter.line_segment([Pos2::new(span.min.x, y), Pos2::new(span.max.x, y)], stroke);
    };
    match kind {
        Underline::None => {}
        Underline::Single => line(y),
        Underline::Double => {
            line(y);
            line(y - 2.0);
        }
        Underline::Curly => {
            let mut points = Vec::new();
            let mut x = span.min.x;
            let mut up = true;
            while x <= span.max.x {
                points.push(Pos2::new(x, if up { y - 1.5 } else { y + 0.5 }));
                x += 2.0;
                up = !up;
            }
            painter.add(egui::Shape::line(points, stroke));
        }
        Underline::Dotted | Underline::Dashed => {
            let (on, off) = if kind == Underline::Dotted { (1.0, 2.0) } else { (4.0, 3.0) };
            let mut x = span.min.x;
            while x < span.max.x {
                let end = (x + on).min(span.max.x);
                painter.line_segment([Pos2::new(x, y), Pos2::new(end, y)], stroke);
                x += on + off;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_to_physical_pixels() {
        assert_eq!(snap_to_pixels(8.79, 1.0), 9.0);
        assert!((snap_to_pixels(8.79, 1.25) - 8.8).abs() < 1e-4, "10.99 px rounds to 11 px = 8.8 pt");
        assert_eq!(snap_to_pixels(0.1, 1.0), 1.0, "never zero");
    }

    #[test]
    fn glyph_cache_asks_once_and_trusts_ascii() {
        let mut cache = GlyphCache::default();
        let mut calls = 0;
        let mut has = |_c: char| {
            calls += 1;
            false
        };
        assert!(cache.in_primary('a', &mut has));
        assert!(!cache.in_primary('↺', &mut has));
        assert!(!cache.in_primary('↺', &mut has));
        assert_eq!(calls, 1);
    }
}
