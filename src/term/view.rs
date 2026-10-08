//! The terminal widget of one pane: sizing, painting, mouse, selection, wheel,
//! search bar and links. Keyboard input does not pass through here (see
//! host/route.rs).

use std::time::{Duration, Instant};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Term, TermMode};
use egui::{CursorIcon, Pos2, Rect, Sense, Vec2};

use crate::config::RightClick;
use crate::fonts::TermFonts;
use crate::hotkeys::Mods;
use crate::strings;
use crate::term::links;
use crate::term::mouse::{self, ClickCounter, MouseAction, MouseButton, MouseModes};
use crate::term::pane::Pane;
use crate::term::render::{
    cell_metrics, paint, snapshot_reusing, CellMetrics, Frame, GlyphCache, Highlight, PaintOptions,
};
use crate::term::search::{search_pattern, RegexCache};
use crate::term::style::Palette;
use crate::theme;

pub const MIN_FONT_SIZE: f32 = 6.0;
pub const MAX_FONT_SIZE: f32 = 48.0;
pub const BLINK_MS: u128 = 530;
const CLICK_INTERVAL_MS: u64 = 400;
const RIGHT_CLICK_MENU_MS: u128 = 250;

/// Pane-level actions requested from the terminal widget (context menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneCommand {
    Copy,
    /// Automatic copy keeps the highlight and does not show a toast.
    CopySelection,
    Paste,
    SelectAll,
    Clear,
    SplitRight,
    SplitDown,
    ClosePane,
}

#[derive(Default)]
pub struct SearchState {
    pub open: bool,
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub error: bool,
    /// (row, first column, end column exclusive) in viewport coordinates.
    pub current: Option<(usize, usize, usize)>,
    pub focus_requested: bool,
    compiled: RegexCache,
}

pub struct TerminalView {
    /// Font size of this pane in points (base size + zoom steps).
    pub font_size: f32,
    glyphs: GlyphCache,
    wheel_lines: f32,
    blink_epoch: Instant,
    pub metrics: Option<CellMetrics>,
    click: ClickCounter,
    right_press_at: Option<Instant>,
    pub search: SearchState,
    /// The application asked for a blinking cursor (DECSCUSR).
    pub app_blink: bool,
    menu_open: bool,
    selecting: bool,
    /// Text of the last painted rows, for link detection (columns per char).
    last_rows: Vec<(String, Vec<usize>)>,
    /// Last cell mouse reports were sent for (to avoid repeating motion).
    last_reported_cell: Option<(usize, usize)>,
    /// Deduplication of no-button motion is separate from press/release state.
    last_motion_cell: Option<(usize, usize)>,
    /// Cell under the pointer (used for wheel reports).
    hover_cell: Option<(usize, usize)>,
    /// A press report was sent for this pane: only then do we report motion
    /// and release, so clicks in other panes are never broadcast here.
    press_reported: bool,
    /// OSC 8 links of the last painted rows: (row, first column, end column, uri).
    last_links: Vec<(usize, usize, usize, String)>,
    /// Ctrl was held on the previous frame. The hover probe reads the previous
    /// frame's text, so the text must survive one frame past the release.
    ctrl_tail: bool,
    /// Where inside the scrollbar thumb the drag was grabbed: dragging keeps
    /// the grab point under the pointer instead of jumping to it.
    scroll_grab: Option<f32>,
    frame: Option<Frame>,
}

pub struct ViewInput<'a> {
    pub palette: &'a Palette,
    pub focused: bool,
    pub cursor_blink: bool,
    pub right_click: RightClick,
    pub paste_on_middle: bool,
    pub copy_on_select: bool,
    /// System fallback fonts are already installed.
    pub fallbacks_loaded: bool,
    /// The pointer is on a window resize border: the press belongs to the
    /// window, not to a text selection.
    pub window_edge: bool,
}

pub struct ViewOutput {
    /// Primary button pressed inside the pane (focus it).
    pub pressed: bool,
    /// Where the IME candidate window should appear.
    pub cursor_rect: Option<Rect>,
    pub commands: Vec<PaneCommand>,
    /// A character showed up that no installed font can draw.
    pub needs_fallbacks: bool,
}

impl TerminalView {
    pub fn new(font_size: f32) -> TerminalView {
        TerminalView {
            font_size,
            glyphs: GlyphCache::default(),
            wheel_lines: 0.0,
            blink_epoch: Instant::now(),
            metrics: None,
            click: ClickCounter::default(),
            right_press_at: None,
            search: SearchState::default(),
            app_blink: false,
            menu_open: false,
            selecting: false,
            last_rows: Vec::new(),
            ctrl_tail: false,
            scroll_grab: None,
            frame: None,
            last_reported_cell: None,
            last_motion_cell: None,
            hover_cell: None,
            press_reported: false,
            last_links: Vec::new(),
        }
    }

