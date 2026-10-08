//! Window chrome and the shared "Terminal Native" control system: square
//! corners, hairline ghost controls, one accent used sparingly. The chrome and
//! tab colours follow the Helm screenshot; the control surfaces use the same
//! palette as SNATCH/BEAT/STRIKE so all four apps look like siblings.

use std::sync::RwLock;

use egui::{Button, Color32, FontId, Response, RichText, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};

use crate::term::style::{ensure_contrast, Palette, MIN_CONTRAST};

// ---- colours ----------------------------------------------------------------

/// Colours of the window chrome and the controls. They follow the terminal's
/// colour scheme, as Tabby's interface does: `DARK` is the owner's design
/// (chrome and tabs from the Helm screenshot, control surfaces shared with
/// SNATCH/BEAT/STRIKE), and any other scheme gets the same design laid on its
/// own background and foreground, so a light scheme gets a light window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// Dark text on a light background (`Palette::is_light`).
    pub light: bool,
    pub chrome_bg: Color32,
    pub border: Color32,
    pub tab_active_bg: Color32,
    pub tab_hover_bg: Color32,
    pub tab_active_text: Color32,
    pub tab_active_number: Color32,
    pub tab_text: Color32,
    pub tab_number: Color32,
    pub title_text: Color32,
    pub window_icon: Color32,
    pub window_button_hover: Color32,
    /// Laid over unfocused panes so they recede: darker in a dark scheme, as
    /// in the reference terminal, paler in a light one, as Tabby fades them.
    pub pane_dim: Color32,
    /// Diff bands of the reference diff view: added and removed lines get a
    /// full-width tint behind their text.
    pub diff_add_bg: Color32,
    pub diff_remove_bg: Color32,
    pub diff_hunk: Color32,
    pub close_hover: Color32,
    pub icon: Color32,
    pub icon_hover: Color32,
    pub divider: Color32,
    pub divider_hover: Color32,
    pub accent: Color32,
    /// Claude status thresholds (the owner's Dark scheme).
    pub status_green: Color32,
    pub status_yellow: Color32,
    pub status_red: Color32,
    /// The shared control palette (SNATCH/BEAT/STRIKE).
    pub lift: Color32,
    pub field: Color32,
    pub line: Color32,
    pub line_bright: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl Colors {
    pub const DARK: Colors = Colors {
        light: false,
        chrome_bg: rgb(0x0a, 0x0a, 0x0a),
        border: rgb(0x13, 0x13, 0x13),
        tab_active_bg: rgb(0x12, 0x12, 0x12),
        tab_hover_bg: rgb(0x0f, 0x0f, 0x0f),
        tab_active_text: rgb(0xb6, 0xb6, 0xb6),
        tab_active_number: rgb(0x56, 0x56, 0x56),
        tab_text: rgb(0x79, 0x79, 0x79),
        tab_number: rgb(0x4c, 0x4c, 0x4c),
        title_text: rgb(0xa3, 0xa3, 0xa3),
        window_icon: rgb(0x8a, 0x8a, 0x8a),
        window_button_hover: rgb(0x1f, 0x1f, 0x1f),
        pane_dim: Color32::from_black_alpha(64),
        diff_add_bg: rgb(0x1a, 0x20, 0x0e),
        diff_remove_bg: rgb(0x22, 0x0d, 0x14),
        diff_hunk: rgb(0x66, 0xd9, 0xef),
        close_hover: rgb(0xc4, 0x2b, 0x1c),
        icon: rgb(0x62, 0x62, 0x62),
        icon_hover: rgb(0xb6, 0xb6, 0xb6),
        divider: rgb(0x0c, 0x0c, 0x0c),
        divider_hover: rgb(0x2a, 0x2a, 0x2a),
        accent: rgb(0x59, 0xd6, 0x8c),
        status_green: rgb(0xa6, 0xe2, 0x2e),
        status_yellow: rgb(0xfd, 0x97, 0x1f),
        status_red: rgb(0xf9, 0x26, 0x72),
        lift: rgb(0x0e, 0x0e, 0x11),
        field: rgb(0x11, 0x11, 0x14),
        line: rgb(0x1e, 0x1e, 0x24),
        line_bright: rgb(0x33, 0x33, 0x3c),
        text: rgb(0xd6, 0xd6, 0xda),
        dim: rgb(0x83, 0x83, 0x8c),
        faint: rgb(0x53, 0x53, 0x5c),
    };

    /// The design laid on `scheme`. Each neutral keeps its place on the line
    /// from Dark's background (0) to its foreground (1) — the chrome a
    /// step beyond the background, labels most of the way to the text — now
    /// drawn from this scheme's background to its foreground, and the control
    /// surfaces keep their cool tint. Dark itself maps onto the design
    /// unchanged. On a light scheme the hues are deepened until they read
    /// against the window, and the diff bands become pale washes of the
    /// scheme's own green and red; dark schemes keep the owner's hues.
    pub fn for_scheme(scheme: &Palette) -> Colors {
        let design = Colors::DARK;
        let reference = Palette::dark();
        let light = scheme.is_light();
        let (bg, fg) = (scheme.background, scheme.foreground);
        let from = reference.background.r() as f32;
        let span = reference.foreground.r() as f32 - from;
        let neutral = |c: Color32| {
            let t = (c.r() as f32 - from) / span;
            let at = |b: u8, f: u8| (b as f32 + (f as f32 - b as f32) * t).round().clamp(0.0, 255.0) as u8;
            let (r, g, b) = (at(bg.r(), fg.r()), at(bg.g(), fg.g()), at(bg.b(), fg.b()));
            let tint = c.b().saturating_sub(c.r());
            if light {
                rgb(r.saturating_sub(tint), g.saturating_sub(tint), b)
            } else {
                rgb(r, g, b.saturating_add(tint))
            }
        };
        let chrome_bg = neutral(design.chrome_bg);
        let hue = |c: Color32| if light { ensure_contrast(chrome_bg, c, MIN_CONTRAST) } else { c };
        let wash = |c: Color32, dark: Color32| {
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * 0.15).round() as u8;
            if light {
                rgb(mix(bg.r(), c.r()), mix(bg.g(), c.g()), mix(bg.b(), c.b()))
            } else {
                dark
            }
        };
        Colors {
            light,
            chrome_bg,
            border: neutral(design.border),
            tab_active_bg: neutral(design.tab_active_bg),
            tab_hover_bg: neutral(design.tab_hover_bg),
            tab_active_text: neutral(design.tab_active_text),
            tab_active_number: neutral(design.tab_active_number),
            tab_text: neutral(design.tab_text),
            tab_number: neutral(design.tab_number),
            title_text: neutral(design.title_text),
            window_icon: neutral(design.window_icon),
            window_button_hover: neutral(design.window_button_hover),
            // White at the black veil's strength. Not `from_white_alpha`: that
            // one is linear white, which egui's sRGB blending overshoots.
            pane_dim: if light { Color32::from_rgba_premultiplied(64, 64, 64, 64) } else { design.pane_dim },
            diff_add_bg: wash(scheme.ansi[2], design.diff_add_bg),
            diff_remove_bg: wash(scheme.ansi[1], design.diff_remove_bg),
            diff_hunk: hue(design.diff_hunk),
            close_hover: design.close_hover,
            icon: neutral(design.icon),
            icon_hover: neutral(design.icon_hover),
            divider: neutral(design.divider),
            divider_hover: neutral(design.divider_hover),
            accent: hue(design.accent),
            status_green: hue(design.status_green),
            status_yellow: hue(design.status_yellow),
            status_red: hue(design.status_red),
            lift: neutral(design.lift),
            field: neutral(design.field),
            line: neutral(design.line),
            line_bright: neutral(design.line_bright),
            text: neutral(design.text),
            dim: neutral(design.dim),
            faint: neutral(design.faint),
        }
    }
}

