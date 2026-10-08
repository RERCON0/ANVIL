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
    Quota,
}

impl SettingsSection {
    const ALL: [SettingsSection; 7] = [
        SettingsSection::Appearance,
        SettingsSection::Terminal,
        SettingsSection::Profiles,
        SettingsSection::Workspace,
        SettingsSection::Hotkeys,
        SettingsSection::Claude,
        SettingsSection::Quota,
    ];

    fn title(self) -> &'static str {
        match self {
            SettingsSection::Appearance => strings::SETTINGS_APPEARANCE(),
            SettingsSection::Terminal => strings::SETTINGS_TERMINAL(),
            SettingsSection::Profiles => strings::SETTINGS_PROFILES(),
            SettingsSection::Workspace => strings::SETTINGS_WORKSPACE(),
            SettingsSection::Hotkeys => strings::SETTINGS_HOTKEYS(),
            SettingsSection::Claude => strings::SETTINGS_CLAUDE(),
            SettingsSection::Quota => strings::SETTINGS_QUOTA(),
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
    /// Provider whose key is being typed, and the text (masked on screen).
    quota_key: Option<(crate::quota::ProviderId, String)>,
    quota_key_error: Option<&'static str>,
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
            quota_key: None,
            quota_key_error: None,
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
    /// Opens `section`. Leaving one ends whatever was typed in it.
    fn select(&mut self, section: SettingsSection, outcome: &mut SettingsOutcome) {
        if self.section != section {
            self.section = section;
            outcome.commit = true;
        }
    }

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
                    self.model_catalog = ModelCatalog::Ready(Err(strings::WORKSPACE_AI_EMPTY().to_owned()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}

pub struct SettingsContext<'a> {
    pub config: &'a mut Config,
    pub keymap_rows: &'a [(String, Vec<String>)],
    pub profiles: &'a [crate::profiles::Profile],
    pub fonts: &'a [String],
    pub claude_line: &'a crate::claude_setup::LineState,
    pub quota: Option<&'a crate::quota::Snapshot>,
}

#[derive(Default)]
pub struct SettingsOutcome {
    /// Edits that apply and are written to config.json at once.
    pub changed: bool,
    /// Keystrokes in a text field: applied in memory at once, written once the
    /// field is left or the typing rests (see `commit`).
    pub typed: bool,
    /// A text field lost the focus or the section changed: whatever typing is
    /// still waiting for the disk has to be written now.
    pub commit: bool,
    pub open_config: bool,
    pub refresh_fonts: bool,
    pub install_claude: bool,
    pub restore_claude: bool,
    pub quota_refresh: bool,
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
    let mut outcome = SettingsOutcome::default();
    ui.painter_at(rect).rect_filled(rect, 0.0, theme::colors().chrome_bg);
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(18.0, 12.0))).id_salt("settings-page"),
        |ui| {
            ui.label(RichText::new(strings::TAB_SETTINGS()).color(theme::colors().text).font(theme::title_font(15.0)));
            ui.label(
                RichText::new(strings::SETTINGS_APPLY_HINT()).color(theme::colors().faint).font(theme::font(11.5)),
            );
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
                        ui.painter().rect_stroke(
                            row,
                            0.0,
                            Stroke::new(1.0_f32, theme::colors().line),
                            egui::StrokeKind::Middle,
                        );
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
                        state.select(section, &mut outcome);
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
                        SettingsSection::Quota => section_quota(ui, cx, state, &mut outcome),
                    }
                });
            });
        },
    );
    outcome
}