    pub fn zoom(&mut self, delta: f32) {
        self.font_size = (self.font_size + delta).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    }

    /// Restarts the blink cycle so the cursor is visible right after typing.
    pub fn reset_blink(&mut self) {
        self.blink_epoch = Instant::now();
    }

    /// The terminal font family changed: which characters the primary face
    /// can draw must be asked again, or glyphs stay routed for the old face.
    pub fn font_changed(&mut self) {
        self.glyphs = GlyphCache::default();
    }

    fn hover_motion(&mut self, reporting: bool, buttons_down: bool, mods: Mods, modes: MouseModes) -> Option<Vec<u8>> {
        if !reporting || self.hover_cell.is_none() {
            self.last_motion_cell = None;
            return None;
        }
        let (col, row) = self.hover_cell?;
        if buttons_down || self.last_motion_cell == Some((col, row)) {
            return None;
        }
        let bytes = mouse::encode_report(MouseButton::NoButton, MouseAction::Motion, col, row, mods, modes)?;
        self.last_motion_cell = Some((col, row));
        Some(bytes)
    }

    /// A held-button move for a tracking application. Like xterm it is reported
    /// when the pointer enters another cell, not on every frame it stays in one:
    /// an application that redraws on each report would keep the pane drawing
    /// (and answering) for as long as the button is held.
    fn drag_motion(&mut self, cell: (usize, usize), mods: Mods, modes: MouseModes) -> Option<Vec<u8>> {
        if !self.press_reported || !mouse::wants_report(MouseAction::Motion, true, modes) {
            return None;
        }
        if self.last_reported_cell == Some(cell) {
            return None;
        }
        let bytes = mouse::encode_report(MouseButton::Left, MouseAction::Motion, cell.0, cell.1, mods, modes)?;
        self.last_reported_cell = Some(cell);
        Some(bytes)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, rect: Rect, pane: &mut Pane, input: &ViewInput) -> ViewOutput {
        let ctx = ui.ctx().clone();
        let fonts = TermFonts::new(self.font_size);
        let metrics = cell_metrics(&ctx, &fonts);
        self.metrics = Some(metrics);
        let inner = Rect::from_min_max(rect.min + Vec2::splat(theme::PANE_PADDING), rect.max);
        let columns = (inner.width() / metrics.width).floor().max(2.0) as u16;
        let lines = (inner.height() / metrics.height).floor().max(1.0) as u16;
        let ppp = ctx.pixels_per_point();
        pane.resize(columns, lines, (metrics.width * ppp).round() as u16, (metrics.height * ppp).round() as u16);

        let response = ui.interact(rect, ui.id().with(("pane", pane.id)), Sense::CLICK | Sense::DRAG);
        let mut commands = Vec::new();
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        let (ctrl, shift, alt) = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift, i.modifiers.alt));
        let mods = Mods { ctrl, alt, shift, meta: false };
        let hovered = response.hovered();
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let display_offset = pane.term.lock().grid().display_offset();

        let clamp_cell = |pos: Pos2| -> (usize, usize) {
            let col = ((pos.x - inner.min.x) / metrics.width).floor().clamp(0.0, columns as f32 - 1.0);
            let row = ((pos.y - inner.min.y) / metrics.height).floor().clamp(0.0, lines as f32 - 1.0);
            (col as usize, row as usize)
        };
        let side_at = |pos: Pos2| -> Side {
            let offset = pos.x - inner.min.x;
            if offset - (offset / metrics.width).floor() * metrics.width < metrics.width / 2.0 {
                Side::Left
            } else {
                Side::Right
            }
        };

        let modes = mouse_modes(pane.term.lock().mode());
        let app_mouse = modes.any() && !shift;
        // Mouse-tracking applications own Ctrl+click too. Shift overrides
        // tracking, so Ctrl+Shift+click remains available to open a link.
        let hovered_link = if ctrl && hovered && !app_mouse {
            pointer.and_then(|pos| {
                let (col, row) = clamp_cell(pos);
                self.link_at(row, col)
            })
        } else {
            None
        };
        if let Some(url) = &hovered_link {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            // An OSC 8 label is whatever the program printed: show where
            // Ctrl+click actually goes before it is clicked.
            let _ = response.clone().on_hover_text_at_pointer(url.as_str());
        }

        let primary_pressed = hovered && !input.window_edge && ui.input(|i| i.pointer.primary_pressed());
        let primary_released = ui.input(|i| i.pointer.primary_released());
        let dragging = response.dragged();

        // The scrollback bar owns presses on its track (drags there must not
        // start a text selection).
        let bar_zone = Rect::from_min_max(Pos2::new(rect.max.x - 6.0, rect.min.y), rect.max);
        let bar_active = pane.term.lock().grid().history_size() > 0;
        if primary_pressed && pointer.is_some_and(|pos| bar_zone.contains(pos) && bar_active) {
            // Handled by the scrollbar widget later in this frame.
        } else if primary_pressed {
            if let Some(pos) = pointer {
                let (col, row) = clamp_cell(pos);
                let point = viewport_to_point(display_offset, Point::new(row, Column(col)));
                if let Some(url) = &hovered_link {
                    ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
                } else if app_mouse {
                    if let Some(bytes) =
                        mouse::encode_report(MouseButton::Left, MouseAction::Press, col, row, mods, modes)
                    {
                        pane.write(bytes);
                        self.press_reported = true;
                        self.last_reported_cell = Some((col, row));
                    }
                    self.selecting = false;
                } else {
                    let count = self.click.click(now_ms, (col, row), CLICK_INTERVAL_MS);
                    let ty = match count {
                        1 => SelectionType::Simple,
                        2 => SelectionType::Semantic,
                        _ => SelectionType::Lines,
                    };
                    let side = side_at(pos);
                    let mut term = pane.term.lock();
                    match term.selection.as_mut() {
                        Some(selection) if shift => selection.update(point, side),
                        _ => term.selection = Some(Selection::new(ty, point, side)),
                    }
                    self.selecting = true;
                }
            }
        }

        if dragging {
            if let Some(pos) = pointer {
                let (col, row) = clamp_cell(pos);
                if app_mouse {
                    if let Some(bytes) = self.drag_motion((col, row), mods, modes) {
                        pane.write(bytes);
                    }
                } else if self.selecting {
                    let point = viewport_to_point(display_offset, Point::new(row, Column(col)));
                    if let Some(selection) = pane.term.lock().selection.as_mut() {
                        selection.update(point, side_at(pos));
                    }
                }
            }
        }

        if primary_released {
            if std::mem::take(&mut self.press_reported) && modes.any() {
                if let Some((col, row)) = pointer.map(clamp_cell).or(self.last_reported_cell) {
                    if let Some(bytes) =
                        mouse::encode_report(MouseButton::Left, MouseAction::Release, col, row, mods, modes)
                    {
                        pane.write(bytes);
                    }
                }
            }
            if std::mem::take(&mut self.selecting) && input.copy_on_select {
                commands.push(PaneCommand::CopySelection);
            }
        }

        if hovered {
            // Terminal mouse reports need each raw tick, not egui's smoothed tail.
            let line_speed = ctx.options(|o| o.input_options.line_scroll_speed);
            let dy = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::MouseWheel { unit, delta, modifiers, .. } if !modifiers.shift => Some(
                            delta.y
                                * match unit {
                                    egui::MouseWheelUnit::Point => 1.0,
                                    egui::MouseWheelUnit::Line => line_speed,
                                    egui::MouseWheelUnit::Page => i.viewport_rect().height(),
                                },
                        ),
                        _ => None,
                    })
                    .sum::<f32>()
            });
            if dy != 0.0 {
                if app_mouse {
                    self.wheel_report(pane, dy, metrics.height, mods, modes);
                } else {
                    self.wheel(pane, dy, metrics.height);
                }
            }
        }

        self.hover_cell = if hovered { pointer.map(clamp_cell) } else { None };
        let motion_reporting = app_mouse && mouse::wants_report(MouseAction::Motion, false, modes);
        let any_button_down = ui.input(|i| i.pointer.any_down());
        if let Some(bytes) = self.hover_motion(motion_reporting, any_button_down, mods, modes) {
            pane.write(bytes);
        }

        // Middle button pastes.
        if hovered && input.paste_on_middle && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle)) {
            commands.push(PaneCommand::Paste);
        }

        // Right button: short click follows the config, long press opens the menu.
        if hovered && ui.input(|i| i.pointer.secondary_pressed()) {
            self.right_press_at = Some(Instant::now());
        }
        let secondary_released = ui.input(|i| i.pointer.secondary_released());
        if let Some(pressed_at) = self.release_right_press(secondary_released) {
            let short = pressed_at.elapsed().as_millis() < RIGHT_CLICK_MENU_MS;
            if short {
                match input.right_click {
                    RightClick::Paste => commands.push(PaneCommand::Paste),
                    RightClick::Clipboard => {
                        if pane.term.lock().selection.as_ref().is_some_and(|selection| !selection.is_empty()) {
                            commands.push(PaneCommand::Copy);
                        } else {
                            commands.push(PaneCommand::Paste);
                        }
                    }
                    RightClick::Menu => self.menu_open = true,
                }
            } else {
                self.menu_open = true;
            }
        }
        let menu_id = ui.id().with(("pane-menu", pane.id));
        if std::mem::take(&mut self.menu_open) {
            egui::Popup::open_id(&ctx, menu_id);
        }
        egui::Popup::from_response(&response)
            .id(menu_id)
            .open_memory(None)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                if ui.button(strings::MENU_COPY()).clicked() {
                    commands.push(PaneCommand::Copy);
                    ui.close();
                }
                if ui.button(strings::MENU_PASTE()).clicked() {
                    commands.push(PaneCommand::Paste);
                    ui.close();
                }
                if ui.button(strings::MENU_SELECT_ALL()).clicked() {
                    commands.push(PaneCommand::SelectAll);
                    ui.close();
                }
                if ui.button(strings::MENU_CLEAR()).clicked() {
                    commands.push(PaneCommand::Clear);
                    ui.close();
                }
                ui.separator();
                if ui.button(strings::MENU_SPLIT_RIGHT()).clicked() {
                    commands.push(PaneCommand::SplitRight);
                    ui.close();
                }
                if ui.button(strings::MENU_SPLIT_DOWN()).clicked() {
                    commands.push(PaneCommand::SplitDown);
                    ui.close();
                }
                if ui.button(strings::MENU_CLOSE_PANE()).clicked() {
                    commands.push(PaneCommand::ClosePane);
                    ui.close();
                }
            });

        // Search highlights for the visible screen.
        let (highlights, frame) = {
            let term = pane.term.lock_unfair();
            let mut highlights: Vec<Highlight> = Vec::new();
            if self.search.open && !self.search.query.is_empty() {
                let pattern = search_pattern(&self.search.query, self.search.regex, self.search.case_sensitive);
                match self.search.compiled.get(&pattern) {
                    Some(regex) => {
                        self.search.error = false;
                        let found = collect_matches(&*term, regex, lines as usize, columns as usize, display_offset);
                        let current = self.search.current.filter(|c| found.contains(c));
                        self.search.current = current;
                        for (row, start, end) in found {
                            let is_current = self.search.current == Some((row, start, end));
                            highlights.push((row, start, end, is_current));
                        }
                    }
                    None => {
                        self.search.error = true;
                        self.search.current = None;
                    }
                }
            }

            let primary = fonts.primary.clone();
            let frame = ctx.fonts_mut(|f| {
                snapshot_reusing(
                    &term,
                    input.palette,
                    &mut self.glyphs,
                    &mut |c| f.has_glyph(&primary, c),
                    self.frame.take(),
                )
            });
            (highlights, frame)
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, input.palette.background);
        // Link detection reads the previous frame's text, so it must also be
        // built on the frame Ctrl is released. Neither is consulted otherwise.
        if ctrl || self.ctrl_tail {
            self.ctrl_tail = ctrl;
            self.last_rows.resize_with(frame.rows.len(), || (String::new(), Vec::new()));
            for (row, (text, cols)) in frame.rows.iter().zip(&mut self.last_rows) {
                text.clear();
                cols.clear();
                for (i, cell) in row.iter().enumerate() {
                    if cell.spacer {
                        continue;
                    }
                    // Base character, then its combining marks, as the painter
                    // and the clipboard build them: a URL with a mark inside
                    // must be found and opened in the order it is written.
                    text.push(cell.ch);
                    cols.push(i);
                    if let Some(extra) = cell.combining.as_deref() {
                        text.push_str(extra);
                        for _ in 0..extra.chars().count() {
                            cols.push(i);
                        }
                    }
                }
            }
            // OSC 8 targets per row, so Ctrl+click works on hyperlinked labels too.
            self.last_links = link_runs(&frame.rows);
        }

        let cursor_on = if (input.cursor_blink || self.app_blink) && input.focused {
            let elapsed = self.blink_epoch.elapsed().as_millis();
            let left = BLINK_MS - elapsed % BLINK_MS;
            ctx.request_repaint_after(Duration::from_millis(left as u64));
            (elapsed / BLINK_MS).is_multiple_of(2)
        } else {
            true
        };
        paint(
            &painter,
            inner.min,
            &frame,
            &PaintOptions {
                metrics,
                fonts: &fonts,
                palette: input.palette,
                focused: input.focused,
                cursor_on,
                highlights: &highlights,
            },
        );

        // Ctrl+hover underline for links.
        if let (Some(url), Some(pos)) = (&hovered_link, pointer) {
            let (col, row) = clamp_cell(pos);
            if let Some((_, start, end)) = self.link_span(row, col) {
                let y = inner.min.y + (row + 1) as f32 * metrics.height - 1.0;
                painter.line_segment(
                    [
                        Pos2::new(inner.min.x + start as f32 * metrics.width, y),
                        Pos2::new(inner.min.x + end as f32 * metrics.width, y),
                    ],
                    egui::Stroke::new(1.0, input.palette.foreground),
                );
            }
            let _ = url;
        }

        // Scrollback bar: painted when scrolled, draggable to move the view.
        if frame.history_size > 0 {
            let track = Rect::from_min_max(Pos2::new(rect.max.x - 6.0, rect.min.y), rect.max);
            let response = ui.interact(track, ui.id().with(("scrollbar", pane.id)), Sense::click_and_drag());
            let active = frame.display_offset > 0 || response.hovered() || response.dragged();
            let thumb = if active {
                let visible = frame.lines as f32 / (frame.lines + frame.history_size) as f32;
                let top = 1.0 - (frame.display_offset + frame.lines) as f32 / (frame.lines + frame.history_size) as f32;
                let thumb = Rect::from_min_size(
                    Pos2::new(track.min.x, track.min.y + top * track.height()),
                    Vec2::new(track.width(), (visible * track.height()).max(12.0)),
                );
                painter.rect_filled(thumb, 0.0, theme::colors().divider_hover);
                Some(thumb)
            } else {
                None
            };
            // The grab point is kept: `fraction` addresses the thumb's top, so
            // without this the view jumps to put the thumb under the pointer.
            if response.drag_started() {
                self.scroll_grab = response.interact_pointer_pos().map(|pos| {
                    let thumb = thumb.unwrap_or(track);
                    (pos.y - thumb.min.y).clamp(0.0, thumb.height())
                });
            }
            if response.dragged() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let grab = self.scroll_grab.unwrap_or(0.0);
                    let total = (frame.lines + frame.history_size) as f32;
                    let fraction = ((pos.y - grab - track.min.y) / track.height()).clamp(0.0, 1.0);
                    let target = ((1.0 - fraction) * total - frame.lines as f32).round().max(0.0) as i32;
                    let delta = target - frame.display_offset as i32;
                    if delta != 0 {
                        pane.term.lock().scroll_display(Scroll::Delta(delta));
                    }
                }
            }
        }

        let cursor_rect = frame.cursor.map(|c| {
            Rect::from_min_size(
                inner.min + Vec2::new(c.col as f32 * metrics.width, c.row as f32 * metrics.height),
                Vec2::new(metrics.width, metrics.height),
            )
        });

        // Search bar overlay.
        if self.search.open {
            let bar = Rect::from_min_size(
                Pos2::new((rect.right() - 330.0).max(rect.left()), rect.top() + 4.0),
                Vec2::new(326.0, 26.0),
            );
            let mut nav_next = false;
            let mut nav_prev = false;
            let mut close = false;
            ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut self.search.query)
                                .hint_text(strings::SEARCH_PLACEHOLDER())
                                // Enter navigates; the default single-line
                                // behaviour would drop the focus and route the
                                // next keystrokes to the shell.
                                .return_key(None)
                                .desired_width(150.0),
                        );
                        if self.search.focus_requested {
                            field.request_focus();
                            self.search.focus_requested = false;
                        }
                        if field.changed() {
                            self.search.current = None;
                        }
                        if self.search.error {
                            // Keep the field focused: the user is mid-expression.
                            ui.colored_label(egui::Color32::from_rgb(0xf9, 0x26, 0x72), "!");
                        }
                        if ui
                            .selectable_label(self.search.case_sensitive, "Aa")
                            .on_hover_text(strings::SEARCH_CASE())
                            .clicked()
                        {
                            self.search.case_sensitive = !self.search.case_sensitive;
                            self.search.current = None;
                        }
                        if ui.selectable_label(self.search.regex, ".*").on_hover_text(strings::SEARCH_REGEX()).clicked()
                        {
                            self.search.regex = !self.search.regex;
                            self.search.current = None;
                        }
                        if ui.small_button("↑").on_hover_text(strings::SEARCH_PREV()).clicked() {
                            nav_prev = true;
                        }
                        if ui.small_button("↓").on_hover_text(strings::SEARCH_NEXT()).clicked() {
                            nav_next = true;
                        }
                        if ui.small_button("×").on_hover_text(strings::SEARCH_CLOSE()).clicked() {
                            close = true;
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                            nav_next = true;
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.shift) {
                            nav_prev = true;
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                    });
                });
            });
            if nav_next || nav_prev {
                self.search_step(pane, nav_next, display_offset);
            }
            if close {
                self.search.open = false;
                self.search.current = None;
            }
        }

        let needs_fallbacks = !input.fallbacks_loaded
            && frame
                .rows
                .iter()
                .any(|row| row.iter().any(|cell| !cell.in_primary_font && !cell.spacer && cell.ch != ' '));
        self.frame = Some(frame);
        ViewOutput { pressed: response.is_pointer_button_down_on(), cursor_rect, commands, needs_fallbacks }
    }

    fn release_right_press(&mut self, released: bool) -> Option<Instant> {
        if released {
            self.right_press_at.take()
        } else {
            None
        }
    }

    /// Recomputes the current match and scrolls it into view.
    fn search_step(&mut self, pane: &Pane, forward: bool, display_offset: usize) {
        if self.search.query.is_empty() {
            return;
        }
        let pattern = search_pattern(&self.search.query, self.search.regex, self.search.case_sensitive);
        let Some(regex) = self.search.compiled.get(&pattern) else {
            self.search.error = true;
            return;
        };
        self.search.error = false;
        let mut term = pane.term.lock();
        let lines = term.screen_lines();
        let total_columns = term.columns().max(1);
        let history = term.grid().history_size() as i32;
        let last_line = lines as i32 - 1;
        let origin = step_origin(&term, self.search.current, forward, display_offset);
        let direction = if forward { Direction::Right } else { Direction::Left };
        let found = term.search_next(regex, origin, direction, Side::Left, None).or_else(|| {
            let wrap = if forward {
                Point::new(Line(-history), Column(0))
            } else {
                Point::new(Line(last_line), Column(total_columns - 1))
            };
            term.search_next(regex, wrap, direction, Side::Left, None)
        });
        if let Some(found) = found {
            term.scroll_to_point(*found.start());
            // The offset may have changed with the scroll: convert afterwards so
            // the highlight lands on the row the user now sees.
            let offset = term.grid().display_offset();
            if let Some(row) = alacritty_terminal::term::point_to_viewport(offset, *found.start()) {
                if row.line < lines {
                    self.search.current = Some((row.line, found.start().column.0, found.end().column.0 + 1));
                }
            }
        }
    }

    /// Text and column positions of the row under (row, col) when it holds a link.
    fn link_at(&self, row: usize, col: usize) -> Option<String> {
        self.link_span(row, col).map(|(url, _, _)| url)
    }

    fn link_span(&self, row: usize, col: usize) -> Option<(String, usize, usize)> {
        if let Some((_, start, end, uri)) =
            self.last_links.iter().find(|(link_row, start, end, _)| *link_row == row && (*start..*end).contains(&col))
        {
            if links::is_openable(uri) {
                return Some((uri.clone(), *start, *end));
            }
        }
        let (text, cols) = self.last_rows.get(row)?;
        let mut start = 0;
        while let Some(offset) = find_scheme(text, start) {
            let rest = &text[offset..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "<>\"'`".contains(c))
                .map(|i| offset + i)
                .unwrap_or(text.len());
            let url = links::trim_url(&text[offset..end]);
            let chars_before = text[..offset].chars().count();
            let chars_url = url.chars().count();
            if chars_url > 0 {
                let first_col = *cols.get(chars_before)?;
                let last_col = *cols.get(chars_before + chars_url - 1)?;
                if (first_col..=last_col).contains(&col) {
                    if links::is_openable(url) {
                        return Some((url.to_owned(), first_col, last_col + 1));
                    }
                    return None;
                }
            }
            start = end;
            if start >= text.len() {
                break;
            }
        }
        None
    }

    /// Wheel deltas accumulate in `wheel_lines` and are spent a whole line at a
    /// time. `f32 as i32` saturates, so an anomalous or infinite delta from a
    /// driver would otherwise reach `repeat`/`unsigned_abs` as `i32::MAX` and
    /// ask for a two-gigabyte allocation (or two billion PTY writes).
    /// Anything past a fast scroll is dropped; the accumulator is reset so the
    /// next real event starts clean.
    fn take_wheel_lines(&mut self, dy_points: f32, cell_height: f32) -> i32 {
        const MAX_LINES: f32 = 1_000.0;
        self.wheel_lines =
            if self.wheel_lines.is_finite() { self.wheel_lines.clamp(-MAX_LINES, MAX_LINES) } else { 0.0 };
        self.wheel_lines += dy_points / cell_height;
        let lines = self.wheel_lines.trunc();
        self.wheel_lines -= lines;
        if lines.is_finite() {
            lines as i32
        } else {
            0
        }
    }

    fn wheel(&mut self, pane: &Pane, dy_points: f32, cell_height: f32) {
        let lines = self.take_wheel_lines(dy_points, cell_height);
        if lines == 0 {
            return;
        }
        let mode = *pane.term.lock().mode();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let key: &[u8] = match (lines > 0, app_cursor) {
                (true, false) => b"\x1b[A",
                (true, true) => b"\x1bOA",
                (false, false) => b"\x1b[B",
                (false, true) => b"\x1bOB",
            };
            pane.write(key.repeat(lines.unsigned_abs() as usize));
        } else {
            pane.term.lock().scroll_display(Scroll::Delta(lines));
        }
    }

    fn wheel_report(&mut self, pane: &Pane, dy_points: f32, cell_height: f32, mods: Mods, modes: MouseModes) {
        let lines = self.take_wheel_lines(dy_points, cell_height);
        let button = if lines > 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
        if let Some((col, row)) = self.hover_cell {
            for _ in 0..lines.unsigned_abs() {
                if let Some(bytes) = mouse::encode_report(button, MouseAction::Press, col, row, mods, modes) {
                    pane.write(bytes);
                }
            }
        }
    }
}

