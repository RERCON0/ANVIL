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
    pub fn dark() -> Palette {
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
    pub fn light() -> Palette {
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

    /// Dark text on a light background: the background is the brighter of
    /// the two, Tabby's test.
    pub fn is_light(&self) -> bool {
        luminance(self.background) > luminance(self.foreground)
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

/// WCAG relative luminance of an sRGB colour.
pub fn luminance(c: Color32) -> f32 {
    static LINEAR: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    let linear = LINEAR.get_or_init(|| {
        std::array::from_fn(|v| {
            let s = v as f32 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    0.2126 * linear[c.r() as usize] + 0.7152 * linear[c.g() as usize] + 0.0722 * linear[c.b() as usize]
}

/// WCAG contrast ratio of two colours, 1 to 21.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `fg` moved until it stands out `ratio`:1 against `bg`, the way xterm.js
/// (Tabby's renderer) enforces its minimum contrast: towards black when it is
/// the darker of the two and towards white otherwise, in 10 % steps, turning
/// the other way when one direction runs out before the ratio is reached.
pub fn ensure_contrast(bg: Color32, fg: Color32, ratio: f32) -> Color32 {
    if contrast_ratio(bg, fg) >= ratio {
        return fg;
    }
    let darker = |fg: Color32| {
        let mut c = fg;
        while contrast_ratio(bg, c) < ratio && c != Color32::BLACK {
            let step = |v: u8| v - (v as f32 * 0.1).ceil() as u8;
            c = Color32::from_rgb(step(c.r()), step(c.g()), step(c.b()));
        }
        c
    };
    let lighter = |fg: Color32| {
        let mut c = fg;
        while contrast_ratio(bg, c) < ratio && c != Color32::WHITE {
            let step = |v: u8| v + ((255 - v) as f32 * 0.1).ceil() as u8;
            c = Color32::from_rgb(step(c.r()), step(c.g()), step(c.b()));
        }
        c
    };
    let toward_black = luminance(fg) < luminance(bg);
    let first = if toward_black { darker(fg) } else { lighter(fg) };
    if contrast_ratio(bg, first) >= ratio {
        return first;
    }
    let second = if toward_black { lighter(fg) } else { darker(fg) };
    if contrast_ratio(bg, first) > contrast_ratio(bg, second) {
        first
    } else {
        second
    }
}

/// Characters xterm.js leaves out of its contrast demands: box drawing, block
/// elements and Powerline separators are shapes that meet a neighbouring
/// background, not text that has to be read against their own.
fn contrast_exempt(ch: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&ch) || ('\u{E0A4}'..='\u{E0D6}').contains(&ch)
}

/// Text stands out at least this much from its cell, as Tabby asks of
/// xterm.js; dim text needs half of it, so it still reads as dim. Colours that
/// already do are drawn exactly as programs ask for them.
pub const MIN_CONTRAST: f32 = 4.0;

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

/// Final colours and decorations of cell `ch` (bold-bright, dim, inverse,
/// hidden, minimum contrast).
pub fn cell_style(ch: char, fg: Color, bg: Color, flags: Flags, colors: &Colors, palette: &Palette) -> CellStyle {
    let bold = flags.contains(Flags::BOLD);
    let mut f = resolve(fg, colors, palette, bold);
    let mut b = resolve(bg, colors, palette, false);
    let dim = flags.contains(Flags::DIM);
    if dim {
        f = blend(f, b);
    }
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut f, &mut b);
    }
    if flags.contains(Flags::HIDDEN) {
        f = b;
    } else if !contrast_exempt(ch) {
        f = ensure_contrast(b, f, if dim { MIN_CONTRAST / 2.0 } else { MIN_CONTRAST });
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
    /// OSC 8 link attached to the cell (shared, not copied per cell).
    pub hyperlink: Option<alacritty_terminal::term::cell::Hyperlink>,
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
    let mut runs = Vec::new();
    text_runs_into(row, &mut runs);
    runs
}

/// The same runs, appended to a buffer the caller keeps. Per row per frame this
/// was one `Vec` and one `String` allocation per run; reusing both is the
/// difference between hundreds of allocations and a handful.
pub fn text_runs_into(row: &[RenderCell], out: &mut Vec<TextRun>) {
    out.clear();
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
        let standalone = cell.wide
            || !cell.in_primary_font
            || cell.combining.is_some()
            || crate::term::render::is_block_element(cell.ch)
            || crate::term::render::is_braille(cell.ch);
        if standalone {
            flush(&mut current, out);
            let mut text = cell.ch.to_string();
            if let Some(extra) = &cell.combining {
                text.push_str(extra);
            }
            if cell.ch != ' ' || cell.combining.is_some() {
                out.push(TextRun {
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
            flush(&mut current, out);
            if cell.ch == ' ' && cell.style.underline == Underline::None && !cell.style.strike {
                continue;
            }
            current = Some(TextRun { col, text: String::new(), style: cell.style, cells: 0, standalone: false });
        }
        let run = current.as_mut().expect("run started");
        run.text.push(cell.ch);
        run.cells += 1;
    }
    flush(&mut current, out);
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
        let p = Palette::dark();
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
        let p = Palette::dark();
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
        let p = Palette::dark();
        let c = Colors::default();
        let fg = Color::Named(NamedColor::Foreground);
        let bg = Color::Named(NamedColor::Background);
        let s = cell_style('x', fg, bg, Flags::INVERSE, &c, &p);
        assert_eq!((s.fg, s.bg), (p.background, p.foreground));
        let s = cell_style('x', fg, bg, Flags::HIDDEN, &c, &p);
        assert_eq!(s.fg, s.bg);
        let s = cell_style('x', fg, bg, Flags::DIM, &c, &p);
        assert_eq!(s.fg, blend(p.foreground, p.background));
        let s = cell_style('x', fg, bg, Flags::UNDERCURL | Flags::UNDERLINE | Flags::STRIKEOUT, &c, &p);
        assert_eq!((s.underline, s.strike), (Underline::Curly, true));
    }

    /// The light scheme's yellow and light cyan vanished into its white
    /// background, the dark scheme's colour 0 into its black one. Every colour reaches 4:1
    /// against its cell and dim text 2:1; colours that already read are left
    /// exactly as they are, and box drawing keeps its colour.
    #[test]
    fn every_scheme_keeps_text_legible() {
        let c = Colors::default();
        let bg = Color::Named(NamedColor::Background);
        let (day, dark) = (Palette::light(), Palette::dark());
        assert!(day.is_light() && !dark.is_light());
        assert!(contrast_ratio(day.ansi[3], day.background) < 2.0, "the yellow this is about");
        assert!(contrast_ratio(dark.ansi[0], dark.background) < 1.2, "the black this is about");
        for p in [&day, &dark] {
            for i in 0..16u8 {
                let s = cell_style('x', Color::Indexed(i), bg, Flags::empty(), &c, p);
                assert!(contrast_ratio(s.fg, s.bg) >= MIN_CONTRAST, "colour {i}: {:?} on {:?}", s.fg, s.bg);
                if contrast_ratio(p.ansi[i as usize], p.background) >= MIN_CONTRAST {
                    assert_eq!(s.fg, p.ansi[i as usize], "colour {i} already reads");
                }
                let dim = cell_style('x', Color::Indexed(i), bg, Flags::DIM, &c, p);
                assert!(contrast_ratio(dim.fg, dim.bg) >= MIN_CONTRAST / 2.0, "dim colour {i}");
            }
        }
        let yellow = Color::Indexed(3);
        assert_eq!(cell_style('\u{2500}', yellow, bg, Flags::empty(), &c, &day).fg, day.ansi[3], "box drawing");
    }

    #[test]
    fn contrast_is_raised_only_as_far_as_needed() {
        let white = Color32::from_rgb(0xf7, 0xf7, 0xf7);
        let black = Color32::BLACK;
        assert_eq!(ensure_contrast(white, black, MIN_CONTRAST), black, "already legible");
        let yellow = ensure_contrast(white, Color32::from_rgb(0xfd, 0xed, 0x02), MIN_CONTRAST);
        assert!(contrast_ratio(white, yellow) >= MIN_CONTRAST);
        assert!(yellow.r() > yellow.b() && yellow.g() > yellow.b(), "still a yellow: {yellow:?}");
        let grey = Color32::from_rgb(0x30, 0x30, 0x30);
        let lifted = ensure_contrast(Color32::from_rgb(0x12, 0x12, 0x12), grey, MIN_CONTRAST);
        assert!(luminance(lifted) > luminance(grey), "lightened on a dark cell");
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
    fn block_elements_are_standalone() {
        let runs = text_runs(&row("a█b", style(Color32::WHITE)));
        let shape: Vec<_> = runs.iter().map(|run| (run.col, run.text.as_str(), run.standalone)).collect();
        assert_eq!(shape, vec![(0, "a", false), (1, "█", true), (2, "b", false)]);
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
