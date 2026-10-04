//! The Settings page (Ctrl+, or the gear): appearance, terminal, profiles,
//! hotkeys and the Claude Code status switch, in the shared "Terminal Native"
//! control style (square, hairline, ghost buttons, one accent).

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use egui::{Align, Layout, Rect, RichText, Sense, Stroke, Vec2};

use crate::config::{Config, CursorShapeConfig, ProfileConfig, RightClick};
use crate::strings;
use crate::theme;


/// Settings pages, one per entry of the vertical nav (as in the reference
/// settings screen): appearance, terminal, profiles, the git panel, hotkeys
/// and the Claude Code switch.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    Appearance,
    Terminal,
    Profiles,
    Workspace,
    Hotkeys,
    Claude,
}

impl SettingsSection {
    const ALL: [SettingsSection; 6] = [
        SettingsSection::Appearance,
        SettingsSection::Terminal,
        SettingsSection::Profiles,
        SettingsSection::Workspace,
        SettingsSection::Hotkeys,
        SettingsSection::Claude,
    ];

    fn title(self) -> &'static str {
        match self {
            SettingsSection::Appearance => strings::SETTINGS_APPEARANCE,
            SettingsSection::Terminal => strings::SETTINGS_TERMINAL,
            SettingsSection::Profiles => strings::SETTINGS_PROFILES,
            SettingsSection::Workspace => strings::SETTINGS_WORKSPACE,
            SettingsSection::Hotkeys => strings::SETTINGS_HOTKEYS,
            SettingsSection::Claude => strings::SETTINGS_CLAUDE,
        }
    }
}

pub struct SettingsState {
    pub section: SettingsSection,
    pub editing: Option<usize>,
    pub adding: bool,
    pub draft: ProfileConfig,
    model_catalog: ModelCatalog,
    model_filter: String,
}

impl Default for SettingsState {
    fn default() -> Self {
        SettingsState {
            section: SettingsSection::default(),
            editing: None,
            adding: false,
            draft: draft_profile(),
            model_catalog: ModelCatalog::default(),
            model_filter: String::new(),
        }
    }
}

#[derive(Default)]
enum ModelCatalog {
    #[default]
    NotLoaded,
    Loading(std::sync::mpsc::Receiver<Result<Vec<String>, String>>),
    Ready(Result<Vec<String>, String>),
}

