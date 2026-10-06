//! User consent for global Claude settings changes and executable multiline paste.

use crate::strings;
use std::path::PathBuf;

pub enum DialogState {
    ClaudeInstall { path: PathBuf, expected: Option<String>, ours: String, current: String, keep_previous: bool },
    Paste { pane_id: u64, text: String },
}

pub enum DialogOutcome {
    None,
    Cancel,
    Accept,
}

pub fn show(ctx: &egui::Context, dialog: &DialogState) -> DialogOutcome {
    let mut outcome =
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { DialogOutcome::Cancel } else { DialogOutcome::None };
    egui::Window::new(crate::strings::APP_TITLE)
        .id(egui::Id::new("anvil-consent-dialog"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .default_width(520.0)
        .show(ctx, |ui| {
            let accept = match dialog {
                DialogState::ClaudeInstall { path, current, ours, .. } => {
                    ui.label(strings::CLAUDE_INSTALL_QUESTION);
                    ui.label(strings::CLAUDE_INSTALL_GLOBAL);
                    ui.monospace(path.display().to_string());
                    if !current.is_empty() {
                        ui.label(strings::CLAUDE_CURRENT_COMMAND);
                        ui.monospace(current);
                    }
                    ui.label(strings::CLAUDE_NEW_COMMAND);
                    ui.monospace(ours);
                    ui.label(strings::CLAUDE_INSTALL_WARNING);
                    strings::CLAUDE_INSTALL_ACCEPT
                }
                DialogState::Paste { text, .. } => {
                    ui.label(strings::PASTE_WARNING);
                    ui.label(strings::PASTE_PREVIEW_HINT);
                    egui::ScrollArea::vertical().max_height(230.0).show(ui, |ui| {
                        let preview = crate::term::paste::prepare_paste(text, false);
                        let preview = String::from_utf8_lossy(&preview).replace('\r', "\n");
                        // The preview is not editable: consent applies to these exact bytes.
                        ui.monospace(preview);
                    });
                    strings::PASTE_ACCEPT
                }
            };
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let cancel = ui.add(crate::theme::ghost_button(strings::SETTINGS_CANCEL));
                if cancel.clicked() {
                    outcome = DialogOutcome::Cancel;
                }
                if !ctx.memory(|m| m.focused().is_some()) {
                    cancel.request_focus();
                }
                if ui.add(crate::theme::accent_button(accept)).clicked() {
                    outcome = DialogOutcome::Accept;
                }
            });
        });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_never_defaults_to_executable_paste_and_escape_cancels() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let dialog = DialogState::Paste { pane_id: 7, text: "echo first\necho second\n".to_owned() };
        let frame = |events| {
            let mut outcome = DialogOutcome::None;
            let _ = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                outcome = show(ui.ctx(), &dialog);
            });
            outcome
        };
        assert!(matches!(frame(Vec::new()), DialogOutcome::None));
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        assert!(!matches!(frame(vec![key(egui::Key::Enter)]), DialogOutcome::Accept));
        assert!(matches!(frame(vec![key(egui::Key::Escape)]), DialogOutcome::Cancel));
    }
}
