//! Colours and text runs for the renderer. Everything here is pure; render.rs
//! only turns the results into egui shapes.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use egui::Color32;

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub foreground: Color32,
    pub background: Color32,
    pub cursor: Color32,
    pub ansi: [Color32; 16],
}

pub fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

impl Palette {
    pub fn from_hex(foreground: &str, background: &str, cursor: &str, colors: &[&str]) -> Result<Palette, String> {
        let p = |s: &str| parse_hex(s).ok_or_else(|| format!("bad colour `{s}`"));
        if colors.len() != 16 {
            return Err(format!("a colour scheme needs 16 colours, got {}", colors.len()));
        }
        let mut ansi = [Color32::BLACK; 16];
        for (slot, c) in ansi.iter_mut().zip(colors) {
            *slot = p(c)?;
        }
        Ok(Palette { foreground: p(foreground)?, background: p(background)?, cursor: p(cursor)?, ansi })
    }

    /// Owner's dark scheme from Helm.
    pub fn hardcore() -> Palette {
        Palette::from_hex(
            "#a0a0a0",
            "#121212",
            "#bbbbbb",
            &[
                "#1b1d1e", "#f92672", "#a6e22e", "#fd971f", "#66d9ef", "#9e6ffe", "#5e7175", "#ccccc6",
                "#505354", "#ff669d", "#beed5f", "#e6db74", "#66d9ef", "#9e6ffe", "#a3babf", "#f8f8f2",
            ],
        )
        .expect("built-in scheme")
    }

    /// Owner's light scheme from Helm.
    pub fn day_3024() -> Palette {
        Palette::from_hex(
            "#4a4543",
            "#f7f7f7",
            "#4a4543",
            &[
                "#090300", "#db2d20", "#01a252", "#fded02", "#01a0e4", "#a16a94", "#b5e4f4", "#a5a2a2",
                "#5c5855", "#e8bbd0", "#3a3432", "#4a4543", "#807d7c", "#d6d5d4", "#cdab53", "#f7f7f7",
            ],
        )
        .expect("built-in scheme")
    }

    /// The xterm 256-colour palette with this scheme's first 16 colours.
    pub fn indexed(&self, i: u8) -> Color32 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            _ => {
                let v = 8 + 10 * (i - 232);
                Color32::from_rgb(v, v, v)
            }
        }
    }
}

