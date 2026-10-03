//! Window chrome and the shared "Terminal Native" control system: square
//! corners, hairline ghost controls, one accent used sparingly. The chrome and
//! tab colours follow the Helm screenshot; the control surfaces use the same
//! palette as SNATCH/BEAT/STRIKE so all four apps look like siblings.

use egui::{Align, Button, Color32, FontId, Layout, Response, RichText, Sense, Stroke, Vec2};

// ---- chrome (Helm sample) -------------------------------------------------

pub const CHROME_BG: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
pub const BORDER: Color32 = Color32::from_rgb(0x13, 0x13, 0x13);
pub const TAB_ACTIVE_BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
pub const TAB_HOVER_BG: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0f);
pub const TAB_ACTIVE_TEXT: Color32 = Color32::from_rgb(0xb6, 0xb6, 0xb6);
pub const TAB_ACTIVE_NUMBER: Color32 = Color32::from_rgb(0x56, 0x56, 0x56);
pub const TAB_TEXT: Color32 = Color32::from_rgb(0x79, 0x79, 0x79);
pub const TAB_NUMBER: Color32 = Color32::from_rgb(0x4c, 0x4c, 0x4c);
pub const TITLE_TEXT: Color32 = Color32::from_rgb(0xa3, 0xa3, 0xa3);
pub const WINDOW_ICON: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x8a);
pub const WINDOW_BUTTON_HOVER: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
pub const CLOSE_HOVER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);
pub const ICON: Color32 = Color32::from_rgb(0x62, 0x62, 0x62);
pub const ICON_HOVER: Color32 = Color32::from_rgb(0xb6, 0xb6, 0xb6);
pub const DIVIDER: Color32 = Color32::from_rgb(0x0c, 0x0c, 0x0c);
pub const DIVIDER_HOVER: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
pub const ACCENT: Color32 = Color32::from_rgb(0x59, 0xd6, 0x8c);
/// Claude status thresholds (the owner's Hardcore scheme).
pub const STATUS_GREEN: Color32 = Color32::from_rgb(0xa6, 0xe2, 0x2e);
pub const STATUS_YELLOW: Color32 = Color32::from_rgb(0xfd, 0x97, 0x1f);
pub const STATUS_RED: Color32 = Color32::from_rgb(0xf9, 0x26, 0x72);

// ---- shared control palette (SNATCH/BEAT/STRIKE) --------------------------

pub const LIFT: Color32 = Color32::from_rgb(0x0e, 0x0e, 0x11);
pub const FIELD: Color32 = Color32::from_rgb(0x11, 0x11, 0x14);
pub const LINE: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x24);
pub const LINE_BRIGHT: Color32 = Color32::from_rgb(0x33, 0x33, 0x3c);
pub const TEXT: Color32 = Color32::from_rgb(0xd6, 0xd6, 0xda);
pub const DIM: Color32 = Color32::from_rgb(0x83, 0x83, 0x8c);
pub const FAINT: Color32 = Color32::from_rgb(0x53, 0x53, 0x5c);

pub const TITLEBAR_HEIGHT: f32 = 30.0;
pub const TABBAR_WIDTH: f32 = 195.0;
pub const TAB_ROW_HEIGHT: f32 = 34.0;
pub const CLAUDE_ROW_HEIGHT: f32 = 16.0;
pub const RESIZE_BORDER: f32 = 6.0;
pub const PANE_PADDING: f32 = 6.0;
pub const DIVIDER_WIDTH: f32 = 6.0;
pub const WINDOW_BUTTON_WIDTH: f32 = 46.0;

pub fn font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("ui".into()))
}

pub fn field_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("ui-tight".into()))
}

pub fn title_font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("ui-title".into()))
}

/// Dark control visuals, written into both egui style buckets by `apply`.
fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.window_rounding = egui::Rounding::ZERO;
    v.menu_rounding = egui::Rounding::ZERO;
    v.window_fill = LIFT;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.panel_fill = CHROME_BG;
    v.extreme_bg_color = FIELD;
    v.faint_bg_color = FIELD;
    v.code_bg_color = FIELD;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = STATUS_YELLOW;
    v.error_fg_color = STATUS_RED;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.widgets.noninteractive.rounding = egui::Rounding::ZERO;
    v.widgets.inactive.rounding = egui::Rounding::ZERO;
    v.widgets.hovered.rounding = egui::Rounding::ZERO;
    v.widgets.active.rounding = egui::Rounding::ZERO;
    v.widgets.open.rounding = egui::Rounding::ZERO;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        // bg_fill backs checkboxes/sliders and stays visible; weak_bg_fill
        // backs plain buttons and being transparent is what makes them ghost.
        w.bg_fill = FIELD;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.weak_bg_fill = CHROME_BG;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, DIM);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, DIM);
    v.widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, LINE_BRIGHT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.weak_bg_fill = FIELD;
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.open.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.open.bg_stroke = Stroke::new(1.0, LINE_BRIGHT);
    v.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    v
}