/// The font family typed by hand. While the field has focus the text is a
/// draft kept in egui's memory; it is returned on Enter or when the focus
/// leaves, if it differs from `current`. Applying every keystroke reinstalled
/// the fonts (100 MB of fallbacks once loaded) and rewrote config.json.
fn font_family_field(ui: &mut egui::Ui, id: egui::Id, current: &str) -> Option<String> {
    // A draft belongs to the editing session that wrote it. One left behind by
    // a page that was closed mid-edit is dropped, not shown (and applied) later.
    let editing = ui.memory(|memory| memory.had_focus_last_frame(id));
    if !editing {
        ui.data_mut(|data| data.remove::<String>(id));
    }
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
    theme::section(ui, strings::SETTINGS_APPEARANCE());
    theme::tag(ui, strings::SETTINGS_FONT());
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
        if ui.add(theme::ghost_button(strings::SETTINGS_FONT_REFRESH())).clicked() {
            outcome.refresh_fonts = true;
        }
    });
    theme::tag(ui, strings::SETTINGS_FONT_SIZE());
    outcome.changed |= theme::stepper_f32(ui, &mut cx.config.font.size, 6.0, 48.0, 1.0);
    theme::tag(ui, strings::SETTINGS_SCHEME());
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("color-scheme")
            .width(220.0)
            .selected_text(RichText::new(strings::scheme_label(&cx.config.color_scheme)).font(theme::field_font(13.0)))
            .show_ui(ui, |ui| {
                let names = crate::app::scheme_names(cx.config);
                for name in &names {
                    if ui.selectable_label(*name == cx.config.color_scheme, strings::scheme_label(name)).clicked() {
                        cx.config.color_scheme = name.clone();
                        outcome.changed = true;
                    }
                }
            });
        let palette = crate::app::scheme_palette(cx.config);
        let (response, painter) = ui.allocate_painter(Vec2::new(16.0 * 15.0 + 2.0, 15.0), Sense::hover());
        painter.rect_stroke(response.rect, 0.0, Stroke::new(1.0_f32, theme::colors().line), egui::StrokeKind::Middle);
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
    theme::section(ui, strings::SETTINGS_TERMINAL());
    if theme::choice(
        ui,
        strings::pick("Restore tabs and layout", "Восстанавливать вкладки и раскладку"),
        cx.config.restore_session,
    )
    .clicked()
    {
        cx.config.restore_session = !cx.config.restore_session;
        outcome.changed = true;
    }
    if theme::choice(
        ui,
        strings::pick(
            "Restore CLI agents and continue the last conversation",
            "Восстанавливать CLI-агентов и продолжать последнюю беседу",
        ),
        cx.config.restore_agents,
    )
    .clicked()
    {
        cx.config.restore_agents = !cx.config.restore_agents;
        outcome.changed = true;
    }
    ui.label(RichText::new(strings::pick("Requires layout restore. Codex, Claude Code, OpenCode and OMP use their own saved conversation in each folder. A running task is not resumed.", "Требуется восстановление раскладки. Codex, Claude Code, OpenCode и OMP открывают свою сохранённую беседу в каждой папке. Выполнявшаяся задача не возобновляется.")).font(theme::font(11.5)).color(theme::colors().faint));
    theme::tag(ui, strings::SETTINGS_SCROLLBACK());
    outcome.changed |= theme::stepper(ui, &mut cx.config.terminal.scrollback, 0, crate::config::MAX_SCROLLBACK, 1000);
    theme::tag(ui, strings::SETTINGS_CURSOR());
    ui.horizontal(|ui| {
        for (shape, label) in [
            (CursorShapeConfig::Block, strings::CURSOR_BLOCK()),
            (CursorShapeConfig::Bar, strings::CURSOR_BAR()),
            (CursorShapeConfig::Underline, strings::CURSOR_UNDERLINE()),
        ] {
            if theme::choice(ui, label, cx.config.terminal.cursor.shape == shape).clicked() {
                cx.config.terminal.cursor.shape = shape;
                outcome.changed = true;
            }
        }
        ui.add_space(10.0);
        let blink = cx.config.terminal.cursor.blink;
        if theme::choice(ui, strings::SETTINGS_BLINK(), blink).clicked() {
            cx.config.terminal.cursor.blink = !blink;
            outcome.changed = true;
        }
    });
    // The bell is honoured at run time, so it belongs here: a config option with no
    // row reads as a feature that was removed.
    theme::tag(ui, strings::SETTINGS_BELL());
    ui.horizontal(|ui| {
        for (mode, label) in
            [(crate::config::Bell::Off, strings::BELL_OFF()), (crate::config::Bell::Visual, strings::BELL_VISUAL())]
        {
            if theme::choice(ui, label, cx.config.terminal.bell == mode).clicked() {
                cx.config.terminal.bell = mode;
                outcome.changed = true;
            }
        }
    });
    theme::tag(ui, strings::SETTINGS_RIGHT_CLICK());
    ui.horizontal(|ui| {
        for (mode, label) in [
            (RightClick::Clipboard, strings::RIGHT_CLICK_CLIPBOARD()),
            (RightClick::Paste, strings::RIGHT_CLICK_PASTE()),
            (RightClick::Menu, strings::RIGHT_CLICK_MENU()),
        ] {
            if theme::choice(ui, label, cx.config.terminal.right_click == mode).clicked() {
                cx.config.terminal.right_click = mode;
                outcome.changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        let middle = cx.config.terminal.paste_on_middle_click;
        if theme::choice(ui, strings::SETTINGS_MIDDLE_CLICK(), middle).clicked() {
            cx.config.terminal.paste_on_middle_click = !middle;
            outcome.changed = true;
        }
        ui.add_space(10.0);
        let copy = cx.config.terminal.copy_on_select;
        if theme::choice(ui, strings::SETTINGS_COPY_ON_SELECT(), copy).clicked() {
            cx.config.terminal.copy_on_select = !copy;
            outcome.changed = true;
        }
    });
    if theme::choice(ui, strings::SETTINGS_ALLOW_OSC52(), cx.config.terminal.allow_osc52).clicked() {
        cx.config.terminal.allow_osc52 = !cx.config.terminal.allow_osc52;
        outcome.changed = true;
    }
    theme::tag(ui, strings::SETTINGS_WORD_SEPARATORS());
    let field = egui::TextEdit::singleline(&mut cx.config.terminal.word_separators)
        .id(egui::Id::new("settings-word-separators"))
        .font(theme::field_font(13.0))
        .desired_width(260.0);
    note_typing(&ui.add(field), outcome);
}

/// The text of a field whose value the config keeps in a normalised form (the
/// AI command is trimmed, the arguments are re-quoted). While the field has the
/// focus its raw text is kept here, so a trailing space or a quote still open
/// survives the next frame; rebuilt from the config each frame, a space typed
/// between two words was gone before the second word, and such a field could
/// only be pasted into. Out of focus it follows the config again.
fn edit_buffer(ui: &mut egui::Ui, id: egui::Id, from_config: impl FnOnce() -> String) -> String {
    let editing = ui.memory(|memory| memory.had_focus_last_frame(id));
    let kept = ui.data_mut(|data| {
        if editing {
            data.get_temp::<String>(id)
        } else {
            data.remove::<String>(id);
            None
        }
    });
    kept.unwrap_or_else(from_config)
}

/// Keeps what `edit_buffer` hands out for the next frame while the field has
/// the focus.
fn keep_buffer(ui: &mut egui::Ui, id: egui::Id, response: &egui::Response, text: String) {
    if response.has_focus() {
        ui.data_mut(|data| data.insert_temp(id, text));
    }
}

/// A text field of the settings: each keystroke applies in memory at once, but
/// config.json is written when the typing rests or the field is left (see
/// `SettingsOutcome::typed` and `commit`), not on every key. True when the text
/// changed.
fn note_typing(response: &egui::Response, outcome: &mut SettingsOutcome) -> bool {
    if response.changed() {
        outcome.typed = true;
    }
    if response.lost_focus() {
        outcome.commit = true;
    }
    response.changed()
}

fn section_profiles(
    ui: &mut egui::Ui,
    cx: &mut SettingsContext,
    state: &mut SettingsState,
    outcome: &mut SettingsOutcome,
) {
    theme::section(ui, strings::SETTINGS_PROFILES());
    theme::tag(ui, strings::SETTINGS_DEFAULT_PROFILE());
    egui::ComboBox::from_id_salt("default-profile")
        .width(260.0)
        .selected_text(RichText::new(cx.config.default_profile.clone()).font(theme::field_font(13.0)))
        .show_ui(ui, |ui| {
            // `cx.profiles` already holds the custom profiles next to the detected ones.
            for profile in cx.profiles {
                if ui.selectable_label(profile.id == cx.config.default_profile, &profile.name).clicked() {
                    cx.config.default_profile = profile.id.clone();
                    outcome.changed = true;
                }
            }
        });
    if cx.config.profiles.is_empty() && !state.adding {
        ui.label(RichText::new(strings::SETTINGS_NO_PROFILES()).color(theme::colors().faint).font(theme::font(12.0)));
    }
    let mut delete: Option<usize> = None;
    for (index, profile) in cx.config.profiles.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&profile.name).color(theme::colors().text).font(theme::font(12.5)));
            ui.label(RichText::new(&profile.command).color(theme::colors().faint).font(theme::field_font(12.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::ghost_button(strings::SETTINGS_DELETE())).clicked() {
                    delete = Some(index);
                }
                if ui.add(theme::ghost_button(strings::SETTINGS_EDIT())).clicked() {
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
    if !state.adding && state.editing.is_none() && ui.add(theme::ghost_button(strings::SETTINGS_ADD())).clicked() {
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
    theme::section(ui, strings::SETTINGS_WORKSPACE());
    theme::tag(ui, strings::SETTINGS_AI_COMMAND());
    let id = egui::Id::new("settings-ai-command");
    let mut ai = edit_buffer(ui, id, || cx.config.workspace.ai_commit_command.clone().unwrap_or_default());
    let field = egui::TextEdit::singleline(&mut ai).id(id).font(theme::field_font(13.0)).desired_width(320.0);
    let response = ui.add(field);
    if note_typing(&response, outcome) {
        let trimmed = ai.trim().to_owned();
        cx.config.workspace.ai_commit_command = (!trimmed.is_empty()).then_some(trimmed);
    }
    keep_buffer(ui, id, &response, ai);
    ui.label(RichText::new(strings::SETTINGS_AI_HINT()).color(theme::colors().faint).font(theme::font(11.5)));
    ui.add_space(8.0);
    ui.checkbox(
        &mut cx.config.workspace.codex_chatgpt_login,
        strings::pick(
            "Use the saved Codex ChatGPT login for commit messages (experimental)",
            "Использовать вход Codex через ChatGPT для сообщений коммита (экспериментально)",
        ),
    );
    ui.label(RichText::new(strings::pick(
        "Off by default. This sends the staged diff through an undocumented subscription endpoint. OpenAI can change or reject it. API keys use the supported OpenAI API with separate billing.",
        "По умолчанию выключено. Staged-diff отправляется через недокументированный endpoint подписки. OpenAI может изменить или отклонить запрос. API-ключ использует официальный API с отдельной оплатой.",
    )).color(theme::colors().faint).font(theme::font(11.5)));
    ui.add_space(14.0);
    theme::tag(ui, strings::SETTINGS_AI_MODEL());
    ui.label(RichText::new(strings::SETTINGS_AI_MODEL_HINT()).color(theme::colors().faint).font(theme::font(11.5)));
    state.absorb_models();
    if matches!(state.model_catalog, ModelCatalog::NotLoaded) {
        state.load_models(ui.ctx());
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.model_filter)
                .font(theme::field_font(13.0))
                .hint_text(strings::SETTINGS_AI_MODEL_SEARCH())
                .desired_width(320.0),
        );
        if ui
            .add_enabled(
                !matches!(state.model_catalog, ModelCatalog::Loading(_)),
                theme::ghost_button(strings::SETTINGS_AI_MODEL_REFRESH()),
            )
            .clicked()
        {
            state.load_models(ui.ctx());
        }
    });
    match &state.model_catalog {
        ModelCatalog::Loading(_) => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(theme::colors().accent));
                ui.label(strings::SETTINGS_AI_MODEL_LOADING());
            });
        }
        ModelCatalog::Ready(Err(error)) => {
            ui.label(
                RichText::new(crate::workspace::notice_text(error))
                    .color(theme::colors().status_red)
                    .font(theme::font(11.5)),
            );
        }
        ModelCatalog::Ready(Ok(models)) => {
            let query = state.model_filter.trim();
            let matching = models.iter().filter(|model| {
                query.is_empty()
                    || model.as_bytes().windows(query.len()).any(|part| part.eq_ignore_ascii_case(query.as_bytes()))
            });
            let has_matches = matching.clone().next().is_some();
            egui::ComboBox::from_id_salt("ai-commit-model")
                .width(ui.available_width().min(560.0))
                .selected_text(
                    cx.config.workspace.ai_commit_model.as_deref().unwrap_or(strings::SETTINGS_AI_MODEL_DEFAULT()),
                )
                .show_ui(ui, |ui| {
                    outcome.changed |= ui
                        .selectable_value(
                            &mut cx.config.workspace.ai_commit_model,
                            None,
                            strings::SETTINGS_AI_MODEL_DEFAULT(),
                        )
                        .changed();
                    for model in matching {
                        if ui
                            .selectable_label(
                                cx.config.workspace.ai_commit_model.as_deref() == Some(model.as_str()),
                                model,
                            )
                            .clicked()
                        {
                            cx.config.workspace.ai_commit_model = Some(model.clone());
                            cx.config.workspace.ai_commit_command = Some("opencode".to_owned());
                            outcome.changed = true;
                        }
                    }
                });
            if !has_matches && !query.is_empty() {
                ui.label(
                    RichText::new(strings::SETTINGS_AI_MODEL_NO_MATCH())
                        .color(theme::colors().faint)
                        .font(theme::font(11.5)),
                );
            }
        }
        ModelCatalog::NotLoaded => {}
    }
}