fn override_or(colors: &Colors, index: usize, fallback: Color32) -> Color32 {
    match colors[index] {
        Some(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        None => fallback,
    }
}

/// Half way between `c` and `toward`.
pub fn blend(c: Color32, toward: Color32) -> Color32 {
    let mix = |a: u8, b: u8| ((a as u16 + b as u16) / 2) as u8;
    Color32::from_rgb(mix(c.r(), toward.r()), mix(c.g(), toward.g()), mix(c.b(), toward.b()))
}

/// Resolves a cell colour. Bold text in colours 0-7 uses the bright variant
/// (as Helm did); colours the application changed (OSC 4/10/11) win.
pub fn resolve(color: Color, colors: &Colors, palette: &Palette, bold: bool) -> Color32 {
    match color {
        Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => {
            let i = if bold && i < 8 { i + 8 } else { i };
            override_or(colors, i as usize, palette.indexed(i))
        }
        Color::Named(named) => {
            let idx = named as usize;
            let dim_base = NamedColor::DimBlack as usize;
            match named {
                NamedColor::Foreground | NamedColor::BrightForeground => {
                    override_or(colors, NamedColor::Foreground as usize, palette.foreground)
                }
                NamedColor::Background => override_or(colors, idx, palette.background),
                NamedColor::Cursor => override_or(colors, idx, palette.cursor),
                NamedColor::DimForeground => {
                    blend(override_or(colors, NamedColor::Foreground as usize, palette.foreground), palette.background)
                }
                _ if (dim_base..dim_base + 8).contains(&idx) => {
                    let base = idx - dim_base;
                    blend(override_or(colors, base, palette.ansi[base]), palette.background)
                }
                _ => {
                    let idx = if bold && idx < 8 { idx + 8 } else { idx };
                    override_or(colors, idx, palette.ansi[idx])
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellStyle {
    pub fg: Color32,
    pub bg: Color32,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    pub strike: bool,
}

/// Final colours and decorations of a cell (bold-bright, dim, inverse, hidden).
pub fn cell_style(fg: Color, bg: Color, flags: Flags, colors: &Colors, palette: &Palette) -> CellStyle {
    let bold = flags.contains(Flags::BOLD);
    let mut f = resolve(fg, colors, palette, bold);
    let mut b = resolve(bg, colors, palette, false);
    if flags.contains(Flags::DIM) {
        f = blend(f, b);
    }
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut f, &mut b);
    }
    if flags.contains(Flags::HIDDEN) {
        f = b;
    }
    let underline = if flags.contains(Flags::UNDERCURL) {
        Underline::Curly
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Underline::Double
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Underline::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Underline::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        Underline::Single
    } else {
        Underline::None
    };
    CellStyle {
        fg: f,
        bg: b,
        bold,
        italic: flags.contains(Flags::ITALIC),
        underline,
        strike: flags.contains(Flags::STRIKEOUT),
    }
}

/// One cell after colour resolution, ready for layout.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderCell {
    pub ch: char,
    /// Zero-width characters drawn on top of `ch` (combining marks).
    pub combining: Option<Box<str>>,
    pub style: CellStyle,
    /// First half of a double-width character.
    pub wide: bool,
    /// Second half of a double-width character (draws nothing).
    pub spacer: bool,
    /// The primary terminal font has this glyph.
    pub in_primary_font: bool,
    /// OSC 8 link attached to the cell.
    pub hyperlink: Option<Box<str>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub col: usize,
    pub text: String,
    pub style: CellStyle,
    /// Cells covered; 2 for a wide character.
    pub cells: usize,
    /// Draw centred in its cells instead of as part of a grid-aligned run
    /// (wide characters, fallback-font glyphs, combining marks).
    pub standalone: bool,
}

/// Groups a row into grid-aligned text runs. Runs never contain wide
/// characters, fallback glyphs or combining marks; trailing spaces are
/// dropped and space-only runs are skipped.
pub fn text_runs(row: &[RenderCell]) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut current: Option<TextRun> = None;
    let flush = |current: &mut Option<TextRun>, runs: &mut Vec<TextRun>| {
        if let Some(mut run) = current.take() {
            if run.style.underline == Underline::None && !run.style.strike {
                let trimmed = run.text.trim_end_matches(' ').len();
                run.cells -= run.text.len() - trimmed;
                run.text.truncate(trimmed);
            }
            if !run.text.is_empty() {
                runs.push(run);
            }
        }
    };
    for (col, cell) in row.iter().enumerate() {
        if cell.spacer {
            continue;
        }
        let standalone = cell.wide || !cell.in_primary_font || cell.combining.is_some();
        if standalone {
            flush(&mut current, &mut runs);
            let mut text = cell.ch.to_string();
            if let Some(extra) = &cell.combining {
                text.push_str(extra);
            }
            if cell.ch != ' ' || cell.combining.is_some() {
                runs.push(TextRun {
                    col,
                    text,
                    style: cell.style,
                    cells: if cell.wide { 2 } else { 1 },
                    standalone: true,
                });
            }
            continue;
        }
        let continues = matches!(&current, Some(run) if run.style == cell.style && run.col + run.cells == col);
        if !continues {
            flush(&mut current, &mut runs);
            if cell.ch == ' ' && cell.style.underline == Underline::None && !cell.style.strike {
                continue;
            }
            current = Some(TextRun { col, text: String::new(), style: cell.style, cells: 0, standalone: false });
        }
        let run = current.as_mut().expect("run started");
        run.text.push(cell.ch);
        run.cells += 1;
    }
    flush(&mut current, &mut runs);
    runs
}

/// Spans of cells whose background differs from the default: (col, len, colour).
pub fn bg_spans(row: &[RenderCell], default_bg: Color32) -> Vec<(usize, usize, Color32)> {
    let mut spans: Vec<(usize, usize, Color32)> = Vec::new();
    for (col, cell) in row.iter().enumerate() {
        let bg = cell.style.bg;
        if bg == default_bg {
            continue;
        }
        match spans.last_mut() {
            Some((start, len, c)) if *c == bg && *start + *len == col => *len += 1,
            _ => spans.push((col, 1, bg)),
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Rgb;

    fn style(fg: Color32) -> CellStyle {
        CellStyle { fg, bg: Color32::BLACK, bold: false, italic: false, underline: Underline::None, strike: false }
    }

    fn cell(ch: char, s: CellStyle) -> RenderCell {
        RenderCell { ch, combining: None, style: s, wide: false, spacer: false, in_primary_font: true, hyperlink: None }
    }

    fn row(text: &str, s: CellStyle) -> Vec<RenderCell> {
        text.chars().map(|c| cell(c, s)).collect()
    }

    #[test]
    fn xterm_256_palette() {
        let p = Palette::hardcore();
        assert_eq!(p.indexed(1), parse_hex("#f92672").unwrap());
        assert_eq!(p.indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(p.indexed(21), Color32::from_rgb(0, 0, 255));
        assert_eq!(p.indexed(196), Color32::from_rgb(255, 0, 0));
        assert_eq!(p.indexed(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(p.indexed(232), Color32::from_rgb(8, 8, 8));
        assert_eq!(p.indexed(255), Color32::from_rgb(238, 238, 238));
    }

    #[test]
    fn named_indexed_spec_and_overrides() {
        let p = Palette::hardcore();
        let mut colors = Colors::default();
        assert_eq!(resolve(Color::Named(NamedColor::Red), &colors, &p, false), p.ansi[1]);
        assert_eq!(resolve(Color::Named(NamedColor::Red), &colors, &p, true), p.ansi[9]);
        assert_eq!(resolve(Color::Indexed(2), &colors, &p, true), p.ansi[10]);
        assert_eq!(resolve(Color::Indexed(200), &colors, &p, true), p.indexed(200));
        assert_eq!(
            resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &colors, &p, false),
            Color32::from_rgb(1, 2, 3)
        );
        assert_eq!(resolve(Color::Named(NamedColor::Foreground), &colors, &p, false), p.foreground);
        assert_eq!(resolve(Color::Named(NamedColor::Background), &colors, &p, false), p.background);
        colors[NamedColor::Background] = Some(Rgb { r: 9, g: 9, b: 9 });
        colors[1] = Some(Rgb { r: 7, g: 7, b: 7 });
        assert_eq!(resolve(Color::Named(NamedColor::Background), &colors, &p, false), Color32::from_rgb(9, 9, 9));
        assert_eq!(resolve(Color::Named(NamedColor::Red), &colors, &p, false), Color32::from_rgb(7, 7, 7));
        assert_eq!(
            resolve(Color::Named(NamedColor::DimRed), &Colors::default(), &p, false),
            blend(p.ansi[1], p.background)
        );
    }

    #[test]
    fn inverse_hidden_dim() {
        let p = Palette::hardcore();
        let c = Colors::default();
        let fg = Color::Named(NamedColor::Foreground);
        let bg = Color::Named(NamedColor::Background);
        let s = cell_style(fg, bg, Flags::INVERSE, &c, &p);
        assert_eq!((s.fg, s.bg), (p.background, p.foreground));
        let s = cell_style(fg, bg, Flags::HIDDEN, &c, &p);
        assert_eq!(s.fg, s.bg);
        let s = cell_style(fg, bg, Flags::DIM, &c, &p);
        assert_eq!(s.fg, blend(p.foreground, p.background));
        let s = cell_style(fg, bg, Flags::UNDERCURL | Flags::UNDERLINE | Flags::STRIKEOUT, &c, &p);
        assert_eq!((s.underline, s.strike), (Underline::Curly, true));
    }

    #[test]
    fn runs_split_on_style_and_trim_spaces() {
        let a = style(Color32::WHITE);
        let b = style(Color32::RED);
        let mut r = row("ab  cd", a);
        r.extend(row("ef  ", b));
        let runs = text_runs(&r);
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].col, runs[0].text.as_str(), runs[0].cells), (0, "ab  cd", 6));
        assert_eq!((runs[1].col, runs[1].text.as_str(), runs[1].cells), (6, "ef", 2));
        assert!(text_runs(&row("     ", a)).is_empty());
    }

    #[test]
    fn leading_spaces_do_not_start_a_run() {
        let runs = text_runs(&row("   $ ls", style(Color32::WHITE)));
        assert_eq!((runs[0].col, runs[0].text.as_str()), (3, "$ ls"));
    }

    #[test]
    fn wide_fallback_and_combining_are_standalone() {
        let s = style(Color32::WHITE);
        let mut r = row("a", s);
        r.push(RenderCell { ch: '中', wide: true, ..cell('中', s) });
        r.push(RenderCell { spacer: true, ..cell(' ', s) });
        r.push(RenderCell { in_primary_font: false, ..cell('↺', s) });
        r.push(RenderCell { combining: Some("\u{301}".into()), ..cell('e', s) });
        r.extend(row("bc", s));
        let runs = text_runs(&r);
        let shapes: Vec<_> = runs.iter().map(|r| (r.col, r.text.as_str(), r.cells, r.standalone)).collect();
        assert_eq!(
            shapes,
            vec![
                (0, "a", 1, false),
                (1, "中", 2, true),
                (3, "↺", 1, true),
                (4, "e\u{301}", 1, true),
                (5, "bc", 2, false),
            ]
        );
    }

    #[test]
    fn underlined_spaces_are_drawn() {
        let s = CellStyle { underline: Underline::Single, ..style(Color32::WHITE) };
        let runs = text_runs(&row("  ", s));
        assert_eq!((runs[0].col, runs[0].text.as_str(), runs[0].cells), (0, "  ", 2));
        let runs = text_runs(&row("  x", s));
        assert_eq!((runs[0].col, runs[0].text.as_str()), (0, "  x"));
    }

    #[test]
    fn background_spans_merge() {
        let mut r = row("abcd", style(Color32::WHITE));
        r[1].style.bg = Color32::RED;
        r[2].style.bg = Color32::RED;
        r[3].style.bg = Color32::BLUE;
        assert_eq!(bg_spans(&r, Color32::BLACK), vec![(1, 2, Color32::RED), (3, 1, Color32::BLUE)]);
    }
}
