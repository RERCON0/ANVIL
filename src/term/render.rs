//! Terminal grid -> egui shapes. `snapshot` copies what is visible while the
//! terminal lock is held (keep it short); `paint` draws without the lock.

use std::collections::HashMap;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::text::LayoutJob;
use egui::{Color32, FontId, Painter, Pos2, Rect, Stroke, TextFormat, Vec2};

use crate::fonts::TermFonts;
use crate::term::glyphs::{self, Face};
use crate::term::style::{bg_spans, cell_style, text_runs, Palette, RenderCell, Underline};

pub const SELECTION: Color32 = Color32::from_rgba_premultiplied(77, 77, 77, 77);
pub const MATCH: Color32 = Color32::from_rgba_premultiplied(89, 53, 11, 89);
pub const MATCH_CURRENT: Color32 = Color32::from_rgba_premultiplied(177, 106, 22, 178);

/// Size of one cell in points, snapped to whole physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
    /// Downward shift applied to a row's text so the font's own line box — the
    /// box its block and box-drawing glyphs fill, which is what a cell means to
    /// a terminal — lands on the cell. epaint places the baseline at the font's
    /// typographic ascent, which for faces whose line box is taller (Consolas)
    /// leaves every row of text hanging under the top edge of its background.
    pub offset_y: f32,
    /// From the top of the cell to the baseline of its text, in whole physical
    /// pixels: where epaint puts the baseline, moved down by `offset_y`. The
    /// hinted glyphs sit on it, so they land where epaint's would.
    pub baseline: f32,
}

pub fn snap_to_pixels(points: f32, pixels_per_point: f32) -> f32 {
    (points * pixels_per_point).round().max(1.0) / pixels_per_point
}

pub fn cell_metrics(ctx: &egui::Context, fonts: &TermFonts) -> CellMetrics {
    let ppp = ctx.pixels_per_point();
    let (advance, row, line, ascent) = ctx.fonts(|f| {
        // The full block is what a terminal cell means: programs tile regions
        // with it, so the face draws it exactly as tall as its line box. The
        // heavy vertical is no reference — Cascadia draws it a third taller on
        // purpose, so borders overlap — and epaint exposes no font table.
        let line = f.layout_no_wrap("\u{2588}".to_owned(), fonts.primary.clone(), Color32::WHITE);
        // epaint's baseline, already snapped to a physical pixel.
        let ascent = line.rows.first().and_then(|r| r.glyphs.first()).map_or(0.0, |g| g.pos.y);
        (f.glyph_width(&fonts.regular, 'M'), f.row_height(&fonts.regular), line.mesh_bounds, ascent)
    });
    // epaint pads every glyph in its atlas by one physical pixel per side.
    let ink_top = line.min.y + 1.0 / ppp;
    // Only a face whose line box agrees with the row height is a terminal face;
    // a substitute box for a missing glyph (or a decorative block) keeps the
    // plain metrics rather than shifting text on a guess.
    let aligned = line.is_finite() && (line.height() - row).abs() <= row * 0.15;
    let round = |points: f32| (points * ppp).round() / ppp;
    let offset_y = if aligned { round(-ink_top) } else { 0.0 };
    CellMetrics { width: snap_to_pixels(advance, ppp), height: snap_to_pixels(row, ppp), offset_y, baseline: offset_y + ascent }
}

/// Longest OSC 8 target treated as a link. Real URLs are far shorter; a
/// hostile one of megabytes is ignored rather than carried into every frame.
pub const MAX_LINK_URI: usize = 2048;

/// `link` when its target is short enough to act on.
pub fn usable_link(link: alacritty_terminal::term::cell::Hyperlink) -> Option<alacritty_terminal::term::cell::Hyperlink> {
    (link.uri().len() <= MAX_LINK_URI).then_some(link)
}

/// Remembers which non-ASCII characters the primary font can draw.
#[derive(Default)]
pub struct GlyphCache {
    known: HashMap<char, bool>,
}

/// True for the Block Elements range (U+2580..=U+259F), which is drawn as
/// cell-sized geometry instead of a font glyph.
pub fn is_block_element(ch: char) -> bool {
    ('\u{2580}'..='\u{259F}').contains(&ch)
}