/// Where the search for the next match starts: from the match on screen if
/// there is one, else from the cursor.
fn step_origin<L: EventListener>(
    term: &Term<L>,
    current: Option<(usize, usize, usize)>,
    forward: bool,
    display_offset: usize,
) -> Point {
    let lines = term.screen_lines();
    let total_columns = term.columns().max(1);
    let history = term.grid().history_size() as i32;
    let last_line = lines as i32 - 1;
    match current {
        Some((row, _, end)) if forward => {
            let line = (row as i32 - display_offset as i32).clamp(-history, last_line);
            // `end` is exclusive: past the last column the next cell is the first
            // one of the next line (wrapping from the bottom to the top).
            if end >= total_columns {
                Point::new(Line(line), Column(total_columns - 1)).add(term, Boundary::None, 1)
            } else {
                Point::new(Line(line), Column(end))
            }
        }
        Some((row, start, _)) => {
            // Step one cell back so the current match is not returned again.
            let line = (row as i32 - display_offset as i32).clamp(-history, last_line);
            match start.checked_sub(1) {
                Some(col) => Point::new(Line(line), Column(col)),
                None => Point::new(Line((line - 1).max(-history)), Column(total_columns - 1)),
            }
        }
        None => {
            let cursor = term.grid().cursor.point;
            Point::new(Line(cursor.line.0.clamp(-history, last_line)), Column(cursor.column.0.min(total_columns - 1)))
        }
    }
}

