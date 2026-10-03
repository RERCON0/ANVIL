//! Modal dialogs. Only the Claude Code status-line replacement question so far.

use crate::strings;

pub struct DialogState {
    command: Option<String>,
}

impl DialogState {
    pub fn claude_replace(command: String) -> DialogState {
        DialogState { command: Some(command) }
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }
}

pub enum DialogOutcome {
    None,
    /// true: replace the foreign status line, false: keep it.
    ClaudeReplace(bool),
}

pub fn show(ctx: &egui::Context, dialog: &DialogState) -> DialogOutcome {
    let mut outcome = DialogOutcome::None;
    let command = dialog.command().unwrap_or_default();
    egui::Window::new(strings::APP_TITLE)
        .id(egui::Id::new("anvil-claude-dialog"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .fixed_size(egui::Vec2::new(460.0, 140.0))
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(strings::claude_replace_question(command))
                    .font(crate::theme::font(12.5))
                    .color(crate::theme::TEXT),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add(crate::theme::accent_button(strings::CLAUDE_REPLACE)).clicked() {
                    outcome = DialogOutcome::ClaudeReplace(true);
                }
                if ui.add(crate::theme::ghost_button(strings::CLAUDE_KEEP)).clicked() {
                    outcome = DialogOutcome::ClaudeReplace(false);
                }
            });
        });
    outcome
}