impl SettingsState {
    fn load_models(&mut self, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.model_catalog = ModelCatalog::Loading(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::git::opencode_models());
            ctx.request_repaint();
        });
    }

    fn absorb_models(&mut self) {
        if let ModelCatalog::Loading(rx) = &self.model_catalog {
            match rx.try_recv() {
                Ok(result) => self.model_catalog = ModelCatalog::Ready(result),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.model_catalog = ModelCatalog::Ready(Err(strings::WORKSPACE_AI_EMPTY.to_owned()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}

pub struct SettingsContext<'a> {
    pub config: &'a mut Config,
    pub keymap_rows: Vec<(String, Vec<String>)>,
    pub profiles: Vec<(String, String)>,
    pub fonts: &'a [String],
}

pub struct SettingsOutcome {
    pub changed: bool,
    pub open_config: bool,
    pub refresh_fonts: bool,
    pub install_claude: bool,
    pub restore_claude: bool,
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
    let mut outcome = SettingsOutcome { changed: false, open_config: false, refresh_fonts: false, install_claude: false, restore_claude: false };
    ui.painter_at(rect).rect_filled(rect, 0.0, theme::colors().chrome_bg);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(18.0, 12.0))).id_salt("settings-page"), |ui| {
        ui.label(RichText::new(strings::TAB_SETTINGS).color(theme::colors().text).font(theme::title_font(15.0)));
        ui.label(RichText::new(strings::SETTINGS_APPLY_HINT).color(theme::colors().faint).font(theme::font(11.5)));
        ui.add_space(10.0);

        // One section at a time: the list on the left, its rows on the right.
        let body = ui.available_rect_before_wrap();
        let nav_width = 172.0_f32.min(body.width() * 0.4);
        let nav_rect = Rect::from_min_size(body.min, Vec2::new(nav_width, body.height()));
        ui.scope_builder(egui::UiBuilder::new().max_rect(nav_rect).id_salt("settings-nav"), |ui| {
            for section in SettingsSection::ALL {
                let selected = state.section == section;
                let (row, response) = ui.allocate_exact_size(Vec2::new(nav_rect.width(), 26.0), Sense::click());
                if selected {
                    ui.painter().rect_filled(row, 0.0, theme::colors().tab_active_bg);
                    ui.painter().rect_stroke(row, 0.0, Stroke::new(1.0, theme::colors().line), egui::StrokeKind::Middle);
                } else if response.hovered() {
                    ui.painter().rect_filled(row, 0.0, theme::colors().tab_hover_bg);
                }
                let color = if selected { theme::colors().accent } else { theme::colors().dim };
                ui.painter().text(
                    egui::Pos2::new(row.min.x + 10.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    section.title(),
                    theme::font(12.5),
                    color,
                );
                if response.clicked() {
                    state.section = section;
                }
            }
        });

        let content_rect = Rect::from_min_max(egui::Pos2::new(nav_rect.max.x + 18.0, body.min.y), body.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect).id_salt("settings-content"), |ui| {
            egui::ScrollArea::vertical().id_salt("settings-section").auto_shrink([false, false]).show(ui, |ui| {
                match state.section {
                    SettingsSection::Appearance => section_appearance(ui, cx, &mut outcome),
                    SettingsSection::Terminal => section_terminal(ui, cx, &mut outcome),
                    SettingsSection::Profiles => section_profiles(ui, cx, state, &mut outcome),
                    SettingsSection::Workspace => section_git(ui, cx, state, &mut outcome),
                    SettingsSection::Hotkeys => section_hotkeys(ui, cx, &mut outcome),
                    SettingsSection::Claude => section_claude(ui, cx, &mut outcome),
                }
            });
        });
    });
    outcome
}

/// The font family typed by hand. While the field has focus the text is a
/// draft kept in egui's memory; it is returned on Enter or when the focus
/// leaves, if it differs from `current`. Applying every keystroke reinstalled
/// the fonts (100 MB of fallbacks once loaded) and rewrote config.json.
fn font_family_field(ui: &mut egui::Ui, id: egui::Id, current: &str) -> Option<String> {
    let mut family = ui.data_mut(|data| data.get_temp::<String>(id)).unwrap_or_else(|| current.to_owned());
    let response =
        ui.add(egui::TextEdit::singleline(&mut family).id(id).font(theme::field_font(13.0)).desired_width(180.0));
    if response.lost_focus() {
        ui.data_mut(|data| data.remove::<String>(id));
        let family = family.trim();
        return (!family.is_empty() && family != current).then(|| family.to_owned());
    }
    if response.has_focus() {
        ui.data_mut(|data| data.insert_temp(id, family));
    }
    None
}

fn section_appearance(ui: &mut egui::Ui, cx: &mut SettingsContext, outcome: &mut SettingsOutcome) {
    theme::section(ui, strings::SETTINGS_APPEARANCE);
    theme::tag(ui, strings::SETTINGS_FONT);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("font-family")
            .width(220.0)
            .selected_text(RichText::new(cx.config.font.family.clone()).font(theme::field_font(13.0)))
            .show_ui(ui, |ui| {
                for family in cx.fonts {
                    if ui.selectable_label(*family == cx.config.font.family, family).clicked() {
                        cx.config.font.family = family.clone();
                        outcome.changed = true;
                    }
                }
            });
        let id = ui.id().with("font-family-field");
        if let Some(family) = font_family_field(ui, id, &cx.config.font.family) {
            cx.config.font.family = family;
            outcome.changed = true;
        }
        if ui.add(theme::ghost_button(strings::SETTINGS_FONT_REFRESH)).clicked() {
            outcome.refresh_fonts = true;
        }
    });
    theme::tag(ui, strings::SETTINGS_FONT_SIZE);
    outcome.changed |= theme::stepper_f32(ui, &mut cx.config.font.size, 6.0, 48.0, 1.0);
    theme::tag(ui, strings::SETTINGS_SCHEME);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("color-scheme")
            .width(220.0)
            .selected_text(RichText::new(cx.config.color_scheme.clone()).font(theme::field_font(13.0)))
            .show_ui(ui, |ui| {
                let names = crate::app::scheme_names(cx.config);
                for name in &names {
                    if ui.selectable_label(*name == cx.config.color_scheme, name).clicked() {
                        cx.config.color_scheme = name.clone();
                        outcome.changed = true;
                    }
                }
            });
        let palette = crate::app::scheme_palette(cx.config);
        let (response, painter) = ui.allocate_painter(Vec2::new(16.0 * 15.0 + 2.0, 15.0), Sense::hover());
        painter.rect_stroke(response.rect, 0.0, Stroke::new(1.0, theme::colors().line), egui::StrokeKind::Middle);
        for (index, color) in palette.ansi.iter().enumerate() {
            let cell = Rect::from_min_size(
                response.rect.min + Vec2::new(1.0 + index as f32 * 15.0, 1.0),
                Vec2::new(14.0, 13.0),
            );
            painter.rect_filled(cell, 0.0, *color);
        }
    });
}