fn find_scheme(text: &str, from: usize) -> Option<usize> {
    ["https://", "http://", "ftp://", "mailto:"].iter().filter_map(|s| text[from..].find(s).map(|i| i + from)).min()
}

fn mouse_modes(mode: &TermMode) -> MouseModes {
    MouseModes {
        click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
        drag: mode.contains(TermMode::MOUSE_DRAG),
        motion: mode.contains(TermMode::MOUSE_MOTION),
        sgr: mode.contains(TermMode::SGR_MOUSE),
        utf8: mode.contains(TermMode::UTF8_MOUSE),
    }
}

fn viewport_to_point(display_offset: usize, point: Point<usize>) -> Point {
    alacritty_terminal::term::viewport_to_point(display_offset, point)
}

/// Matches the search bar may highlight across the whole visible screen in one
/// frame. The scan runs on every frame while the bar is open, so this has to
/// bound the work, not the per-row work.
pub const MAX_VISIBLE_MATCHES: usize = 400;

/// Visible-screen matches of `regex`, per row, as (row, start column, end column).
fn collect_matches<L: EventListener>(
    term: &Term<L>,
    regex: &mut RegexSearch,
    lines: usize,
    columns: usize,
    display_offset: usize,
) -> Vec<(usize, usize, usize)> {
    // The budget is for the whole visible screen, not per row: a row's own limit
    // multiplied by the row count turns a broad regex into tens of thousands of
    // searches on every frame.
    let mut budget = MAX_VISIBLE_MATCHES;
    let mut out = Vec::new();
    if columns == 0 {
        return out;
    }
    for row in 0..lines {
        let line = row as i32 - display_offset as i32;
        let start = Point::new(Line(line), Column(0));
        let end = Point::new(Line(line), Column(columns - 1));
        let mut origin = start;
        while budget > 0 {
            budget -= 1;
            let Some(found) = term.regex_search_right(regex, origin, end) else { break };
            let (first, last) = (*found.start(), *found.end());
            if first.line != last.line || last.column.0 >= columns {
                break;
            }
            out.push((row, first.column.0, last.column.0 + 1));
            let next = Point::new(last.line, Column(last.column.0 + 1));
            origin = if next == origin { Point::new(origin.line, Column(origin.column.0 + 1)) } else { next };
            if origin.column.0 >= columns {
                break;
            }
        }
    }
    out
}