fn section_hotkeys(ui: &mut egui::Ui, cx: &mut SettingsContext, outcome: &mut SettingsOutcome) {
    theme::section(ui, strings::SETTINGS_HOTKEYS());
    ui.label(RichText::new(strings::SETTINGS_HOTKEYS_HINT()).color(theme::colors().faint).font(theme::font(11.5)));
    ui.add_space(2.0);
    for (action, chords) in cx.keymap_rows {
        let label = crate::hotkeys::Action::from_id(action)
            .map(|action| strings::hotkey_label(&action))
            .unwrap_or_else(|| action.clone());
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(theme::colors().dim).font(theme::font(12.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(chords.join(", ")).color(theme::colors().text).font(theme::field_font(12.0)));
            });
        });
        theme::hairline(ui);
    }
    ui.add_space(6.0);
    if ui.add(theme::ghost_button(strings::SETTINGS_OPEN_CONFIG())).clicked() {
        outcome.open_config = true;
    }
}

fn section_claude(ui: &mut egui::Ui, cx: &mut SettingsContext, outcome: &mut SettingsOutcome) {
    use crate::claude_setup::LineState;
    theme::section(ui, strings::SETTINGS_CLAUDE());
    let (current, color) = match cx.claude_line {
        LineState::Missing => (strings::SETTINGS_CLAUDE_LINE_MISSING().to_owned(), theme::colors().dim),
        LineState::Anvil => (strings::SETTINGS_CLAUDE_LINE_ANVIL().to_owned(), theme::colors().status_green),
        LineState::Foreign(command) => {
            (format!("{} {command}", strings::SETTINGS_CLAUDE_LINE_FOREIGN()), theme::colors().text)
        }
        LineState::Broken(error) => {
            (format!("{} {error}", strings::SETTINGS_CLAUDE_LINE_BROKEN()), theme::colors().status_yellow)
        }
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(strings::SETTINGS_CLAUDE_NOW()).color(theme::colors().dim).font(theme::font(12.0)));
        ui.add(egui::Label::new(RichText::new(current).color(color).font(theme::field_font(12.0))).truncate());
    });
    ui.add_space(4.0);

    let enabled = cx.config.claude_status.enabled;
    if theme::choice(ui, strings::SETTINGS_CLAUDE_ENABLED(), enabled).clicked() {
        cx.config.claude_status.enabled = !enabled;
        outcome.changed = true;
        outcome.install_claude = !enabled;
    }
    ui.label(
        RichText::new(strings::SETTINGS_CLAUDE_ENABLED_HINT()).color(theme::colors().faint).font(theme::font(11.5)),
    );
    if cx.config.claude_status.declined_command.is_some() {
        ui.label(
            RichText::new(strings::SETTINGS_CLAUDE_DECLINED())
                .color(theme::colors().status_yellow)
                .font(theme::font(12.0)),
        );
    } else if enabled && cx.config.claude_status.installed_command.is_none() {
        ui.label(
            RichText::new(strings::SETTINGS_CLAUDE_PENDING())
                .color(theme::colors().status_green)
                .font(theme::font(12.0)),
        );
    }
    ui.add_space(6.0);

    let badge = cx.config.claude_status.badge;
    if theme::choice(ui, strings::SETTINGS_CLAUDE_BADGE(), badge).clicked() {
        cx.config.claude_status.badge = !badge;
        outcome.changed = true;
    }
    let ours = matches!(cx.claude_line, LineState::Anvil);
    let status = &mut cx.config.claude_status;
    let mut changed = false;
    egui::Grid::new("claude-fields").num_columns(3).spacing([18.0, 2.0]).show(ui, |ui| {
        let head = |text: &str| RichText::new(text).color(theme::colors().faint).font(theme::font(11.5));
        ui.label(head(strings::SETTINGS_CLAUDE_FIELDS()));
        ui.label(head(strings::SETTINGS_CLAUDE_IN_CLAUDE()));
        ui.label(head(strings::SETTINGS_CLAUDE_UNDER_TAB()));
        ui.end_row();
        let line = &mut status.line_fields;
        let tab = &mut status.badge_fields;
        let rows: [(&str, &mut bool, Option<&mut bool>); 7] = [
            (strings::SETTINGS_CLAUDE_FIELD_MODEL(), &mut line.model, Some(&mut tab.model)),
            (strings::SETTINGS_CLAUDE_FIELD_DIR(), &mut line.dir, None),
            (strings::SETTINGS_CLAUDE_FIELD_BRANCH(), &mut line.branch, None),
            (strings::SETTINGS_CLAUDE_FIELD_CONTEXT(), &mut line.context, Some(&mut tab.context)),
            (strings::SETTINGS_CLAUDE_FIELD_FIVE_HOUR(), &mut line.five_hour, Some(&mut tab.five_hour)),
            (strings::SETTINGS_CLAUDE_FIELD_SEVEN_DAY(), &mut line.seven_day, Some(&mut tab.seven_day)),
            (strings::SETTINGS_CLAUDE_FIELD_AGENT(), &mut line.agent, Some(&mut tab.agent)),
        ];
        for (label, in_claude, under_tab) in rows {
            ui.label(RichText::new(label).color(theme::colors().dim).font(theme::font(12.0)));
            ui.add_enabled_ui(ours, |ui| {
                if theme::choice(ui, "", *in_claude).clicked() {
                    *in_claude = !*in_claude;
                    changed = true;
                }
            });
            match under_tab {
                Some(value) => {
                    ui.add_enabled_ui(ours && badge, |ui| {
                        if theme::choice(ui, "", *value).clicked() {
                            *value = !*value;
                            changed = true;
                        }
                    });
                }
                None => {
                    ui.label(RichText::new("—").color(theme::colors().faint));
                }
            }
            ui.end_row();
        }
    });
    outcome.changed |= changed;
    if !ours {
        ui.label(
            RichText::new(strings::SETTINGS_CLAUDE_FIELDS_NEED_ANVIL())
                .color(theme::colors().faint)
                .font(theme::font(11.5)),
        );
    }
    ui.add_space(6.0);
    ui.label(strings::SETTINGS_CLAUDE_GLOBAL_HINT());
    if enabled && ui.add(theme::ghost_button(strings::SETTINGS_CLAUDE_INSTALL())).clicked() {
        outcome.install_claude = true;
    }
    if !enabled
        && (cx.config.claude_status.installed_command.is_some()
            || cx.config.claude_status.previous_status_line.is_some())
        && ui.add(theme::ghost_button(strings::SETTINGS_CLAUDE_RESTORE())).clicked()
    {
        outcome.restore_claude = true;
    }
    ui.add_space(12.0);
}