fn section_terminal(ui: &mut egui::Ui, cx: &mut SettingsContext, outcome: &mut SettingsOutcome) {
    theme::section(ui, strings::SETTINGS_TERMINAL);
    theme::tag(ui, strings::SETTINGS_SCROLLBACK);
    outcome.changed |= theme::stepper(ui, &mut cx.config.terminal.scrollback, 0, crate::config::MAX_SCROLLBACK, 1000);
    theme::tag(ui, strings::SETTINGS_CURSOR);
    ui.horizontal(|ui| {
        for (shape, label) in [
            (CursorShapeConfig::Block, strings::CURSOR_BLOCK),
            (CursorShapeConfig::Bar, strings::CURSOR_BAR),
            (CursorShapeConfig::Underline, strings::CURSOR_UNDERLINE),
        ] {
            if theme::choice(ui, label, cx.config.terminal.cursor.shape == shape).clicked() {
                cx.config.terminal.cursor.shape = shape;
                outcome.changed = true;
            }
        }
        ui.add_space(10.0);
        let blink = cx.config.terminal.cursor.blink;
        if theme::choice(ui, strings::SETTINGS_BLINK, blink).clicked() {
            cx.config.terminal.cursor.blink = !blink;
            outcome.changed = true;
        }
    });
    theme::tag(ui, strings::SETTINGS_RIGHT_CLICK);
    ui.horizontal(|ui| {
        for (mode, label) in [
            (RightClick::Clipboard, strings::RIGHT_CLICK_CLIPBOARD),
            (RightClick::Paste, strings::RIGHT_CLICK_PASTE),
            (RightClick::Menu, strings::RIGHT_CLICK_MENU),
        ] {
            if theme::choice(ui, label, cx.config.terminal.right_click == mode).clicked() {
                cx.config.terminal.right_click = mode;
                outcome.changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        let middle = cx.config.terminal.paste_on_middle_click;
        if theme::choice(ui, strings::SETTINGS_MIDDLE_CLICK, middle).clicked() {
            cx.config.terminal.paste_on_middle_click = !middle;
            outcome.changed = true;
        }
        ui.add_space(10.0);
        let copy = cx.config.terminal.copy_on_select;
        if theme::choice(ui, strings::SETTINGS_COPY_ON_SELECT, copy).clicked() {
            cx.config.terminal.copy_on_select = !copy;
            outcome.changed = true;
        }
    });
    if theme::choice(ui, strings::SETTINGS_ALLOW_OSC52, cx.config.terminal.allow_osc52).clicked() {
        cx.config.terminal.allow_osc52 = !cx.config.terminal.allow_osc52;
        outcome.changed = true;
    }
    theme::tag(ui, strings::SETTINGS_WORD_SEPARATORS);
    if ui
        .add(
            egui::TextEdit::singleline(&mut cx.config.terminal.word_separators)
                .font(theme::field_font(13.0))
                .desired_width(260.0),
        )
        .changed()
    {
        outcome.changed = true;
    }
}

fn section_profiles(ui: &mut egui::Ui, cx: &mut SettingsContext, state: &mut SettingsState, outcome: &mut SettingsOutcome) {
    theme::section(ui, strings::SETTINGS_PROFILES);
    theme::tag(ui, strings::SETTINGS_DEFAULT_PROFILE);
    egui::ComboBox::from_id_salt("default-profile")
        .width(260.0)
        .selected_text(RichText::new(cx.config.default_profile.clone()).font(theme::field_font(13.0)))
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
    if cx.config.profiles.is_empty() && !state.adding {
        ui.label(RichText::new(strings::SETTINGS_NO_PROFILES).color(theme::colors().faint).font(theme::font(12.0)));
    }
    let mut delete: Option<usize> = None;
    for (index, profile) in cx.config.profiles.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&profile.name).color(theme::colors().text).font(theme::font(12.5)));
            ui.label(RichText::new(&profile.command).color(theme::colors().faint).font(theme::field_font(12.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::ghost_button(strings::SETTINGS_DELETE)).clicked() {
                    delete = Some(index);
                }
                if ui.add(theme::ghost_button(strings::SETTINGS_EDIT)).clicked() {
                    state.editing = Some(index);
                    state.adding = false;
                    state.draft = profile.clone();
                }
            });
        });
        theme::hairline(ui);
    }
    if let Some(index) = delete {
        let removed = cx.config.profiles.remove(index);
        if cx.config.default_profile == removed.id {
            cx.config.default_profile = "git-bash".into();
        }
        // Keep the editor pointing at the same profile it was editing.
        match state.editing {
            Some(editing) if editing == index => {
                state.editing = None;
                state.adding = false;
            }
            Some(editing) if editing > index => state.editing = Some(editing - 1),
            _ => {}
        }
        outcome.changed = true;
    }
    if !state.adding && state.editing.is_none() && ui.add(theme::ghost_button(strings::SETTINGS_ADD)).clicked() {
        state.adding = true;
        state.editing = None;
        state.draft = draft_profile();
    }
    if state.adding || state.editing.is_some() {
        ui.add_space(6.0);
        profile_editor(ui, state, cx, outcome);
    }
}