/// `(row, first column, end column, target)` of every OSC 8 link on screen.
/// Cells of one link share its target (one OSC 8 sequence), so a run is
/// recognised by identity and its target copied once, not compared and
/// copied per cell.
pub fn link_runs(rows: &[Vec<crate::term::style::RenderCell>]) -> Vec<(usize, usize, usize, String)> {
    let mut links = Vec::new();
    for (row, cells) in rows.iter().enumerate() {
        let mut run: Option<(usize, &str)> = None;
        for (col, cell) in cells.iter().enumerate() {
            let uri = cell.hyperlink.as_ref().map(|link| link.uri()).filter(|uri| !uri.is_empty());
            let continues = matches!((run, uri), (Some((_, current)), Some(next)) if std::ptr::eq(current, next));
            if continues {
                continue;
            }
            if let Some((start, current)) = run.take() {
                links.push((row, start, col, current.to_owned()));
            }
            run = uri.map(|next| (col, next));
        }
        if let Some((start, current)) = run {
            links.push((row, start, cells.len(), current.to_owned()));
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::style::{CellStyle, RenderCell, Underline};
    use alacritty_terminal::term::cell::Hyperlink;
    use egui::Color32;

    #[test]
    fn enabling_motion_reports_the_current_cell_without_losing_release_coordinates() {
        let mut view = TerminalView::new(15.0);
        let modes = MouseModes { motion: true, sgr: true, ..Default::default() };
        view.last_reported_cell = Some((3, 4)); // A press to release even if the pointer leaves.
        view.hover_cell = Some((3, 4));
        assert!(view.hover_motion(false, false, Mods::default(), modes).is_none());
        let expected = b"\x1b[<35;4;5M".to_vec();
        assert_eq!(view.hover_motion(true, false, Mods::default(), modes), Some(expected.clone()));
        assert!(view.hover_motion(true, false, Mods::default(), modes).is_none());
        view.hover_cell = None;
        assert!(view.hover_motion(true, false, Mods::default(), modes).is_none());
        assert_eq!(view.last_reported_cell, Some((3, 4)), "hover state cannot erase a pending release");
        view.hover_cell = Some((3, 4));
        assert_eq!(view.hover_motion(true, false, Mods::default(), modes), Some(expected));
    }

    /// `end` of a match is exclusive: for one in the last column the step has to
    /// start on the next line, not on the match's own cell, or Enter finds it
    /// again forever.
    #[test]
    fn next_match_steps_past_one_in_the_last_column() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::Config;
        use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
        let size = crate::term::pane::GridSize { columns: 20, lines: 3 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        Processor::<StdSyncHandler>::new().advance(&mut term, b"\x1b[1;20Hx\x1b[2;3Hx");
        let mut regex = RegexSearch::new("x").unwrap();
        let next = |term: &Term<VoidListener>, regex: &mut RegexSearch, current, forward| {
            let origin = step_origin(term, current, forward, 0);
            let direction = if forward { Direction::Right } else { Direction::Left };
            let found = term.search_next(regex, origin, direction, Side::Left, None).expect("a match");
            (found.start().line.0, found.start().column.0)
        };
        assert_eq!(next(&term, &mut regex, Some((0, 19, 20)), true), (1, 2), "forward from the last column");
        assert_eq!(next(&term, &mut regex, Some((1, 2, 3)), true), (0, 19), "forward wraps to the top");
        assert_eq!(next(&term, &mut regex, Some((1, 2, 3)), false), (0, 19), "backward");
        assert_eq!(next(&term, &mut regex, Some((0, 19, 20)), false), (1, 2), "backward wraps to the bottom");
    }

    #[test]
    fn a_held_button_reports_motion_only_when_the_cell_changes() {
        let mut view = TerminalView::new(15.0);
        let modes = MouseModes { drag: true, sgr: true, ..Default::default() };
        assert!(view.drag_motion((3, 4), Mods::default(), modes).is_none(), "no press was reported to this pane");
        view.press_reported = true;
        view.last_reported_cell = Some((3, 4));
        assert!(view.drag_motion((3, 4), Mods::default(), modes).is_none(), "the pointer stayed in the cell");
        let moved = view.drag_motion((4, 4), Mods::default(), modes).expect("a new cell");
        assert_eq!(moved, b"\x1b[<32;5;5M".to_vec());
        assert!(view.drag_motion((4, 4), Mods::default(), modes).is_none(), "once per cell");
        assert!(view.drag_motion((3, 4), Mods::default(), modes).is_some(), "moving back is a move");
        let clicks_only = MouseModes { click: true, sgr: true, ..Default::default() };
        assert!(view.drag_motion((9, 9), Mods::default(), clicks_only).is_none(), "1000 does not report drags");
    }

    fn cell(ch: char, hyperlink: Option<Hyperlink>) -> RenderCell {
        let style = CellStyle {
            fg: Color32::WHITE,
            bg: Color32::BLACK,
            bold: false,
            italic: false,
            underline: Underline::None,
            strike: false,
        };
        RenderCell { ch, combining: None, style, wide: false, spacer: false, in_primary_font: true, hyperlink }
    }

    #[test]
    fn right_press_survives_frames_until_release() {
        let mut view = TerminalView::new(15.0);
        let pressed_at = Instant::now();
        view.right_press_at = Some(pressed_at);
        assert_eq!(view.release_right_press(false), None);
        assert_eq!(view.release_right_press(false), None);
        assert_eq!(view.release_right_press(true), Some(pressed_at));
        assert_eq!(view.release_right_press(true), None);
    }
    #[test]
    fn hyperlinked_cells_form_one_target_per_link() {
        let a = Hyperlink::new(None::<String>, "https://a.example".to_owned());
        let b = Hyperlink::new(None::<String>, "https://b.example".to_owned());
        let row = vec![
            cell('x', None),
            cell('a', Some(a.clone())),
            cell('a', Some(a.clone())),
            cell('b', Some(b)),
            cell('y', None),
            cell('a', Some(a)),
        ];
        assert_eq!(
            link_runs(&[row]),
            vec![
                (0, 1, 3, "https://a.example".to_owned()),
                (0, 3, 4, "https://b.example".to_owned()),
                (0, 5, 6, "https://a.example".to_owned()),
            ]
        );
    }
}
