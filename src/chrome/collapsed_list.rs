//! Centered list of the panes hidden by Ctrl+Alt+C (Ctrl+Alt+L).

use crate::app::CollapsedListState;
use crate::layout::split_tree::PaneId;
use crate::strings;

/// One hidden pane: the pane's own title plus the title of
/// the tab it belongs to, so a pane hidden in a background
/// tab is told apart from the active one.
pub struct CollapsedRow {
    pub tab: usize,
    pub pane: PaneId,
    pub tab_title: String,
    pub pane_title: String,
    /// The last few screen lines of the pane. Panes of the
    /// same profile share a title and are otherwise
    /// indistinguishable in the list.
    pub preview: String,
}

pub enum CollapsedListOutcome {
    None,
    Closed,
    Restore { tab: usize, pane: PaneId },
}

impl std::fmt::Debug for CollapsedListOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollapsedListOutcome::None => f.write_str("None"),
            CollapsedListOutcome::Closed => f.write_str("Closed"),
            CollapsedListOutcome::Restore { tab, pane } => {
                write!(f, "Restore {{ tab: {tab}, pane: {pane} }}")
            }
        }
    }
}

pub fn show(ctx: &egui::Context, list: &mut CollapsedListState, rows: &[CollapsedRow]) -> CollapsedListOutcome {
    let mut outcome = CollapsedListOutcome::None;
    if !rows.is_empty() {
        list.selected = list.selected.min(rows.len() - 1);
    }

    let window = egui::Window::new("anvil-collapsed-list")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .fixed_size(egui::Vec2::new(460.0, 320.0))
        .show(ctx, |ui| {
            let down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
            let up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
            if !rows.is_empty() {
                if down {
                    list.selected = (list.selected + 1) % rows.len();
                }
                if up {
                    list.selected = (list.selected + rows.len() - 1) % rows.len();
                }
            }
            if rows.is_empty() {
                ui.label(strings::COLLAPSED_EMPTY());
            } else {
                ui.label(
                    egui::RichText::new(strings::COLLAPSED_HINT())
                        .color(crate::theme::colors().faint)
                        .font(crate::theme::font(11.5)),
                );
            }
            ui.separator();
            egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                if rows.is_empty() {
                    return;
                }
                for (index, row) in rows.iter().enumerate() {
                    let selected = index == list.selected;
                    let title = if row.tab_title.is_empty() {
                        row.pane_title.clone()
                    } else {
                        format!("{} — {}", row.pane_title, row.tab_title)
                    };
                    let response =
                        ui.selectable_label(selected, egui::RichText::new(title).font(crate::theme::font(13.0)));
                    if selected {
                        response.scroll_to_me(None);
                    }
                    if !row.preview.trim().is_empty() {
                        // The tooltip is a clone: a click on the row
                        // restores the pane, and the response must
                        // still be there to test for it.
                        response.clone().on_hover_text(row.preview.clone());
                    }
                    if response.clicked() {
                        outcome = CollapsedListOutcome::Restore { tab: row.tab, pane: row.pane };
                    }
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(row) = rows.get(list.selected) {
                    outcome = CollapsedListOutcome::Restore { tab: row.tab, pane: row.pane };
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                outcome = CollapsedListOutcome::Closed;
            }
        });
    if let Some(window) = window {
        if list.opened_pass != ctx.cumulative_pass_nr() && window.response.clicked_elsewhere() {
            outcome = CollapsedListOutcome::Closed;
        }
    } else {
        outcome = CollapsedListOutcome::Closed;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::CollapsedListState;

    fn rows() -> Vec<CollapsedRow> {
        vec![
            CollapsedRow {
                tab: 0,
                pane: 4,
                tab_title: "work".to_owned(),
                pane_title: "git bash".to_owned(),
                preview: String::new(),
            },
            CollapsedRow {
                tab: 1,
                pane: 7,
                tab_title: String::new(),
                pane_title: "PowerShell".to_owned(),
                preview: "C:\\src> ".to_owned(),
            },
        ]
    }

    fn run(
        ctx: &egui::Context,
        list: &mut CollapsedListState,
        rows: &[CollapsedRow],
        events: Vec<egui::Event>,
    ) -> CollapsedListOutcome {
        let mut outcome = CollapsedListOutcome::None;
        let _ = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            outcome = show(ui.ctx(), list, rows);
        });
        outcome
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    #[test]
    fn empty_list_has_nothing_to_select() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let mut list = CollapsedListState { selected: 0, opened_pass: 0 };
        // Nothing collapsed: the list stays open, shows the
        // empty note and Enter has nothing to restore.
        assert!(matches!(run(&ctx, &mut list, &[], Vec::new()), CollapsedListOutcome::None));
        assert!(matches!(run(&ctx, &mut list, &[], vec![key(egui::Key::Enter)]), CollapsedListOutcome::None));
    }

    #[test]
    fn arrows_move_the_selection_and_enter_restores() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let rows = rows();
        let mut list = CollapsedListState { selected: 0, opened_pass: 0 };
        assert!(matches!(run(&ctx, &mut list, &rows, vec![key(egui::Key::ArrowDown)]), CollapsedListOutcome::None));
        assert_eq!(list.selected, 1);
        assert!(matches!(run(&ctx, &mut list, &rows, vec![key(egui::Key::ArrowUp)]), CollapsedListOutcome::None));
        assert_eq!(list.selected, 0);
        // Enter restores the selected row, tab and pane included.
        match run(&ctx, &mut list, &rows, vec![key(egui::Key::Enter)]) {
            CollapsedListOutcome::Restore { tab, pane } => {
                assert_eq!((tab, pane), (0, 4));
            }
            other => panic!("Enter must restore, got {other:?}"),
        }
    }

    #[test]
    fn escape_and_an_outside_click_close_the_list() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, "Consolas", &crate::fonts::registry_font_entries(), false);
        let rows = rows();
        let mut list = CollapsedListState { selected: 0, opened_pass: 0 };
        assert!(matches!(run(&ctx, &mut list, &rows, vec![key(egui::Key::Escape)]), CollapsedListOutcome::Closed));

        // A click far from the rows closes the list, but the pass
        // it opened in does not: the opening click is not "elsewhere".
        let mut list = CollapsedListState { selected: 0, opened_pass: 1 };
        let click = |pressed| egui::Event::PointerButton {
            pos: egui::Pos2::new(900.0, 900.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        assert!(
            matches!(run(&ctx, &mut list, &rows, vec![click(true), click(false)]), CollapsedListOutcome::None),
            "the opening pass must not close the list"
        );
        list.opened_pass = 0;
        assert!(matches!(run(&ctx, &mut list, &rows, vec![click(true), click(false)]), CollapsedListOutcome::Closed));
    }
}