fn section_git(ui: &mut egui::Ui, cx: &mut SettingsContext, state: &mut SettingsState, outcome: &mut SettingsOutcome) {
    theme::section(ui, strings::SETTINGS_WORKSPACE);
    theme::tag(ui, strings::SETTINGS_AI_COMMAND);
    let mut ai = cx.config.workspace.ai_commit_command.clone().unwrap_or_default();
    if ui
        .add(egui::TextEdit::singleline(&mut ai).font(theme::field_font(13.0)).desired_width(320.0))
        .changed()
    {
        let trimmed = ai.trim().to_owned();
        cx.config.workspace.ai_commit_command = (!trimmed.is_empty()).then_some(trimmed);
        outcome.changed = true;
    }
    ui.label(RichText::new(strings::SETTINGS_AI_HINT).color(theme::colors().faint).font(theme::font(11.5)));
    ui.add_space(14.0);
    theme::tag(ui, strings::SETTINGS_AI_MODEL);
    ui.label(RichText::new(strings::SETTINGS_AI_MODEL_HINT).color(theme::colors().faint).font(theme::font(11.5)));
    state.absorb_models();
    if matches!(state.model_catalog, ModelCatalog::NotLoaded) {
        state.load_models(ui.ctx());
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.model_filter)
                .font(theme::field_font(13.0))
                .hint_text(strings::SETTINGS_AI_MODEL_SEARCH)
                .desired_width(320.0),
        );
        if ui.add_enabled(!matches!(state.model_catalog, ModelCatalog::Loading(_)), theme::ghost_button(strings::SETTINGS_AI_MODEL_REFRESH)).clicked() {
            state.load_models(ui.ctx());
        }
    });
    match &state.model_catalog {
        ModelCatalog::Loading(_) => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(theme::colors().accent));
                ui.label(strings::SETTINGS_AI_MODEL_LOADING);
            });
        }
        ModelCatalog::Ready(Err(error)) => {
            ui.label(RichText::new(error).color(theme::colors().status_red).font(theme::font(11.5)));
        }
        ModelCatalog::Ready(Ok(models)) => {
            let query = state.model_filter.trim();
            let matching = models.iter().filter(|model| {
                query.is_empty() || model.as_bytes().windows(query.len()).any(|part| part.eq_ignore_ascii_case(query.as_bytes()))
            });
            let has_matches = matching.clone().next().is_some();
            egui::ComboBox::from_id_salt("ai-commit-model")
                .width(ui.available_width().min(560.0))
                .selected_text(cx.config.workspace.ai_commit_model.as_deref().unwrap_or(strings::SETTINGS_AI_MODEL_DEFAULT))
                .show_ui(ui, |ui| {
                    outcome.changed |= ui
                        .selectable_value(&mut cx.config.workspace.ai_commit_model, None, strings::SETTINGS_AI_MODEL_DEFAULT)
                        .changed();
                    for model in matching {
                        if ui.selectable_label(cx.config.workspace.ai_commit_model.as_deref() == Some(model.as_str()), model).clicked() {
                            cx.config.workspace.ai_commit_model = Some(model.clone());
                            cx.config.workspace.ai_commit_command = Some("opencode".to_owned());
                            outcome.changed = true;
                        }
                    }
                });
            if !has_matches && !query.is_empty() {
                ui.label(RichText::new(strings::SETTINGS_AI_MODEL_NO_MATCH).color(theme::colors().faint).font(theme::font(11.5)));
            }
        }
        ModelCatalog::NotLoaded => {}
    }
}

