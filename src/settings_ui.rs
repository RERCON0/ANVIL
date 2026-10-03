//! The Settings page (Ctrl+, or the gear): appearance, terminal, profiles,
//! hotkeys and the Claude Code status switch. Changes apply immediately.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use egui::{Color32, Rect, RichText};

use crate::config::{Config, CursorShapeConfig, ProfileConfig, RightClick};
use crate::strings;
use crate::theme;

pub struct SettingsState {
    pub editing: Option<usize>,
    pub adding: bool,
    pub draft: ProfileConfig,
}

impl Default for SettingsState {
    fn default() -> Self {
        SettingsState { editing: None, adding: false, draft: draft_profile() }
    }
}

pub struct SettingsContext<'a> {
    pub config: &'a mut Config,
    pub keymap_rows: Vec<(String, Vec<String>)>,
    pub profiles: Vec<(String, String)>,
    pub fonts: Vec<String>,
}

pub struct SettingsOutcome {
    pub changed: bool,
    pub open_config: bool,
}

fn draft_profile() -> ProfileConfig {
    ProfileConfig {
        id: String::new(),
        name: String::new(),
        command: String::new(),
        args: Vec::new(),
        cwd: None,
        env: Default::default(),
    }
}

pub fn show(ui: &mut egui::Ui, rect: Rect, cx: &mut SettingsContext, state: &mut SettingsState) -> SettingsOutcome {
    let mut outcome = SettingsOutcome { changed: false, open_config: false };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CHROME_BG);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(16.0)), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(RichText::new(strings::TAB_SETTINGS).color(theme::TAB_ACTIVE_TEXT));
            ui.label(RichText::new(strings::SETTINGS_APPLY_HINT).small().color(theme::TAB_NUMBER));
            ui.add_space(8.0);

            // Appearance.
            ui.group(|ui| {
                ui.label(RichText::new(strings::SETTINGS_APPEARANCE).strong());
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_FONT);
                    egui::ComboBox::from_id_salt("font-family")
                        .selected_text(cx.config.font.family.clone())
                        .show_ui(ui, |ui| {
                            for family in &cx.fonts {
                                if ui.selectable_label(*family == cx.config.font.family, family).clicked() {
                                    cx.config.font.family = family.clone();
                                    outcome.changed = true;
                                }
                            }
                        });
                    if ui
                        .add(egui::TextEdit::singleline(&mut cx.config.font.family).desired_width(160.0))
                        .changed()
                    {
                        outcome.changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_FONT_SIZE);
                    if ui.add(egui::Slider::new(&mut cx.config.font.size, 6.0..=48.0).fixed_decimals(0)).changed() {
                        outcome.changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_SCHEME);
                    let names = crate::app::scheme_names(cx.config);
                    egui::ComboBox::from_id_salt("color-scheme")
                        .selected_text(cx.config.color_scheme.clone())
                        .show_ui(ui, |ui| {
                            for name in &names {
                                if ui.selectable_label(*name == cx.config.color_scheme, name).clicked() {
                                    cx.config.color_scheme = name.clone();
                                    outcome.changed = true;
                                }
                            }
                        });
                    let palette = crate::app::scheme_palette(cx.config);
                    let (response, painter) = ui.allocate_painter(egui::Vec2::new(16.0 * 16.0, 14.0), egui::Sense::hover());
                    for (index, color) in palette.ansi.iter().enumerate() {
                        let cell = Rect::from_min_size(
                            response.rect.min + egui::Vec2::new(index as f32 * 16.0, 0.0),
                            egui::Vec2::new(16.0, 14.0),
                        );
                        painter.rect_filled(cell, 0.0, *color);
                    }
                });
            });

            // Terminal.
            ui.group(|ui| {
                ui.label(RichText::new(strings::SETTINGS_TERMINAL).strong());
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_SCROLLBACK);
                    if ui.add(egui::DragValue::new(&mut cx.config.terminal.scrollback).range(0..=1_000_000)).changed() {
                        outcome.changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_CURSOR);
                    for (shape, label) in [
                        (CursorShapeConfig::Block, strings::CURSOR_BLOCK),
                        (CursorShapeConfig::Bar, strings::CURSOR_BAR),
                        (CursorShapeConfig::Underline, strings::CURSOR_UNDERLINE),
                    ] {
                        if ui.selectable_label(cx.config.terminal.cursor.shape == shape, label).clicked() {
                            cx.config.terminal.cursor.shape = shape;
                            outcome.changed = true;
                        }
                    }
                    if ui.checkbox(&mut cx.config.terminal.cursor.blink, strings::SETTINGS_BLINK).changed() {
                        outcome.changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_RIGHT_CLICK);
                    for (mode, label) in [
                        (RightClick::Clipboard, strings::RIGHT_CLICK_CLIPBOARD),
                        (RightClick::Paste, strings::RIGHT_CLICK_PASTE),
                        (RightClick::Menu, strings::RIGHT_CLICK_MENU),
                    ] {
                        if ui.selectable_label(cx.config.terminal.right_click == mode, label).clicked() {
                            cx.config.terminal.right_click = mode;
                            outcome.changed = true;
                        }
                    }
                });
                if ui.checkbox(&mut cx.config.terminal.paste_on_middle_click, strings::SETTINGS_MIDDLE_CLICK).changed() {
                    outcome.changed = true;
                }
                if ui.checkbox(&mut cx.config.terminal.copy_on_select, strings::SETTINGS_COPY_ON_SELECT).changed() {
                    outcome.changed = true;
                }
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_WORD_SEPARATORS);
                    if ui.add(egui::TextEdit::singleline(&mut cx.config.terminal.word_separators).desired_width(200.0)).changed() {
                        outcome.changed = true;
                    }
                });
            });

            // Profiles.
            ui.group(|ui| {
                ui.label(RichText::new(strings::SETTINGS_PROFILES).strong());
                ui.horizontal(|ui| {
                    ui.label(strings::SETTINGS_DEFAULT_PROFILE);
                    egui::ComboBox::from_id_salt("default-profile")
                        .selected_text(cx.config.default_profile.clone())
                        .show_ui(ui, |ui| {
                            for (id, name) in &cx.profiles {
                                if ui.selectable_label(*id == cx.config.default_profile, name).clicked() {
                                    cx.config.default_profile = id.clone();
                                    outcome.changed = true;
                                }
                            }
                            for profile in cx.config.profiles.iter() {
                                if ui.selectable_label(profile.id == cx.config.default_profile, &profile.name).clicked() {
                                    cx.config.default_profile = profile.id.clone();
                                    outcome.changed = true;
                                }
                            }
                        });
                });
                let mut delete: Option<usize> = None;
                for (index, profile) in cx.config.profiles.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} — {}", profile.name, profile.command));
                        if ui.small_button(strings::SETTINGS_EDIT).clicked() {
                            state.editing = Some(index);
                            state.adding = false;
                            state.draft = profile.clone();
                        }
                        if ui.small_button(strings::SETTINGS_DELETE).clicked() {
                            delete = Some(index);
                        }
                    });
                }
                if let Some(index) = delete {
                    let removed = cx.config.profiles.remove(index);
                    if cx.config.default_profile == removed.id {
                        cx.config.default_profile = "git-bash".into();
                    }
                    outcome.changed = true;
                }
                if ui.button(strings::SETTINGS_ADD).clicked() {
                    state.adding = true;
                    state.editing = None;
                    state.draft = draft_profile();
                }
                if state.adding || state.editing.is_some() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(strings::SETTINGS_NAME);
                        ui.text_edit_singleline(&mut state.draft.name);
                    });
                    ui.horizontal(|ui| {
                        ui.label(strings::SETTINGS_COMMAND);
                        ui.add(egui::TextEdit::singleline(&mut state.draft.command).desired_width(360.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label(strings::SETTINGS_ARGS);
                        let mut args = state.draft.args.join(" ");
                        if ui.add(egui::TextEdit::singleline(&mut args).desired_width(300.0)).changed() {
                            state.draft.args = args.split_whitespace().map(str::to_owned).collect();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(strings::SETTINGS_CWD);
                        let mut cwd = state.draft.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut cwd).desired_width(360.0)).changed() {
                            state.draft.cwd = (!cwd.trim().is_empty()).then(|| PathBuf::from(cwd.trim()));
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button(strings::SETTINGS_SAVE).clicked() && !state.draft.command.trim().is_empty() {
                            if state.draft.name.trim().is_empty() {
                                state.draft.name = state.draft.command.clone();
                            }
                            if state.draft.id.is_empty() {
                                state.draft.id = unique_profile_id(cx.config, &state.draft.name);
                            }
                            match state.editing {
                                Some(index) => cx.config.profiles[index] = state.draft.clone(),
                                None => cx.config.profiles.push(state.draft.clone()),
                            }
                            state.adding = false;
                            state.editing = None;
                            outcome.changed = true;
                        }
                        if ui.button(strings::SETTINGS_CANCEL).clicked() {
                            state.adding = false;
                            state.editing = None;
                        }
                    });
                }
            });

            // Hotkeys.
            ui.group(|ui| {
                ui.label(RichText::new(strings::SETTINGS_HOTKEYS).strong());
                ui.label(RichText::new(strings::SETTINGS_HOTKEYS_HINT).small().color(theme::TAB_NUMBER));
                egui::Grid::new("hotkeys").num_columns(2).spacing([18.0, 2.0]).show(ui, |ui| {
                    for (action, chords) in &cx.keymap_rows {
                        ui.label(RichText::new(action).color(theme::TAB_ACTIVE_TEXT));
                        ui.label(RichText::new(chords.join(", ")).color(theme::TAB_TEXT));
                        ui.end_row();
                    }
                });
                if ui.button(strings::SETTINGS_OPEN_CONFIG).clicked() {
                    outcome.open_config = true;
                }
            });

            // Claude Code.
            ui.group(|ui| {
                ui.label(RichText::new(strings::SETTINGS_CLAUDE).strong());
                let mut enabled = cx.config.claude_status.enabled;
                if ui.checkbox(&mut enabled, strings::SETTINGS_CLAUDE_ENABLED).changed() {
                    cx.config.claude_status.enabled = enabled;
                    outcome.changed = true;
                }
                let state_text = if cx.config.claude_status.declined_command.is_some() {
                    RichText::new(strings::SETTINGS_CLAUDE_DECLINED).color(theme::STATUS_YELLOW)
                } else if cx.config.claude_status.enabled {
                    RichText::new(strings::SETTINGS_CLAUDE_CONNECTED).color(theme::STATUS_GREEN)
                } else {
                    RichText::new(strings::SETTINGS_CLAUDE_NOT_CONNECTED).color(theme::TAB_TEXT)
                };
                ui.label(state_text);
            });
        });
    });
    outcome
}

fn unique_profile_id(config: &Config, name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_owned();
    let base = if slug.is_empty() { "custom".to_owned() } else { slug };
    let mut id = base.clone();
    let mut n = 2;
    while config.profiles.iter().any(|p| p.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Opens a file with the shell (config.json in its default editor).
pub fn open_path(path: &Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let operation = wide(OsStr::new("open"));
    let file = wide(path.as_os_str());
    // SAFETY: NUL-terminated strings; no output parameters are used.
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), operation.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

pub fn palette_swatch(_color: Color32) {}