static COLORS: RwLock<Colors> = RwLock::new(Colors::DARK);

/// The colours of the current scheme. One window per process, one scheme.
pub fn colors() -> Colors {
    *COLORS.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Colour of a used share, by the Claude status thresholds: green, yellow
/// from 60 %, red from 85 %.
pub fn threshold_color(pct: f64) -> Color32 {
    if pct >= 85.0 {
        colors().status_red
    } else if pct >= 60.0 {
        colors().status_yellow
    } else {
        colors().status_green
    }
}

/// The tab colours of the reference menu (Tabby's palette), stored by name in
/// the session so a saved colour survives a scheme change and a hand-edited
/// `session.json` stays readable. The values are the reference's: they are
/// chosen to read as a bar on either scheme, so they do not follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabColor {
    Blue,
    Green,
    Orange,
    Purple,
    Red,
    Yellow,
}

impl TabColor {
    pub const ALL: [TabColor; 6] =
        [TabColor::Blue, TabColor::Green, TabColor::Orange, TabColor::Purple, TabColor::Red, TabColor::Yellow];

    pub fn color(self) -> Color32 {
        match self {
            TabColor::Blue => Color32::from_rgb(0x02, 0x75, 0xd8),
            TabColor::Green => Color32::from_rgb(0x5c, 0xb8, 0x5c),
            TabColor::Orange => Color32::from_rgb(0xf0, 0xad, 0x4e),
            TabColor::Purple => Color32::from_rgb(0x61, 0x3d, 0x7c),
            TabColor::Red => Color32::from_rgb(0xd9, 0x53, 0x4f),
            TabColor::Yellow => Color32::from_rgb(0xff, 0xd5, 0x00),
        }
    }