fn section_hotkeys(ui: &mut egui::Ui, cx: &mut SettingsContext, outcome: &mut SettingsOutcome) {
    theme::section(ui, strings::SETTINGS_HOTKEYS);
    ui.label(RichText::new(strings::SETTINGS_HOTKEYS_HINT).color(theme::colors().faint).font(theme::font(11.5)));
    ui.add_space(2.0);
    for (action, chords) in &cx.keymap_rows {
        ui.horizontal(|ui| {
            ui.label(RichText::new(action).color(theme::colors().dim).font(theme::font(12.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(chords.join(", ")).color(theme::colors().text).font(theme::field_font(12.0)));
            });
        });
        theme::hairline(ui);
    }
    ui.add_space(6.0);
    if ui.add(theme::ghost_button(strings::SETTINGS_OPEN_CONFIG)).clicked() {
        outcome.open_config = true;
    }
}

fn section_claude(ui: &mut egui::Ui, cx: &mut SettingsContext, outcome: &mut SettingsOutcome) {
    theme::section(ui, strings::SETTINGS_CLAUDE);
    let enabled = cx.config.claude_status.enabled;
    if theme::choice(ui, strings::SETTINGS_CLAUDE_ENABLED, enabled).clicked() {
        cx.config.claude_status.enabled = !enabled;
        outcome.changed = true;
        outcome.install_claude = !enabled;
    }
    let state_text = if cx.config.claude_status.declined_command.is_some() {
        RichText::new(strings::SETTINGS_CLAUDE_DECLINED).color(theme::colors().status_yellow)
    } else if enabled {
        RichText::new(if cx.config.claude_status.installed_command.is_some() {
            strings::SETTINGS_CLAUDE_CONNECTED
        } else {
            strings::SETTINGS_CLAUDE_PENDING
        }).color(theme::colors().status_green)
    } else {
        RichText::new(strings::SETTINGS_CLAUDE_NOT_CONNECTED).color(theme::colors().dim)
    };
    ui.label(state_text.font(theme::font(12.0)));
    ui.label(strings::SETTINGS_CLAUDE_GLOBAL_HINT);
    if enabled && ui.add(theme::ghost_button(strings::SETTINGS_CLAUDE_INSTALL)).clicked() {
        outcome.install_claude = true;
    }
    if !enabled && (cx.config.claude_status.installed_command.is_some() || cx.config.claude_status.previous_status_line.is_some())
        && ui.add(theme::ghost_button(strings::SETTINGS_CLAUDE_RESTORE)).clicked()
    {
        outcome.restore_claude = true;
    }
    ui.add_space(12.0);
}