/// `v` points moved onto the nearest physical pixel boundary.
pub fn snap(v: f32, pixels_per_point: f32) -> f32 {
    (v * pixels_per_point).round() / pixels_per_point
}

/// Solid cell fills (backgrounds, selection, search matches, block elements)
/// collected into one mesh. A mesh is drawn as is, while `rect_filled` gets an
/// anti-aliased fringe: two fringed rects meeting at a cell edge each cover
/// that pixel only partly, which left a hairline through every bar and through
/// block-element art. Edges are snapped to physical pixels, so neighbouring
/// fills meet exactly instead of overlapping or leaving a gap.
pub struct CellFills {
    mesh: egui::Mesh,
    pixels_per_point: f32,
}

impl CellFills {
    pub fn new(pixels_per_point: f32) -> CellFills {
        CellFills { mesh: egui::Mesh::default(), pixels_per_point }
    }

    pub fn rect(&mut self, rect: Rect, color: Color32) {
        let ppp = self.pixels_per_point;
        let rect = Rect::from_min_max(
            Pos2::new(snap(rect.min.x, ppp), snap(rect.min.y, ppp)),
            Pos2::new(snap(rect.max.x, ppp), snap(rect.max.y, ppp)),
        );
        if rect.width() > 0.0 && rect.height() > 0.0 {
            self.mesh.add_colored_rect(rect, color);
        }
    }

    pub fn into_shape(self) -> egui::Shape {
        egui::Shape::mesh(self.mesh)
    }
}

/// Cell-sized fills for the block elements: the font's own glyph sits inside
/// its em box, so rows and columns of them leave seams and ASCII art (the
/// Claude Code mascot, meters, bars) falls apart instead of forming a picture.
/// False for characters that are not block elements.
pub fn block_fills(ch: char, rect: Rect, color: Color32, fills: &mut CellFills) -> bool {
    let shade = match ch {
        '\u{2591}' => Some(0.25), // ░
        '\u{2592}' => Some(0.5),  // ▒
        '\u{2593}' => Some(0.75), // ▓
        _ => None,
    };
    if let Some(alpha) = shade {
        fills.rect(rect, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (alpha * 255.0) as u8));
        return true;
    }
    let mut fill = |x: f32, y: f32, w: f32, h: f32| {
        fills.rect(Rect::from_min_size(rect.min + Vec2::new(x, y), Vec2::new(w, h)), color);
    };
    let (w, h) = (rect.width(), rect.height());
    let (hx, hy) = (w / 2.0, h / 2.0);
    let (ex, ey) = (w / 8.0, h / 8.0);
    match ch {
        '\u{2580}' => fill(0.0, 0.0, w, hy),                       // ▀
        '\u{2581}' => fill(0.0, h - ey, w, ey),                    // ▁
        '\u{2582}' => fill(0.0, h - 2.0 * ey, w, 2.0 * ey),        // ▂
        '\u{2583}' => fill(0.0, h - 3.0 * ey, w, 3.0 * ey),        // ▃
        '\u{2584}' => fill(0.0, hy, w, hy),                        // ▄
        '\u{2585}' => fill(0.0, h - 5.0 * ey, w, 5.0 * ey),        // ▅
        '\u{2586}' => fill(0.0, h - 6.0 * ey, w, 6.0 * ey),        // ▆
        '\u{2587}' => fill(0.0, h - 7.0 * ey, w, 7.0 * ey),        // ▇
        '\u{2588}' => fill(0.0, 0.0, w, h),                        // █
        '\u{2589}' => fill(0.0, 0.0, 7.0 * ex, h),                 // ▉
        '\u{258A}' => fill(0.0, 0.0, 6.0 * ex, h),                 // ▊
        '\u{258B}' => fill(0.0, 0.0, 5.0 * ex, h),                 // ▋
        '\u{258C}' => fill(0.0, 0.0, hx, h),                       // ▌
        '\u{258D}' => fill(0.0, 0.0, 3.0 * ex, h),                 // ▍
        '\u{258E}' => fill(0.0, 0.0, 2.0 * ex, h),                 // ▎
        '\u{258F}' => fill(0.0, 0.0, ex, h),                       // ▏
        '\u{2590}' => fill(hx, 0.0, hx, h),                        // ▐
        '\u{2594}' => fill(0.0, 0.0, w, ey),                       // ▔
        '\u{2595}' => fill(w - ex, 0.0, ex, h),                    // ▕
        '\u{2596}' => fill(0.0, hy, hx, hy),                       // ▖
        '\u{2597}' => fill(hx, hy, hx, hy),                        // ▗
        '\u{2598}' => fill(0.0, 0.0, hx, hy),                      // ▘
        '\u{2599}' => { fill(0.0, 0.0, hx, hy); fill(0.0, hy, w, hy) } // ▙
        '\u{259A}' => { fill(0.0, 0.0, hx, hy); fill(hx, hy, hx, hy) } // ▚
        '\u{259B}' => { fill(0.0, 0.0, w, hy); fill(0.0, hy, hx, hy) } // ▛
        '\u{259C}' => { fill(0.0, 0.0, w, hy); fill(hx, hy, hx, hy) }  // ▜
        '\u{259D}' => fill(hx, 0.0, hx, hy),                       // ▝
        '\u{259E}' => { fill(hx, 0.0, hx, hy); fill(0.0, hy, hx, hy) } // ▞
        '\u{259F}' => { fill(hx, 0.0, hx, hy); fill(0.0, hy, w, hy) }  // ▟
        _ => return false,
    }
    true
}