    /// Label of the colour in the tab's menu.
    pub fn label(self) -> &'static str {
        match self {
            TabColor::Blue => crate::strings::TAB_COLOR_BLUE(),
            TabColor::Green => crate::strings::TAB_COLOR_GREEN(),
            TabColor::Orange => crate::strings::TAB_COLOR_ORANGE(),
            TabColor::Purple => crate::strings::TAB_COLOR_PURPLE(),
            TabColor::Red => crate::strings::TAB_COLOR_RED(),
            TabColor::Yellow => crate::strings::TAB_COLOR_YELLOW(),
        }
    }
}

/// Recolours the chrome for `scheme`; `apply` then rebuilds egui's visuals.
pub fn set_scheme(scheme: &Palette) {
    *COLORS.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Colors::for_scheme(scheme);
}

pub const TITLEBAR_HEIGHT: f32 = 30.0;
pub const TABBAR_WIDTH: f32 = 195.0;
pub const TAB_ROW_HEIGHT: f32 = 34.0;
pub const CLAUDE_ROW_HEIGHT: f32 = 16.0;
pub const RESIZE_BORDER: f32 = 6.0;
pub const PANE_PADDING: f32 = 6.0;
pub const DIVIDER_WIDTH: f32 = 6.0;
pub const WINDOW_BUTTON_WIDTH: f32 = 46.0;

pub fn font(size: f32) -> FontId {
    static FAMILY: std::sync::LazyLock<egui::FontFamily> =
        std::sync::LazyLock::new(|| egui::FontFamily::Name("ui".into()));
    FontId::new(size, FAMILY.clone())
}

pub fn field_font(size: f32) -> FontId {
    static FAMILY: std::sync::LazyLock<egui::FontFamily> =
        std::sync::LazyLock::new(|| egui::FontFamily::Name("ui-tight".into()));
    FontId::new(size, FAMILY.clone())
}

pub fn title_font(size: f32) -> FontId {
    static FAMILY: std::sync::LazyLock<egui::FontFamily> =
        std::sync::LazyLock::new(|| egui::FontFamily::Name("ui-title".into()));
    FontId::new(size, FAMILY.clone())
}

/// File-type icon glyphs (Seti).
pub fn icon_font(size: f32) -> FontId {
    static FAMILY: std::sync::LazyLock<egui::FontFamily> =
        std::sync::LazyLock::new(|| egui::FontFamily::Name("icons".into()));
    FontId::new(size, FAMILY.clone())
}