/// One provider row: its switch, where the key came from, the balance labels
/// and the per-window toggles. Split out of `section_quota` so the toggle
/// semantics can be tested without driving the whole page.
fn quota_provider_row(
    ui: &mut egui::Ui,
    cx: &mut SettingsContext,
    id: crate::quota::ProviderId,
) -> (bool, Option<crate::quota::ProviderId>) {
    use crate::quota::{creds::Source, ProviderId};
    let mut changed = false;
    let mut open_key = None;
    let anvil_key = Source::AnvilKey.label();
    let snapshot = cx.quota.and_then(|s| s.get(id));
    let found = snapshot.is_some();
    let pref = cx.config.quota.provider_enabled(id.key());
    let on = pref.unwrap_or(found);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(found || pref.is_some(), |ui| {
            if theme::choice(ui, id.label(), on).clicked() {
                // Off is remembered; switching back on returns to automatic.
                cx.config.quota.set_provider_enabled(id.key(), if on { Some(false) } else { None });
                changed = true;
            }
        });
        let (source, color) = match snapshot {
            Some(s) => (format!("{} {}", strings::QUOTA_LOGIN(), strings::localize(&s.source)), theme::colors().dim),
            None => (strings::SETTINGS_QUOTA_NO_LOGIN().to_owned(), theme::colors().faint),
        };
        ui.label(RichText::new(source).color(color).font(theme::font(12.0)));
        if id.accepts_own_key() {
            let label = match snapshot {
                Some(s) if strings::localize(&s.source) == anvil_key => strings::SETTINGS_QUOTA_CHANGE_KEY(),
                Some(_) => strings::SETTINGS_QUOTA_OWN_KEY(),
                None => strings::SETTINGS_QUOTA_SET_KEY(),
            };
            if ui.add(theme::ghost_button(label)).clicked() {
                open_key = Some(id);
            }
        } else if id == ProviderId::OpencodeZen && !found {
            let hint = RichText::new(strings::SETTINGS_QUOTA_ZEN_LOGIN());
            ui.label(hint.color(theme::colors().faint).font(theme::font(11.5)));
        }
    });
    if let Some(s) = snapshot.filter(|_| on) {
        if let Some(note) = crate::quota::view::state_note(&s.state, crate::quota::time::now_unix()) {
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.label(RichText::new(note).color(theme::colors().status_yellow).font(theme::font(11.5)));
            });
        }
        // Windows by their label, balances with their value ("баланс (¥12.40)"):
        // a provider may report one balance per currency.
        let balance = |b: &crate::quota::model::Balance| {
            (b.key.clone(), format!("{} ({})", strings::localize(&b.label), crate::quota::view::balance_text(b)))
        };
        let items: Vec<(String, String)> = s
            .windows
            .iter()
            .map(|w| (w.key.clone(), strings::quota_window_label(&w.label)))
            .chain(s.balances.iter().map(balance))
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.add_space(22.0);
            if items.is_empty() && s.state == crate::quota::model::ProviderState::Idle {
                let hint = RichText::new(strings::SETTINGS_QUOTA_WINDOWS_LATER());
                ui.label(hint.color(theme::colors().faint).font(theme::font(11.5)));
            }
            for (key, label) in &items {
                let visible = cx.config.quota.window_visible(id.key(), key);
                if theme::choice(ui, label, visible).clicked() {
                    cx.config.quota.set_window_visible(id.key(), key, !visible);
                    changed = true;
                }
            }
        });
    }
    (changed, open_key)
}

