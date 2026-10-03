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
                    .font(crate::theme::field_font(13.0))
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
                    let response = ui.selectable_label(selected, egui::RichText::new(name).font(crate::theme::font(13.0)));
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
        if picker.opened_pass != ctx.cumulative_pass_nr() && window.response.clicked_elsewhere() {
            outcome = PickerOutcome::Closed;
        }
    } else {
        outcome = PickerOutcome::Closed;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PickerState;

    fn profiles() -> Vec<(String, String)> {
        vec![("powershell".to_owned(), "PowerShell".to_owned()), ("gitbash".to_owned(), "Git Bash".to_owned())]
    }

    fn click(pos: egui::Pos2) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1280.0, 800.0))),
            ..Default::default()
        };
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        input.events.push(egui::Event::PointerMoved(pos));
        input.events.push(button(true));
        input.events.push(button(false));
        input
    }

    fn run(ctx: &egui::Context, picker: &mut PickerState, input: egui::RawInput) -> PickerOutcome {
        let profiles = profiles();
        let mut outcome = PickerOutcome::None;
        let _ = ctx.run(input, |ctx| outcome = show(ctx, picker, &profiles));
        outcome
    }

    /// The very click that opens the picker must not count as a click
    /// elsewhere, or the window closes in the frame where it appears.
    #[test]
    fn the_opening_click_keeps_the_picker_open() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let profiles = profiles();
        let mut picker = PickerState { filter: String::new(), selected: 0, focus: true, opened_pass: 0 };
        let mut outcomes = Vec::new();
        let _ = ctx.run(click(egui::Pos2::new(300.0, 300.0)), |ctx| {
            // The panel action re-arms `opened_pass` in every pass of the
            // frame, exactly like the tabbar button that opened the picker.
            if ctx.input(|i| i.pointer.any_click()) {
                picker.opened_pass = ctx.cumulative_pass_nr();
            }
            outcomes.push(show(ctx, &mut picker, &profiles));
        });
        assert!(outcomes.iter().all(|outcome| matches!(outcome, PickerOutcome::None)), "the opening click closed the picker");

        // An idle frame keeps it open, a later click outside closes it.
        assert!(matches!(run(&ctx, &mut picker, egui::RawInput::default()), PickerOutcome::None));
        assert!(matches!(run(&ctx, &mut picker, click(egui::Pos2::new(300.0, 300.0))), PickerOutcome::Closed));
    }
}