/// Control visuals of the current scheme, written into both egui style
/// buckets by `apply`.
fn visuals() -> egui::Visuals {
    let c = colors();
    let mut v = if c.light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.window_corner_radius = egui::CornerRadius::ZERO;
    v.menu_corner_radius = egui::CornerRadius::ZERO;
    v.window_fill = c.lift;
    v.window_stroke = Stroke::new(1.0_f32, c.line);
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.panel_fill = c.chrome_bg;
    v.extreme_bg_color = c.field;
    v.faint_bg_color = c.field;
    v.code_bg_color = c.field;
    v.hyperlink_color = c.accent;
    v.warn_fg_color = c.status_yellow;
    v.error_fg_color = c.status_red;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), 45);
    v.selection.stroke = Stroke::new(1.0_f32, c.accent);
    v.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.active.corner_radius = egui::CornerRadius::ZERO;
    v.widgets.open.corner_radius = egui::CornerRadius::ZERO;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        // bg_fill backs checkboxes/sliders and stays visible; weak_bg_fill
        // backs plain buttons and being transparent is what makes them ghost.
        w.bg_fill = c.field;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.weak_bg_fill = c.chrome_bg;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, c.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, c.dim);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, c.line);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, c.dim);
    v.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, c.line_bright);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, c.text);
    v.widgets.active.weak_bg_fill = c.field;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, c.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, c.text);
    v.widgets.open.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, c.line_bright);
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, c.text);
    v
}

/// The scheme's visuals plus the shared typography and rhythm. Called once at
/// start and after every scheme change; `all_styles_mut` keeps both egui theme
/// buckets in sync (see SNATCH's comment on the two-bucket trap).
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        // Tight rhythm inside a group; loose spacing between groups is added
        // explicitly with `ui.add_space`.
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.window_margin = egui::Margin::symmetric(10, 12);
        style.spacing.scroll = egui::style::ScrollStyle { foreground_color: true, ..egui::style::ScrollStyle::solid() };
        let mut text = style.text_styles.clone();
        text.insert(egui::TextStyle::Body, font(13.0));
        // Buttons and fields use the un-nudged face, otherwise the +0.15em
        // body baseline pushes the label below the button's centre line.
        text.insert(egui::TextStyle::Button, field_font(12.5));
        text.insert(egui::TextStyle::Small, font(12.0));
        text.insert(egui::TextStyle::Monospace, font(13.0));
        text.insert(egui::TextStyle::Heading, title_font(14.0));
        style.text_styles = text;
    });
}

// ---- shared widgets --------------------------------------------------------

/// `[ SECTION ]` caption with a hairline under it.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(format!("[ {title} ]")).color(colors().faint).font(font(12.0)));
    hairline(ui);
    ui.add_space(2.0);
}

/// 1px rule across the available width.
pub fn hairline(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0_f32, colors().line));
}

/// `› label` above its control (SNATCH's `label.tag`).
pub fn tag(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(format!("› {text}")).color(colors().dim).font(font(12.0)));
}

/// Flat `● label` / `○ label` choice with no box.
pub fn choice(ui: &mut egui::Ui, label: &str, selected: bool) -> Response {
    let (dot, dot_color, color) =
        if selected { ("●", colors().accent, colors().accent) } else { ("○", colors().faint, colors().dim) };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("{dot} "),
        0.0,
        egui::TextFormat { font_id: field_font(11.0), color: dot_color, ..Default::default() },
    );
    job.append(label, 0.0, egui::TextFormat { font_id: field_font(13.0), color, ..Default::default() });
    ui.add(Button::new(job).frame(false))
}

/// Border-only button: hairline stroke, no fill (see `visuals`).
pub fn ghost_button(text: impl Into<String>) -> Button<'static> {
    Button::new(RichText::new(text.into()).color(colors().text).font(field_font(12.5)))
}

/// The primary action: accent text and stroke, transparent fill.
pub fn accent_button(text: impl Into<String>) -> Button<'static> {
    Button::new(RichText::new(text.into()).color(colors().accent).font(field_font(12.5)))
        .stroke(Stroke::new(1.0_f32, colors().accent))
        .fill(Color32::TRANSPARENT)
}

/// Accent button with a fading hover tint and an immediate, stronger pressed tint.
/// Delegating to `accent_button` keeps its sizing, hitbox and disabled semantics.
pub fn animated_accent_button(text: impl Into<String>) -> impl egui::Widget {
    move |ui: &mut egui::Ui| {
        let id = ui.next_auto_id();
        // Like egui's Button, read this pass's interaction before allocating it.
        let response = ui.ctx().read_response(id);
        let enabled = ui.is_enabled();
        let hovered = enabled && response.as_ref().is_some_and(Response::hovered);
        let pressed = enabled && response.as_ref().is_some_and(Response::is_pointer_button_down_on);
        let hover = ui.ctx().animate_bool_with_time(id.with("accent-hover"), hovered, 0.14);
        let opacity = if !enabled {
            0.0
        } else if pressed {
            0.32
        } else {
            0.15 * hover
        };
        ui.add(accent_button(text).fill(colors().accent.gamma_multiply(opacity)))
    }
}