/// Inline editor for one custom profile.
fn profile_editor(ui: &mut egui::Ui, state: &mut SettingsState, cx: &mut SettingsContext, outcome: &mut SettingsOutcome) {
    let draft = &mut state.draft;
    ui.painter().rect_stroke(
        ui.max_rect().shrink(1.0),
        0.0,
        Stroke::new(1.0, theme::colors().line),
        egui::StrokeKind::Middle,
    );
    ui.add_space(4.0);
    theme::tag(ui, strings::SETTINGS_NAME);
    ui.add(egui::TextEdit::singleline(&mut draft.name).font(theme::field_font(13.0)).desired_width(320.0));
    theme::tag(ui, strings::SETTINGS_COMMAND);
    ui.add(egui::TextEdit::singleline(&mut draft.command).font(theme::field_font(13.0)).desired_width(520.0));
    theme::tag(ui, strings::SETTINGS_ARGS);
    let mut args = join_args(&draft.args);
    if ui.add(egui::TextEdit::singleline(&mut args).font(theme::field_font(13.0)).desired_width(420.0)).changed() {
        draft.args = split_args(&args);
    }
    theme::tag(ui, strings::SETTINGS_CWD);
    let mut cwd = draft.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    if ui.add(egui::TextEdit::singleline(&mut cwd).font(theme::field_font(13.0)).desired_width(520.0)).changed() {
        draft.cwd = (!cwd.trim().is_empty()).then(|| PathBuf::from(cwd.trim()));
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add(theme::accent_button(strings::SETTINGS_SAVE)).clicked() && !draft.command.trim().is_empty() {
            if draft.name.trim().is_empty() {
                draft.name = draft.command.clone();
            }
            if draft.id.is_empty() {
                draft.id = unique_profile_id(cx.config, &draft.name);
            }
            match state.editing.and_then(|index| cx.config.profiles.get_mut(index)) {
                Some(slot) => *slot = draft.clone(),
                None => cx.config.profiles.push(draft.clone()),
            }
            state.adding = false;
            state.editing = None;
            outcome.changed = true;
        }
        if ui.add(theme::ghost_button(strings::SETTINGS_CANCEL)).clicked() {
            state.adding = false;
            state.editing = None;
        }
    });
    ui.add_space(4.0);
}

/// Quotes arguments that contain spaces, so the text field round-trips an
/// argument list instead of silently splitting `-File "C:\My Scripts\x.ps1"`.
fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| if arg.contains(' ') || arg.contains('"') { format!("\"{}\"", arg.replace('"', "'")) } else { arg.clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits on whitespace, keeping quoted runs together (quotes are dropped).
fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every keystroke in the family field used to reinstall the fonts (with
    /// the 100 MB of fallbacks once loaded) and rewrite config.json; the typed
    /// name is a draft until Enter or the field loses focus.
    #[test]
    fn the_font_family_applies_on_enter_not_per_keystroke() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let id = egui::Id::new("family-test");
        let frame = |events: Vec<egui::Event>| {
            let mut committed = None;
            let _ = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| committed = font_family_field(ui, id, "Consolas"));
            });
            committed
        };
        assert_eq!(frame(Vec::new()), None);
        ctx.memory_mut(|memory| memory.request_focus(id));
        assert_eq!(frame(Vec::new()), None);
        assert_eq!(frame(vec![egui::Event::Text(" X".to_owned())]), None, "typing is not applied");
        assert_eq!(frame(Vec::new()), None, "the draft survives the next frame");
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        assert_eq!(frame(vec![enter]), Some("Consolas X".to_owned()), "Enter applies the name");
    }

    #[test]
    fn arguments_round_trip_through_the_text_field() {
        let args = vec!["-NoLogo".to_owned(), "-File".to_owned(), r"C:\My Scripts\start.ps1".to_owned()];
        let text = join_args(&args);
        assert_eq!(text, r#"-NoLogo -File "C:\My Scripts\start.ps1""#);
        assert_eq!(split_args(&text), args);
        assert_eq!(split_args("  -a   -b  "), vec!["-a".to_owned(), "-b".to_owned()]);
        assert_eq!(split_args(""), Vec::<String>::new());
    }
}