/// A smaller `font` for a standalone glyph whose ink is `ink` points large
/// and has to fit the `room` of its cells, or None when it already fits.
/// Fallback faces are not monospaced to the terminal font: Segoe UI Symbol's
/// ↺ and ⏵ or the circles of Claude Code's spinner are wider than a Consolas
/// cell, and the cell clip used to cut them in half. The ink, not the advance,
/// is what has to fit: those faces pad their symbols with wide side bearings,
/// and fitting the advance shrank ⏵ to a speck.
pub fn fit_font(font: &FontId, ink: Vec2, room: Vec2) -> Option<FontId> {
    if !(ink.x > 0.0 && ink.y > 0.0 && ink.x.is_finite() && ink.y.is_finite()) {
        return None;
    }
    let scale = (room.x / ink.x).min(room.y / ink.y);
    (ink.x > room.x + 0.5 || ink.y > room.y + 0.5).then(|| FontId::new(font.size * scale, font.family.clone()))
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
            hyperlink: cell.hyperlink().and_then(usable_link),
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
    // The font's line box sits this far below epaint's ascent: shift every
    // glyph down so text lines up with the cell its background fills.
    let dy = opt.metrics.offset_y;
    let ppp = painter.ctx().pixels_per_point();
    // Cells come from grid lines snapped to physical pixels: the edge two
    // neighbours share is computed once, so their fills meet exactly.
    let cell_rect = |row: usize, col: usize, cells: usize| {
        Rect::from_min_max(
            Pos2::new(snap(origin.x + col as f32 * cw, ppp), snap(origin.y + row as f32 * ch, ppp)),
            Pos2::new(snap(origin.x + (col + cells) as f32 * cw, ppp), snap(origin.y + (row + 1) as f32 * ch, ppp)),
        )
    };

    let mut fills = CellFills::new(ppp);
    for (r, row) in frame.rows.iter().enumerate() {
        for (col, len, color) in bg_spans(row, frame.default_bg) {
            fills.rect(cell_rect(r, col, len), color);
        }
    }
    for &(r, a, b) in &frame.selection {
        fills.rect(cell_rect(r, a, b - a), SELECTION);
    }
    for &(r, a, b, current) in opt.highlights {
        fills.rect(cell_rect(r, a, b.saturating_sub(a)), if current { MATCH_CURRENT } else { MATCH });
    }
    painter.add(fills.into_shape());
    let mut blocks = CellFills::new(ppp);
    // Grid text is drawn hinted from the DirectWrite atlas, epaint's glyphs
    // only stand in for what the atlas cannot hold.
    let hinted = glyphs::shared(painter.ctx());
    let mut hinted = hinted.as_deref().and_then(|glyphs| glyphs.lock().ok());
    let mut hinted_mesh = egui::Mesh::default();
    let pixel = |points: f32| (points * ppp).round() as i32;

    let advance = |bold: bool, italic: bool| painter.ctx().fonts(|f| f.glyph_width(opt.fonts.for_style(bold, italic), 'M'));
    let mut primary_ink_center = None;
    for (r, row) in frame.rows.iter().enumerate() {
        for run in text_runs(row) {
            let s = run.style;
            let font = opt.fonts.for_style(s.bold, s.italic).clone();
            let span = cell_rect(r, run.col, run.cells);
            if run.standalone {
                let mut chars = run.text.chars();
                let single = chars.next().filter(|_| chars.next().is_none());
                if single.is_some_and(|ch| block_fills(ch, span, s.fg, &mut blocks)) {
                    // Painted with the other block fills after the text.
                } else {
                    let mut galley = painter.layout_no_wrap(run.text.clone(), font.clone(), s.fg);
                    if let Some(smaller) = fit_font(&font, galley.mesh_bounds.size(), span.size()) {
                        galley = painter.layout_no_wrap(run.text.clone(), smaller, s.fg);
                    }
                    // Fitting a fallback glyph changes its baseline and row
                    // height. Align its ink to the primary font's cap-height
                    // centre, not the (taller) terminal row box. Wide text and
                    // combining marks keep their text baseline.
                    let ink = galley.mesh_bounds;
                    let mut pos = Pos2::new(
                        span.min.x + ((span.width() - galley.size().x) / 2.0).max(0.0),
                        span.min.y + dy + (ch - galley.size().y) / 2.0,
                    );
                    if ink.is_positive() && ink.is_finite() {
                        pos.x = span.center().x - ink.center().x;
                        let cell = &row[run.col];
                        if !cell.in_primary_font && !cell.wide && cell.combining.is_none() {
                            // Measured from the cell's top on the capitals as
                            // they are drawn: hinted when the atlas has them.
                            let center = *primary_ink_center.get_or_insert_with(|| {
                                let primary = &opt.fonts.primary;
                                let hinted_m = hinted.as_mut().and_then(|h| h.run(painter.ctx(), Face::Regular, primary.size * ppp, "M"));
                                match hinted_m.as_deref() {
                                    Some([(_, m)]) => opt.metrics.baseline + (m.offset[1] as f32 + m.size[1] as f32 / 2.0) / ppp,
                                    _ => dy + painter.layout_no_wrap("M".to_owned(), primary.clone(), s.fg).mesh_bounds.center().y,
                                }
                            });
                            pos.y = span.min.y + center - ink.center().y;
                        }
                        if pos.y + ink.min.y < span.min.y || pos.y + ink.max.y > span.max.y {
                            pos.y = span.center().y - ink.center().y;
                        }
                    }
                    painter.with_clip_rect(span).galley(pos, galley, s.fg);
                }
            } else if let Some(run_glyphs) =
                hinted.as_mut().and_then(|h| h.run(painter.ctx(), Face::for_style(s.bold, s.italic), font.size * ppp, &run.text))
            {
                let baseline = pixel(span.min.y + opt.metrics.baseline);
                for (i, glyph) in run_glyphs {
                    let pen = pixel(cell_rect(r, run.col + i, 1).min.x);
                    glyphs::add_quad(&mut hinted_mesh, glyph, [pen, baseline], ppp, s.fg);
                }
                if s.strike {
                    // Where epaint strikes text through: the middle of its row.
                    let y = span.min.y + dy + ch / 2.0;
                    blocks.rect(Rect::from_x_y_ranges(span.x_range(), y - 0.5..=y + 0.5), s.fg);
                }
            } else {
                let format = TextFormat {
                    font_id: font,
                    color: s.fg,
                    extra_letter_spacing: cw - advance(s.bold, s.italic),
                    strikethrough: if s.strike { Stroke::new(1.0, s.fg) } else { Stroke::NONE },
                    ..Default::default()
                };
                let galley = painter.layout_job(LayoutJob::single_section(run.text.clone(), format));
                painter.galley(span.min + Vec2::new(0.0, dy), galley, s.fg);
            }
            paint_underline(painter, span, s.underline, s.fg);
        }
    }
    if let Some(texture) = hinted.as_ref().and_then(|h| h.texture_id()).filter(|_| !hinted_mesh.is_empty()) {
        hinted_mesh.texture_id = texture;
        painter.add(egui::Shape::mesh(hinted_mesh));
    }
    painter.add(blocks.into_shape());

    if let Some(c) = frame.cursor {
        let cell = cell_rect(c.row, c.col, if c.wide { 2 } else { 1 });
        let color = opt.palette.cursor;
        let shape = if opt.focused { c.shape } else { CursorShape::HollowBlock };
        // The cell is the font's line box now, so the block covers exactly what
        // the text occupies.
        let rect = cell;
        match shape {
            _ if !opt.cursor_on && opt.focused => {}
            CursorShape::Block => {
                painter.rect_filled(rect, 0.0, color);
                if c.ch != ' ' {
                    let font = opt.fonts.regular.clone();
                    let text = c.ch.to_string();
                    let cursor_glyphs = hinted.as_mut().and_then(|h| h.run(painter.ctx(), Face::Regular, font.size * ppp, &text));
                    match (cursor_glyphs, hinted.as_ref().and_then(|h| h.texture_id())) {
                        (Some(cursor_glyphs), Some(texture)) => {
                            let mut mesh = egui::Mesh::with_texture(texture);
                            let pen = [pixel(cell.min.x), pixel(cell.min.y + opt.metrics.baseline)];
                            for (_, glyph) in cursor_glyphs {
                                glyphs::add_quad(&mut mesh, glyph, pen, ppp, frame.default_bg);
                            }
                            painter.add(egui::Shape::mesh(mesh));
                        }
                        _ => {
                            let galley = painter.layout_no_wrap(text, font, frame.default_bg);
                            painter.galley(cell.min + Vec2::new(0.0, dy), galley, frame.default_bg);
                        }
                    }
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

    /// The cell is the font's line box, so the capitals have to sit centred in
    /// it. Otherwise every backgrounded row is off — the text of a chip or a
    /// menu row hangs under the top edge of its highlight, which is how the
    /// Consolas look went wrong: epaint places the baseline at the font's
    /// typographic ascent, below the line box the face actually draws.
    #[test]
    fn capitals_sit_centred_in_their_cells() {
        let ppp = 1.25;
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppp);
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run(Default::default(), |_| {});
        let fonts = TermFonts::new(14.0);
        let metrics = cell_metrics(&ctx, &fonts);
        let style = crate::term::style::CellStyle {
            fg: Color32::WHITE,
            bg: Color32::BLACK,
            bold: false,
            italic: false,
            underline: Underline::None,
            strike: false,
        };
        let cell = |ch: char| RenderCell { ch, combining: None, style, wide: false, spacer: false, in_primary_font: true, hyperlink: None };
        let rows = vec![
            "MM".chars().map(cell).collect::<Vec<_>>(),
            "  ".chars().map(cell).collect(),
        ];
        let frame = Frame { rows, columns: 2, lines: 2, cursor: None, selection: Vec::new(), display_offset: 0, history_size: 0, default_bg: Color32::BLACK };
        let origin = Pos2::new(4.0, 4.0);
        let output = ctx.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let palette = Palette::hardcore();
            let opt = PaintOptions { metrics, fonts: &fonts, palette: &palette, focused: true, cursor_on: true, highlights: &[] };
            paint(&painter, origin, &frame, &opt);
        });
        let mut inks = Vec::new();
        for primitive in ctx.tessellate(output.shapes, ppp) {
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else { continue };
            // Hinted glyphs come from their own atlas, epaint's from the font
            // texture, where only glyphs sample anything but the white texel.
            let atlas = mesh.texture_id != egui::TextureId::default();
            let mut i = 0;
            while i + 5 < mesh.indices.len() {
                let verts: Vec<&egui::epaint::Vertex> = (0..6).map(|k| &mesh.vertices[mesh.indices[i + k] as usize]).collect();
                let white = |uv: Pos2| (uv - egui::epaint::WHITE_UV).length() < 1e-6;
                let glyph = atlas || verts.iter().any(|v| !white(v.uv));
                if glyph {
                    let y0 = verts.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min);
                    let y1 = verts.iter().map(|v| v.pos.y).fold(f32::NEG_INFINITY, f32::max);
                    inks.push((y0, y1));
                }
                i += 6;
            }
        }
        assert_eq!(inks.len(), 2, "both capitals were drawn");
        let centre = snap_to_pixels(origin.y, ppp) + metrics.height / 2.0;
        for (y0, y1) in inks {
            let offset = ((y0 + y1) / 2.0 - centre) * ppp;
            assert!(offset.abs() <= 1.5, "the capital's centre is {offset:.2} px off the cell's centre");
        }
    }

    /// Bars and block art (the Claude Code mascot, the context meter) showed a
    /// hairline between every pair of cells: each cell was a separately
    /// anti-aliased rect at a fractional position, so the shared edge pixel
    /// was only partly covered. Cell fills must tessellate to opaque quads on
    /// whole physical pixels.
    #[test]
    fn block_cells_tessellate_without_seams() {
        let ppp = 1.25;
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppp);
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run(Default::default(), |_| {});
        let fonts = TermFonts::new(14.0);
        let metrics = cell_metrics(&ctx, &fonts);
        let style = |bg: Color32| crate::term::style::CellStyle {
            fg: Color32::WHITE,
            bg,
            bold: false,
            italic: false,
            underline: Underline::None,
            strike: false,
        };
        let cell = |ch: char, bg: Color32| RenderCell {
            ch,
            combining: None,
            style: style(bg),
            wide: false,
            spacer: false,
            in_primary_font: true,
            hyperlink: None,
        };
        let blue = Color32::from_rgb(0, 0, 200);
        let rows = vec![
            "██▌▐▓▓▀▄".chars().map(|ch| cell(ch, Color32::BLACK)).collect::<Vec<_>>(),
            "        ".chars().map(|ch| cell(ch, blue)).collect(),
            "        ".chars().map(|ch| cell(ch, blue)).collect(),
        ];
        let frame = Frame {
            rows,
            columns: 8,
            lines: 3,
            cursor: None,
            selection: vec![(1, 0, 8), (2, 0, 8)],
            display_offset: 0,
            history_size: 0,
            default_bg: Color32::BLACK,
        };
        let output = ctx.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let palette = Palette::hardcore();
            let opt = PaintOptions { metrics, fonts: &fonts, palette: &palette, focused: true, cursor_on: true, highlights: &[] };
            paint(&painter, Pos2::new(10.3, 7.7), &frame, &opt);
        });
        let primitives = ctx.tessellate(output.shapes, ppp);
        let mut vertices = 0;
        for primitive in &primitives {
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else { continue };
            for vertex in &mesh.vertices {
                vertices += 1;
                assert_ne!(vertex.color.a(), 0, "an anti-aliasing fringe around a cell fill at {:?}", vertex.pos);
                for v in [vertex.pos.x, vertex.pos.y] {
                    let px = v * ppp;
                    assert!((px - px.round()).abs() < 1e-3, "cell edge off the pixel grid: {v} pt = {px} px");
                }
            }
        }
        assert!(vertices > 0, "the cells were painted");
    }

    #[test]
    fn wide_fallback_glyphs_shrink_to_their_cells() {
        let font = FontId::new(14.0, egui::FontFamily::Name("term".into()));
        let cell = Vec2::new(8.0, 16.0);
        let smaller = fit_font(&font, Vec2::new(12.0, 10.0), cell).expect("12 pt of ink does not fit an 8 pt cell");
        assert!((smaller.size - 14.0 * 8.0 / 12.0).abs() < 1e-4);
        assert_eq!(smaller.family, font.family);
        let short = fit_font(&font, Vec2::new(6.0, 32.0), cell).expect("too tall");
        assert!((short.size - 14.0 * 0.5).abs() < 1e-4, "height limits too");
        assert_eq!(fit_font(&font, Vec2::new(8.0, 16.0), cell), None, "a glyph that fits keeps its size");
        assert_eq!(fit_font(&font, Vec2::new(8.3, 9.0), cell), None, "sub-pixel overhang is not worth a smaller glyph");
        assert_eq!(fit_font(&font, Vec2::splat(f32::NEG_INFINITY), cell), None, "no ink (a space) stays as is");
    }

    /// Paints `ch` as a fallback glyph in one cell: (ink bounds, clip, cell).
    fn paint_fallback_glyph(ch: char) -> (Rect, Rect, CellMetrics) {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), true);
        let _ = ctx.run(Default::default(), |_| {});
        let fonts = TermFonts::new(14.0);
        let cell = cell_metrics(&ctx, &fonts);
        let output = ctx.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let cells = vec![RenderCell {
                ch,
                combining: None,
                style: crate::term::style::CellStyle {
                    fg: Color32::WHITE,
                    bg: Color32::BLACK,
                    bold: false,
                    italic: false,
                    underline: Underline::None,
                    strike: false,
                },
                wide: false,
                spacer: false,
                in_primary_font: ch.is_ascii(),
                hyperlink: None,
            }];
            let frame = Frame {
                rows: vec![cells],
                columns: 1,
                lines: 1,
                cursor: None,
                selection: Vec::new(),
                display_offset: 0,
                history_size: 0,
                default_bg: Color32::BLACK,
            };
            let palette = Palette::hardcore();
            let opt = PaintOptions { metrics: cell, fonts: &fonts, palette: &palette, focused: true, cursor_on: true, highlights: &[] };
            paint(&painter, Pos2::ZERO, &frame, &opt);
        });
        let (bounds, clip) = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((text.visual_bounding_rect(), clipped.clip_rect)),
                // A hinted glyph from the terminal's atlas.
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some((mesh.calc_bounds(), clipped.clip_rect))
                }
                _ => None,
            })
            .expect("the glyph is painted");
        (bounds, clip, cell)
    }

    /// Real fonts: Segoe UI Symbol's ↺, ⏵ and ◯ are wider than a Consolas
    /// cell and were cut in half by the cell clip (the Claude Code status
    /// line, its permission-mode arrows and the agent spinner).
    #[test]
    fn wide_fallback_glyphs_are_drawn_whole() {
        for ch in ['↺', '⏵', '◯'] {
            let (bounds, clip, cell) = paint_fallback_glyph(ch);
            assert!(bounds.width() <= cell.width + 0.5, "{ch}: ink {bounds:?} wider than its cell {cell:?}");
            assert!(clip.contains_rect(bounds.shrink(0.25)), "{ch}: ink {bounds:?} is cut by the clip {clip:?}");
        }
        // Fitted by its ink, not its padded advance: it still fills the cell.
        let (bounds, _, cell) = paint_fallback_glyph('↺');
        assert!(bounds.width() >= cell.width * 0.75, "↺ shrunk to {bounds:?} in a {cell:?} cell");
    }

    #[test]
    fn permission_arrows_align_with_primary_text_instead_of_the_row_box() {
        let (arrow, clip, _) = paint_fallback_glyph('⏵');
        let (text, _, _) = paint_fallback_glyph('M');
        assert!((arrow.center().y - text.center().y).abs() <= 0.5, "permission arrow {arrow:?} sits below primary text {text:?}");
        assert!(clip.contains_rect(arrow.shrink(0.25)), "aligned arrow {arrow:?} is clipped by {clip:?}");
    }

    /// epaint rasterizes without the font's hinting, so a stem between two
    /// pixels came out as two grey columns beside letters whose stems hit the
    /// grid: one row of Consolas looked bold in places and crisp in others.
    /// Grid text and the character under the cursor have to come from the
    /// hinted atlas, every texel on exactly one physical pixel: a fractional
    /// offset or any scaling would resample the hinted glyph.
    #[test]
    fn grid_text_is_drawn_hinted_texel_for_pixel() {
        let ppp = 1.25;
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppp);
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run(Default::default(), |_| {});
        let fonts = TermFonts::new(15.0);
        let metrics = cell_metrics(&ctx, &fonts);
        let style = |bold: bool| crate::term::style::CellStyle {
            fg: Color32::WHITE,
            bg: Color32::BLACK,
            bold,
            italic: false,
            underline: Underline::None,
            strike: false,
        };
        let cell = |ch: char, bold: bool| RenderCell {
            ch,
            combining: None,
            style: style(bold),
            wide: false,
            spacer: false,
            in_primary_font: true,
            hyperlink: None,
        };
        let text = "/backend-build mn Привет";
        let mut row: Vec<RenderCell> = text.chars().map(|ch| cell(ch, false)).collect();
        row.extend("bold".chars().map(|ch| cell(ch, true)));
        let inked = row.iter().filter(|c| c.ch != ' ').count();
        let columns = row.len();
        let cursor = Some(CursorDraw { row: 0, col: 1, shape: CursorShape::Block, ch: 'b', wide: false });
        let frame = Frame { rows: vec![row], columns, lines: 1, cursor, selection: Vec::new(), display_offset: 0, history_size: 0, default_bg: Color32::BLACK };
        let output = ctx.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let palette = Palette::hardcore();
            let opt = PaintOptions { metrics, fonts: &fonts, palette: &palette, focused: true, cursor_on: true, highlights: &[] };
            paint(&painter, Pos2::new(10.3, 7.7), &frame, &opt);
        });
        let atlas = glyphs::shared(&ctx).expect("DirectWrite loads Consolas").lock().unwrap().texture_id().expect("glyphs were stored");
        let mut quads = 0;
        for primitive in ctx.tessellate(output.shapes, ppp) {
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else { continue };
            for quad in mesh.indices.chunks_exact(6) {
                let verts: Vec<&egui::epaint::Vertex> = quad.iter().map(|&i| &mesh.vertices[i as usize]).collect();
                let white = verts.iter().all(|v| (v.uv - egui::epaint::WHITE_UV).length() < 1e-6);
                if mesh.texture_id == egui::TextureId::default() && white {
                    continue; // a fill
                }
                assert_eq!(mesh.texture_id, atlas, "a glyph drawn from epaint's unhinted font texture");
                quads += 1;
                let lo = |f: fn(&egui::epaint::Vertex) -> f32| verts.iter().map(|v| f(v)).fold(f32::INFINITY, f32::min);
                let hi = |f: fn(&egui::epaint::Vertex) -> f32| verts.iter().map(|v| f(v)).fold(f32::NEG_INFINITY, f32::max);
                for v in [lo(|v| v.pos.x), hi(|v| v.pos.x), lo(|v| v.pos.y), hi(|v| v.pos.y)] {
                    let px = v * ppp;
                    assert!((px - px.round()).abs() < 1e-3, "glyph edge off the pixel grid: {v} pt = {px} px");
                }
                let pixels = ((hi(|v| v.pos.x) - lo(|v| v.pos.x)) * ppp, (hi(|v| v.pos.y) - lo(|v| v.pos.y)) * ppp);
                let texels = ((hi(|v| v.uv.x) - lo(|v| v.uv.x)) * glyphs::ATLAS as f32, (hi(|v| v.uv.y) - lo(|v| v.uv.y)) * glyphs::ATLAS as f32);
                assert!((pixels.0 - texels.0).abs() < 1e-2 && (pixels.1 - texels.1).abs() < 1e-2, "{texels:?} texels stretched over {pixels:?} px");
            }
        }
        assert_eq!(quads, inked + 1, "every character, and the one under the cursor, is a hinted glyph");
    }

    /// OSC 8 targets come from any program's output; a multi-megabyte URI on
    /// every cell was copied per cell per frame (tens of GB a frame).
    #[test]
    fn overlong_link_targets_are_ignored() {
        use alacritty_terminal::term::cell::Hyperlink;
        let short = Hyperlink::new(None::<String>, "https://example.com".to_owned());
        assert_eq!(usable_link(short.clone()), Some(short));
        let long = Hyperlink::new(None::<String>, format!("https://example.com/{}", "a".repeat(MAX_LINK_URI)));
        assert_eq!(usable_link(long), None);
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

    #[test]
    fn block_elements_fill_their_cell() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0, 16.0));
        let fills = |ch: char| {
            let mut fills = CellFills::new(1.0);
            block_fills(ch, rect, Color32::WHITE, &mut fills).then(|| fills.into_shape())
        };
        let bounds = |ch: char| fills(ch).expect("block").visual_bounding_rect();
        assert_eq!(bounds('\u{2588}'), rect, "a full block covers the whole cell");
        assert_eq!(bounds('\u{2580}'), Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0, 8.0)), "upper half");
        assert_eq!(bounds('\u{2590}'), Rect::from_min_size(Pos2::new(4.0, 0.0), Vec2::new(4.0, 16.0)), "right half");
        assert_eq!(bounds('\u{259D}'), Rect::from_min_size(Pos2::new(4.0, 0.0), Vec2::new(4.0, 8.0)), "upper right quadrant");
        assert_eq!(bounds('\u{259B}'), Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0, 16.0)), "three quadrants reach every edge");
        assert_eq!(bounds('\u{2593}'), rect, "shades fill the cell");
        assert!(fills('A').is_none(), "letters stay with the font");
        assert!(fills('\u{2500}').is_none(), "box drawing stays with the font");
    }
}