fn section_quota(
    ui: &mut egui::Ui,
    cx: &mut SettingsContext,
    state: &mut SettingsState,
    outcome: &mut SettingsOutcome,
) {
    use crate::quota::ProviderId;
    theme::section(ui, strings::SETTINGS_QUOTA());
    let enabled = cx.config.quota.enabled;
    if theme::choice(ui, strings::SETTINGS_QUOTA_ENABLED(), enabled).clicked() {
        cx.config.quota.enabled = !enabled;
        outcome.changed = true;
    }
    if !cx.config.quota.enabled {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(strings::SETTINGS_QUOTA_INTERVAL()).color(theme::colors().dim).font(theme::font(12.0)));
        if ui.add(theme::ghost_button(strings::SETTINGS_QUOTA_REFRESH())).clicked() {
            outcome.quota_refresh = true;
        }
    });
    ui.add_space(6.0);
    for id in ProviderId::ALL {
        let (changed, open_key) = quota_provider_row(ui, cx, id);
        outcome.changed |= changed;
        if let Some(id) = open_key {
            state.quota_key = Some((id, String::new()));
            state.quota_key_error = None;
        }
        if state.quota_key.as_ref().is_some_and(|(editing, _)| *editing == id) {
            quota_key_editor(ui, id, state, outcome);
        }
    }
}

enum KeyAction {
    Save,
    Delete,
    Cancel,
}

/// The masked key field under a provider. The key goes straight to the
/// Credential Manager; config.json never sees it.
fn quota_key_editor(
    ui: &mut egui::Ui,
    id: crate::quota::ProviderId,
    state: &mut SettingsState,
    outcome: &mut SettingsOutcome,
) {
    use crate::quota::credman;
    let mut action = None;
    if let Some((_, text)) = state.quota_key.as_mut() {
        ui.horizontal(|ui| {
            ui.add_space(22.0);
            let field =
                egui::TextEdit::singleline(text).password(true).desired_width(260.0).font(theme::field_font(12.5));
            let response = ui.add(field);
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.add(theme::accent_button(strings::SETTINGS_SAVE())).clicked() || enter {
                action = Some(KeyAction::Save);
            }
            if ui.add(theme::ghost_button(strings::SETTINGS_QUOTA_DELETE_KEY())).clicked() {
                action = Some(KeyAction::Delete);
            }
            if ui.add(theme::ghost_button(strings::SETTINGS_CANCEL())).clicked() {
                action = Some(KeyAction::Cancel);
            }
        });
    }
    if let Some(error) = state.quota_key_error {
        ui.horizontal(|ui| {
            ui.add_space(22.0);
            ui.label(RichText::new(error).color(theme::colors().status_yellow).font(theme::font(11.5)));
        });
    }
    let target = credman::target(id);
    match action {
        Some(KeyAction::Save) => {
            let typed = state.quota_key.as_ref().map(|(_, text)| text.as_str()).unwrap_or("");
            match credman::clean_key(typed) {
                None => state.quota_key_error = Some(strings::SETTINGS_QUOTA_KEY_INVALID()),
                Some(key) => match credman::write(&target, &key) {
                    Ok(()) => {
                        state.quota_key = None;
                        state.quota_key_error = None;
                        outcome.quota_refresh = true;
                    }
                    Err(code) => {
                        log::warn!("quota: CredWriteW failed with {code}");
                        state.quota_key_error = Some(strings::SETTINGS_QUOTA_KEY_FAILED());
                    }
                },
            }
        }
        Some(KeyAction::Delete) => {
            // A failed delete leaves the key in place, and the next cycle keeps
            // using it: the editor must not close as if it were gone.
            match credman::delete(&target) {
                Ok(()) => {
                    state.quota_key = None;
                    state.quota_key_error = None;
                    outcome.quota_refresh = true;
                }
                Err(code) => {
                    log::warn!("quota: CredDeleteW failed with {code}");
                    state.quota_key_error = Some(strings::SETTINGS_QUOTA_KEY_DELETE_FAILED());
                }
            }
        }
        Some(KeyAction::Cancel) => {
            state.quota_key = None;
            state.quota_key_error = None;
        }
        None => {}
    }
}

