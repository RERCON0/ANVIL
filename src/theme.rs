//! Window chrome palette and sizes, sampled from the owner's Helm screenshot.

use egui::Color32;

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

pub const TITLEBAR_HEIGHT: f32 = 30.0;
pub const TABBAR_WIDTH: f32 = 195.0;
pub const TAB_ROW_HEIGHT: f32 = 34.0;
pub const CLAUDE_ROW_HEIGHT: f32 = 16.0;
pub const RESIZE_BORDER: f32 = 6.0;
pub const PANE_PADDING: f32 = 6.0;
pub const DIVIDER_WIDTH: f32 = 6.0;
pub const WINDOW_BUTTON_WIDTH: f32 = 46.0;

/// Dark egui visuals matching the chrome. Called once at start.
pub fn apply(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = CHROME_BG;
        v.window_fill = Color32::from_rgb(0x16, 0x16, 0x16);
        v.extreme_bg_color = TAB_ACTIVE_BG;
        v.window_stroke = egui::Stroke::new(1.0, BORDER);
        v.selection.bg_fill = ACCENT.linear_multiply(0.35);
        v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
        v.widgets.noninteractive.fg_stroke.color = TAB_TEXT;
        v.widgets.inactive.fg_stroke.color = TAB_ACTIVE_TEXT;
        v.window_rounding = egui::Rounding::ZERO;
        v.menu_rounding = egui::Rounding::ZERO;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = egui::Rounding::ZERO;
        }
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}