/// Dark visuals plus the shared typography and rhythm. Called once at start
/// and after every font setup; `all_styles_mut` keeps both egui theme buckets
/// in sync (see SNATCH's comment on the two-bucket trap).
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        // Tight rhythm inside a group; loose spacing between groups is added
        // explicitly with `ui.add_space`.
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.window_margin = egui::Margin::symmetric(10.0, 12.0);
        style.spacing.scroll = egui::style::ScrollStyle { foreground_color: true, ..egui::style::ScrollStyle::solid() };
        let mut text = style.text_styles.clone();
        text.insert(egui::TextStyle::Body, font(13.0));
        text.insert(egui::TextStyle::Button, font(12.5));
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
    ui.label(RichText::new(format!("[ {title} ]")).color(FAINT).font(font(12.0)));
    hairline(ui);
    ui.add_space(2.0);
}

/// 1px rule across the available width.
pub fn hairline(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, LINE));
}

/// `› label` above its control (SNATCH's `label.tag`).
pub fn tag(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(format!("› {text}")).color(DIM).font(font(12.0)));
}

/// Flat `● label` / `○ label` choice with no box.
pub fn choice(ui: &mut egui::Ui, label: &str, selected: bool) -> Response {
    let (dot, dot_color, color) = if selected {
        ("●", ACCENT, ACCENT)
    } else {
        ("○", FAINT, DIM)
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("{dot} "),
        0.0,
        egui::TextFormat { font_id: field_font(11.0), color: dot_color, ..Default::default() },
    );
    job.append(label, 0.0, egui::TextFormat { font_id: field_font(13.0), color, ..Default::default() });
    ui.add(Button::new(job).frame(false))
}

/// `label` left, value right, on one hairline-separated row.
pub fn kv_row(ui: &mut egui::Ui, label: &str, value: &str, value_color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(DIM).font(font(12.5)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).color(value_color).font(field_font(12.5)));
        });
    });
}

/// Border-only button: hairline stroke, no fill (see `visuals`).
pub fn ghost_button(text: impl Into<String>) -> Button<'static> {
    Button::new(RichText::new(text.into()).color(TEXT).font(font(12.5)))
}

/// The primary action: accent text and stroke, transparent fill.
pub fn accent_button(text: impl Into<String>) -> Button<'static> {
    Button::new(RichText::new(text.into()).color(ACCENT).font(font(12.5)))
        .stroke(Stroke::new(1.0, ACCENT))
        .fill(Color32::TRANSPARENT)
}

/// `− [value] +` stepper with ghost buttons; returns true when the value changed.
pub fn stepper_f32(ui: &mut egui::Ui, value: &mut f32, min: f32, max: f32, step: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.add_sized([22.0, 22.0], ghost_button("−")).clicked() {
            *value = (*value - step).max(min);
            changed = true;
        }
        ui.add_sized(
            [52.0, 22.0],
            egui::Label::new(RichText::new(format!("{value:.0}")).color(TEXT).font(field_font(13.0))),
        );
        if ui.add_sized([22.0, 22.0], ghost_button("+")).clicked() {
            *value = (*value + step).min(max);
            changed = true;
        }
    });
    changed
}

/// `− [value] +` for counts.
pub fn stepper(ui: &mut egui::Ui, value: &mut usize, min: usize, max: usize, step: usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.add_sized([22.0, 22.0], ghost_button("−")).clicked() {
            *value = value.saturating_sub(step).max(min);
            changed = true;
        }
        ui.add_sized(
            [64.0, 22.0],
            egui::Label::new(RichText::new(value.to_string()).color(TEXT).font(field_font(13.0))),
        );
        if ui.add_sized([22.0, 22.0], ghost_button("+")).clicked() {
            *value = (*value + step).min(max);
            changed = true;
        }
    });
    changed
}