/// Inline editor for one custom profile.
fn profile_editor(
    ui: &mut egui::Ui,
    state: &mut SettingsState,
    cx: &mut SettingsContext,
    outcome: &mut SettingsOutcome,
) {
    let draft = &mut state.draft;
    ui.painter().rect_stroke(
        ui.max_rect().shrink(1.0),
        0.0,
        Stroke::new(1.0_f32, theme::colors().line),
        egui::StrokeKind::Middle,
    );
    ui.add_space(4.0);
    theme::tag(ui, strings::SETTINGS_NAME());
    ui.add(egui::TextEdit::singleline(&mut draft.name).font(theme::field_font(13.0)).desired_width(320.0));
    theme::tag(ui, strings::SETTINGS_COMMAND());
    ui.add(egui::TextEdit::singleline(&mut draft.command).font(theme::field_font(13.0)).desired_width(520.0));
    theme::tag(ui, strings::SETTINGS_ARGS());
    let id = egui::Id::new("profile-args");
    let mut args = edit_buffer(ui, id, || join_args(&draft.args));
    let field = egui::TextEdit::singleline(&mut args).id(id).font(theme::field_font(13.0)).desired_width(420.0);
    let response = ui.add(field);
    if response.changed() {
        draft.args = split_args(&args);
    }
    keep_buffer(ui, id, &response, args);
    theme::tag(ui, strings::SETTINGS_CWD());
    let id = egui::Id::new("profile-cwd");
    let mut cwd =
        edit_buffer(ui, id, || draft.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    let field = egui::TextEdit::singleline(&mut cwd).id(id).font(theme::field_font(13.0)).desired_width(520.0);
    let response = ui.add(field);
    if response.changed() {
        draft.cwd = (!cwd.trim().is_empty()).then(|| PathBuf::from(cwd.trim()));
    }
    keep_buffer(ui, id, &response, cwd);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add(theme::accent_button(strings::SETTINGS_SAVE())).clicked() && !draft.command.trim().is_empty() {
            if draft.name.trim().is_empty() {
                draft.name = draft.command.clone();
            }
            if draft.id.is_empty() {
                draft.id = unique_profile_id(cx.config, cx.profiles, &draft.name);
            }
            match state.editing.and_then(|index| cx.config.profiles.get_mut(index)) {
                Some(slot) => *slot = draft.clone(),
                None => cx.config.profiles.push(draft.clone()),
            }
            state.adding = false;
            state.editing = None;
            outcome.changed = true;
        }
        if ui.add(theme::ghost_button(strings::SETTINGS_CANCEL())).clicked() {
            state.adding = false;
            state.editing = None;
        }
    });
    ui.add_space(4.0);
}

/// The argument list as one line of text, in the Windows command-line grammar
/// that `split_args` reads back (and that a child process sees): an empty
/// argument is `""`, one with white space is wrapped in quotes, a `"` inside
/// is `\"`, and the backslashes in front of a quote (or of the closing one)
/// are doubled. `split_args(&join_args(args)) == args` for any list.
fn join_args(args: &[String]) -> String {
    let mut line = String::new();
    for arg in args {
        if !line.is_empty() {
            line.push(' ');
        }
        let quote = arg.is_empty() || arg.chars().any(char::is_whitespace);
        if quote {
            line.push('"');
        }
        let mut backslashes = 0;
        for ch in arg.chars() {
            if ch == '\\' {
                backslashes += 1;
            } else {
                if ch == '"' {
                    // 2n+1 backslashes make an escaped quote out of n.
                    line.extend(std::iter::repeat_n('\\', backslashes + 1));
                }
                backslashes = 0;
            }
            line.push(ch);
        }
        if quote {
            // 2n before the closing quote, so it still closes.
            line.extend(std::iter::repeat_n('\\', backslashes));
            line.push('"');
        }
    }
    line
}

