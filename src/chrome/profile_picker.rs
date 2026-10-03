//! Centered profile list with a fuzzy filter (Ctrl+Shift+E, button under tabs).

use crate::app::PickerState;
use crate::profiles::fuzzy_score;
use crate::strings;

pub enum PickerOutcome {
    None,
    Closed,
    Selected(String),
}

pub fn show(ctx: &egui::Context, picker: &mut PickerState, profiles: &[(String, String)]) -> PickerOutcome {
    let mut outcome = PickerOutcome::None;
    let mut matches: Vec<(i32, usize)> = profiles
        .iter()
        .enumerate()
        .filter_map(|(index, (_, name))| fuzzy_score(&picker.filter, name).map(|score| (score, index)))
        .collect();
    matches.sort_by(|a, b| b.0.cmp(&a.0));
    if !matches.is_empty() {
        picker.selected = picker.selected.min(matches.len() - 1);
    }

    let window = egui::Window::new("anvil-profile-picker")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .fixed_size(egui::Vec2::new(420.0, 320.0))
        .show(ctx, |ui| {
            let down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
            let up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
            if !matches.is_empty() {
                if down {
                    picker.selected = (picker.selected + 1) % matches.len();
                }
                if up {
                    picker.selected = (picker.selected + matches.len() - 1) % matches.len();
                }
            }
            let field = ui.add(
                egui::TextEdit::singleline(&mut picker.filter)
                    .hint_text(strings::PICKER_FILTER)
                    .desired_width(f32::INFINITY),
            );
            if picker.focus {
                field.request_focus();
                picker.focus = false;
            }
            if field.changed() {
                picker.selected = 0;
            }
            ui.separator();
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                if matches.is_empty() {
                    ui.label(strings::PICKER_EMPTY);
                }
                for (rank, (_, index)) in matches.iter().enumerate() {
                    let (id, name) = &profiles[*index];
                    let selected = rank == picker.selected;
                    let response = ui.selectable_label(selected, name);
                    if response.clicked() {
                        outcome = PickerOutcome::Selected(id.clone());
                    }
                    if selected {
                        response.scroll_to_me(None);
                    }
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some((_, index)) = matches.get(picker.selected) {
                    outcome = PickerOutcome::Selected(profiles[*index].0.clone());
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                outcome = PickerOutcome::Closed;
            }
        });
    if let Some(window) = window {
        if window.response.clicked_elsewhere() {
            outcome = PickerOutcome::Closed;
        }
    } else {
        outcome = PickerOutcome::Closed;
    }
    outcome
}