/// `− [value] +` stepper with ghost buttons; returns true when the value changed.
pub fn stepper_f32(ui: &mut egui::Ui, value: &mut f32, min: f32, max: f32, step: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.add_sized([22.0, 22.0], ghost_button("−")).clicked() {
            let next = (*value - step).max(min);
            changed = next != *value;
            *value = next;
        }
        ui.add_sized(
            [52.0, 22.0],
            egui::Label::new(RichText::new(format!("{value:.0}")).color(colors().text).font(field_font(13.0))),
        );
        if ui.add_sized([22.0, 22.0], ghost_button("+")).clicked() {
            let next = (*value + step).min(max);
            changed = next != *value;
            *value = next;
        }
    });
    changed
}

/// `− [value] +` for counts.
pub fn stepper(ui: &mut egui::Ui, value: &mut usize, min: usize, max: usize, step: usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.add_sized([22.0, 22.0], ghost_button("−")).clicked() {
            // At the bound the click changes nothing, and reporting a change
            // would rewrite config.json and sweep apply_config for no reason.
            let next = value.saturating_sub(step).max(min);
            changed = next != *value;
            *value = next;
        }
        ui.add_sized(
            [64.0, 22.0],
            egui::Label::new(RichText::new(value.to_string()).color(colors().text).font(field_font(13.0))),
        );
        if ui.add_sized([22.0, 22.0], ghost_button("+")).clicked() {
            let next = (*value + step).min(max);
            changed = next != *value;
            *value = next;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::style::contrast_ratio;

    /// The chrome follows the scheme, but the owner's dark design is the
    /// scheme it was drawn for: the dark one must come out exactly as designed.
    #[test]
    fn dark_keeps_the_design() {
        assert_eq!(Colors::for_scheme(&Palette::dark()), Colors::DARK);
    }

    /// A light scheme used to leave the window black around a white terminal.
    /// A light one now gets a light window whose labels, accent and status
    /// colours read against it as well as the dark design's do against black.
    #[test]
    fn light_schemes_get_a_light_window() {
        let scheme = Palette::light();
        let c = Colors::for_scheme(&scheme);
        assert!(c.light);
        let surfaces = [("chrome", c.chrome_bg), ("tab", c.tab_active_bg), ("lift", c.lift), ("field", c.field)];
        for (name, surface) in surfaces {
            assert!(contrast_ratio(surface, Color32::WHITE) < 1.2, "{name} {surface:?} is not light");
        }
        let dark = Colors::DARK;
        let readable = |fg: Color32, bg: Color32, design_fg: Color32, design_bg: Color32| {
            contrast_ratio(fg, bg) >= contrast_ratio(design_fg, design_bg).min(MIN_CONTRAST) - 0.05
        };
        for (name, fg, design_fg) in [
            ("text", c.text, dark.text),
            ("dim", c.dim, dark.dim),
            ("tab text", c.tab_text, dark.tab_text),
            ("active tab", c.tab_active_text, dark.tab_active_text),
            ("title", c.title_text, dark.title_text),
        ] {
            assert!(readable(fg, c.chrome_bg, design_fg, dark.chrome_bg), "{name}: {fg:?} on {:?}", c.chrome_bg);
        }
        let hues =
            [("accent", c.accent), ("green", c.status_green), ("yellow", c.status_yellow), ("red", c.status_red)];
        for (name, hue) in hues {
            assert!(contrast_ratio(hue, c.chrome_bg) >= MIN_CONTRAST - 0.01, "{name} {hue:?} fades into the window");
        }
        assert_eq!(c.pane_dim, Color32::from_rgba_premultiplied(64, 64, 64, 64), "unfocused panes pale, not grey");
        assert!(c.diff_add_bg.g() > c.diff_add_bg.r(), "a green wash");
        assert!(c.diff_remove_bg.r() > c.diff_remove_bg.g(), "a red wash");
    }
}