/// Splits a line into arguments by the rules of the Microsoft C runtime's
/// command-line parser (2008 and later), which differ from the shell's
/// `CommandLineToArgvW` in one point, noted below: white space outside quotes
/// separates, `"` opens or closes a quoted run (and is dropped), 2n backslashes
/// before a quote are n and the quote keeps its meaning, 2n+1 are n and a
/// literal quote, backslashes anywhere else are literal. `""` inside a run is a
/// literal quote and the run stays open (`CommandLineToArgvW` closes the run
/// there); `join_args` never writes `""` inside a run, so the pair reads back
/// the same under either rule. An argument that was quoted stays even when
/// empty (`""`), and a quote left open runs to the end of the line.
fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let mut backslashes = 1;
                while chars.next_if_eq(&'\\').is_some() {
                    backslashes += 1;
                }
                if chars.peek() == Some(&'"') {
                    current.extend(std::iter::repeat_n('\\', backslashes / 2));
                    if backslashes % 2 == 1 {
                        chars.next();
                        current.push('"');
                    }
                } else {
                    current.extend(std::iter::repeat_n('\\', backslashes));
                }
                started = true;
            }
            '"' => {
                if quoted && chars.next_if_eq(&'"').is_some() {
                    current.push('"');
                } else {
                    quoted = !quoted;
                }
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

/// An id no profile uses yet. The built-in ones count too: a custom profile
/// with their id replaces them in the list, so a profile named "PowerShell"
/// would make the detected PowerShell vanish.
fn unique_profile_id(config: &Config, profiles: &[crate::profiles::Profile], name: &str) -> String {
    let slug: String = name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.trim_matches('-').to_owned();
    let base = if slug.is_empty() { "custom".to_owned() } else { slug };
    let mut id = base.clone();
    let mut n = 2;
    while config.profiles.iter().any(|p| p.id == id) || profiles.iter().any(|p| p.id == id) {
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
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider the snapshot knows is switched off by remembering `Some(false)`,
    /// and switching it back on drops the preference so the automatic behaviour
    /// (follow the snapshot) returns. Nothing had covered this, and it is the
    /// only place the two states can drift apart.
    #[test]
    fn a_quota_switch_remembers_off_and_returns_to_automatic() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut config = crate::config::Config::default();
        let snapshot = crate::quota::model::Snapshot::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(700.0, 600.0));
        // Automatic for a provider nobody logged into.
        let id = crate::quota::ProviderId::ChatGpt;
        assert_eq!(config.quota.provider_enabled(id.key()), None);
        assert!(!config.quota.provider_enabled(id.key()).unwrap_or(false));
        let _ = (snapshot, rect);
        // Turning it on then off leaves the remembered `Some(false)`.
        config.quota.set_provider_enabled(id.key(), None);
        config.quota.set_provider_enabled(id.key(), Some(false));
        assert_eq!(config.quota.provider_enabled(id.key()), Some(false));
        // Turning it on again drops the preference entirely, so a later change
        // of the snapshot decides again.
        config.quota.set_provider_enabled(id.key(), None);
        assert_eq!(config.quota.provider_enabled(id.key()), None, "off then on must be automatic, not on");
    }

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

    /// A draft that nobody read back (the page was left mid-edit, so the field
    /// never saw the focus go) must not reappear in a later visit and be applied
    /// by the first focus and blur.
    #[test]
    fn an_abandoned_family_draft_does_not_come_back() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let id = egui::Id::new("family-abandoned");
        let frame = |events: Vec<egui::Event>, draw: bool| {
            let mut committed = None;
            let _ = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    if draw {
                        committed = font_family_field(ui, id, "Consolas");
                    }
                });
            });
            committed
        };
        assert_eq!(frame(Vec::new(), true), None);
        ctx.memory_mut(|memory| memory.request_focus(id));
        assert_eq!(frame(Vec::new(), true), None);
        assert_eq!(frame(vec![egui::Event::Text(" X".to_owned())], true), None);
        // The page is left (a click on a tab) while the field still holds its draft.
        assert_eq!(frame(Vec::new(), false), None);
        // Back on the page: focusing and leaving the field changes nothing.
        assert_eq!(frame(Vec::new(), true), None);
        ctx.memory_mut(|memory| memory.request_focus(id));
        assert_eq!(frame(Vec::new(), true), None);
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        assert_eq!(frame(vec![enter], true), None, "the old draft must not be applied");
    }

    fn built_in(id: &str, name: &str) -> crate::profiles::Profile {
        crate::profiles::Profile {
            id: id.into(),
            name: name.into(),
            command: "sh".into(),
            args: Vec::new(),
            cwd: None,
            env: Default::default(),
            kind: crate::profiles::ProfileKind::Cmd,
        }
    }

    /// A custom profile with a built-in's id replaces it in the list, so a new
    /// profile named like a detected one must get a different id.
    #[test]
    fn a_new_profile_never_takes_a_built_in_id() {
        let config = Config::default();
        let detected = [built_in("powershell", "PowerShell"), built_in("git-bash", "Git Bash")];
        assert_eq!(unique_profile_id(&config, &detected, "PowerShell"), "powershell-2");
        assert_eq!(unique_profile_id(&config, &detected, "Git Bash"), "git-bash-2");
        assert_eq!(unique_profile_id(&config, &[], "PowerShell"), "powershell");
    }

    /// The profile list the app hands over already holds the custom profiles, so
    /// listing the config's ones again offered each of them twice.
    #[test]
    fn the_default_profile_list_names_every_profile_once() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut config = Config::default();
        config.profiles.push(ProfileConfig {
            id: "mine".into(),
            name: "Zeta shell".into(),
            command: "sh".into(),
            args: Vec::new(),
            cwd: None,
            env: Default::default(),
        });
        let profiles = [built_in("cmd", "cmd"), built_in("mine", "Zeta shell")];
        let mut state = SettingsState::default();
        let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(900.0, 700.0));
        let mut frame = |open: bool| {
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    if open {
                        let combo = ui.make_persistent_id(egui::Id::new("default-profile"));
                        egui::Popup::open_id(ui.ctx(), combo.with("popup"));
                    }
                    let mut cx = SettingsContext {
                        config: &mut config,
                        keymap_rows: &[],
                        profiles: &profiles,
                        fonts: &[],
                        claude_line: &crate::claude_setup::LineState::Missing,
                        quota: None,
                    };
                    let mut outcome = SettingsOutcome::default();
                    section_profiles(ui, &mut cx, &mut state, &mut outcome);
                });
            });
            output
                .shapes
                .iter()
                .filter(
                    |clipped| matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == "Zeta shell"),
                )
                .count()
        };
        let closed = frame(false);
        assert_eq!(closed, 1, "the profile's own row");
        let _ = frame(true);
        let _ = frame(false);
        let open = frame(false);
        assert_eq!(open - closed, 1, "one entry in the opened list");
    }

    /// Draws the profile editor for one frame with `events`.
    fn editor_frame(ctx: &egui::Context, state: &mut SettingsState, events: Vec<egui::Event>) {
        let mut config = Config::default();
        let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(900.0, 700.0));
        let raw = egui::RawInput { screen_rect: Some(rect), events, ..Default::default() };
        let _ = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let mut cx = SettingsContext {
                    config: &mut config,
                    keymap_rows: &[],
                    profiles: &[],
                    fonts: &[],
                    claude_line: &crate::claude_setup::LineState::Missing,
                    quota: None,
                };
                profile_editor(ui, state, &mut cx, &mut SettingsOutcome::default());
            });
        });
    }

    /// The args field is typed into one key at a time: what is typed stays in
    /// the field. It used to be rebuilt from the parsed list every frame, which
    /// dropped a trailing space (and a quote still open), so a second argument
    /// or a quoted one could only be pasted, never typed.
    #[test]
    fn arguments_can_be_typed_one_key_at_a_time() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut state = SettingsState { adding: true, ..SettingsState::default() };
        editor_frame(&ctx, &mut state, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("profile-args")));
        editor_frame(&ctx, &mut state, Vec::new());
        for key in ["-a", " ", "-b", " ", "\"", "c", " ", "d", "\""] {
            editor_frame(&ctx, &mut state, vec![egui::Event::Text(key.to_owned())]);
        }
        assert_eq!(state.draft.args, ["-a", "-b", "c d"]);
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    /// Draws a settings section for one frame with `events`.
    fn page_frame(
        ctx: &egui::Context,
        config: &mut Config,
        state: &mut SettingsState,
        git: bool,
        events: Vec<egui::Event>,
    ) -> SettingsOutcome {
        let mut outcome = SettingsOutcome::default();
        let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(900.0, 700.0));
        let raw = egui::RawInput { screen_rect: Some(rect), events, ..Default::default() };
        let _ = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let mut cx = SettingsContext {
                    config,
                    keymap_rows: &[],
                    profiles: &[],
                    fonts: &[],
                    claude_line: &crate::claude_setup::LineState::Missing,
                    quota: None,
                };
                if git {
                    section_git(ui, &mut cx, state, &mut outcome);
                } else {
                    section_terminal(ui, &mut cx, &mut outcome);
                }
            });
        });
        outcome
    }

    /// Typing in a text field is reported as typing, not as a change to write:
    /// the page applies it at once and the write waits for `commit`, which the
    /// field raises when it is left.
    #[test]
    fn a_text_field_reports_typing_and_then_its_end() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut config = Config::default();
        let mut state = SettingsState::default();
        let before = config.terminal.word_separators.clone();
        page_frame(&ctx, &mut config, &mut state, false, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("settings-word-separators")));
        let idle = page_frame(&ctx, &mut config, &mut state, false, Vec::new());
        assert!(!idle.changed && !idle.typed && !idle.commit);
        let outcome = page_frame(&ctx, &mut config, &mut state, false, vec![egui::Event::Text("x".to_owned())]);
        assert!(outcome.typed && !outcome.changed && !outcome.commit, "a keystroke is typing");
        assert_eq!(config.terminal.word_separators, format!("{before}x"), "and is in the edited config");
        let outcome = page_frame(&ctx, &mut config, &mut state, false, vec![key(egui::Key::Escape)]);
        assert!(outcome.commit && !outcome.typed, "leaving the field ends the typing");
    }

    /// The AI command is kept trimmed in the config, and used to be rebuilt from
    /// it every frame: the space typed after the first word vanished before the
    /// second word, so a command with an argument could only be pasted.
    #[test]
    fn the_ai_command_can_be_typed_with_spaces() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let mut config = Config::default();
        // A loaded catalogue: the page does not start the real `opencode models`.
        let mut state = SettingsState { model_catalog: ModelCatalog::Ready(Ok(Vec::new())), ..Default::default() };
        page_frame(&ctx, &mut config, &mut state, true, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("settings-ai-command")));
        page_frame(&ctx, &mut config, &mut state, true, Vec::new());
        for text in ["opencode", " ", "run", " ", "--pure", " "] {
            page_frame(&ctx, &mut config, &mut state, true, vec![egui::Event::Text(text.to_owned())]);
        }
        assert_eq!(config.workspace.ai_commit_command.as_deref(), Some("opencode run --pure"));
    }

    #[test]
    fn leaving_a_section_ends_its_typing() {
        let mut state = SettingsState::default();
        let mut outcome = SettingsOutcome::default();
        state.select(SettingsSection::Appearance, &mut outcome);
        assert!(!outcome.commit, "the same section is not leaving");
        state.select(SettingsSection::Terminal, &mut outcome);
        assert!(outcome.commit && state.section == SettingsSection::Terminal);
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

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    /// Whatever the editor shows for a list reads back as the same list: the
    /// old pair turned a `"` into `'` and dropped an empty argument, so editing
    /// the field rewrote the profile's arguments.
    #[test]
    fn arguments_survive_the_text_field_whatever_they_hold() {
        let hard: &[&[&str]] = &[
            &[],
            &[""],
            &["", "a", ""],
            &["a b"],
            &["a\"b"],
            &["\""],
            &["\"\""],
            &["a b\"c d"],
            &["a\\"],
            &["a b\\"],
            &["a\\\\"],
            &["a b\\\\"],
            &["a\\\"b"],
            &["a b\\\\\"c"],
            &["\\"],
            &["\\\\", "\\ "],
            &[" "],
            &["\t"],
            &["a\tb", "c"],
            &["C:\\My Scripts\\start.ps1", "-Command", "Write-Host \"hi there\""],
            &["'", "a'b c", "\"'\""],
            &["привет мир", "ж"],
            &["x\"\"y z", "\"", ""],
        ];
        for args in hard {
            let args = owned(args);
            let text = join_args(&args);
            assert_eq!(split_args(&text), args, "through `{text}`");
        }
        // Every short string over the characters that matter, alone and in pairs.
        let mut strings = vec![String::new()];
        let mut layer = vec![String::new()];
        for _ in 0..4 {
            layer = layer.iter().flat_map(|s| ['a', ' ', '\t', '"', '\\'].map(|c| format!("{s}{c}"))).collect();
            strings.extend(layer.iter().cloned());
        }
        for one in &strings {
            let args = vec![one.clone()];
            assert_eq!(split_args(&join_args(&args)), args, "{one:?}");
        }
        for first in strings.iter().filter(|s| s.len() <= 3) {
            for second in strings.iter().filter(|s| s.len() <= 3) {
                let args = vec![first.clone(), second.clone()];
                assert_eq!(split_args(&join_args(&args)), args, "{first:?} {second:?}");
            }
        }
    }

    /// Text typed in the format the field has always taken reads as before, and
    /// the Windows rules cover what it could not say.
    #[test]
    fn typed_arguments_keep_their_old_meaning() {
        assert_eq!(
            split_args(r#"-NoLogo -File "C:\My Scripts\start.ps1""#),
            owned(&["-NoLogo", "-File", r"C:\My Scripts\start.ps1"])
        );
        assert_eq!(split_args(r#"a "b c" d"#), owned(&["a", "b c", "d"]));
        assert_eq!(split_args(r#"a"b c"d"#), owned(&["ab cd"]), "a quote can start inside a word");
        assert_eq!(split_args(r"C:\dir\file.txt"), owned(&[r"C:\dir\file.txt"]), "backslashes are literal");
        assert_eq!(split_args("a\tb\u{a0}c"), owned(&["a", "b", "c"]), "any white space separates");
        assert_eq!(split_args(r#"-m "open"#), owned(&["-m", "open"]), "an open quote runs to the end");
        // What the old reader could not express.
        assert_eq!(split_args(r#"a "" b"#), owned(&["a", "", "b"]), "an empty argument");
        assert_eq!(split_args(r#"say \"hi\""#), owned(&["say", "\"hi\""]), "an escaped quote");
        assert_eq!(split_args(r#""a ""b"" c""#), owned(&["a \"b\" c"]), "a doubled quote inside a quoted run");
        assert_eq!(split_args(r#""C:\dir\\" x"#), owned(&[r"C:\dir\", "x"]), "2n backslashes before a closing quote");
    }
}
