//! Who gets a key press: an app hotkey, egui, or the focused terminal.

use crate::hotkeys::Action;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRoute {
    /// Run the bound app action (tabs, splits, window...). egui never sees it.
    AppAction,
    /// Run the bound terminal action (copy, paste, zoom...) on the focused pane.
    TerminalAction,
    /// Encode the press and send it to the focused pane's PTY.
    Terminal,
    /// Hand the event to egui (text fields, buttons).
    Egui,
    /// Hand a copy key to egui, which copies the selected panel text. The text
    /// is deselected afterwards (see [`clear_egui_text_selection`]), as a
    /// terminal selection is, so the next Ctrl+C interrupts the shell again.
    EguiCopy,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyFocus {
    /// `egui::Context::egui_wants_keyboard_input()` from the last frame.
    pub egui_wants_keyboard: bool,
    /// A terminal pane owns the keyboard (ANVIL state, not an egui widget).
    pub terminal_focused: bool,
    /// Text is selected in an egui label (see [`egui_text_selected`]).
    pub egui_text_selected: bool,
}

/// Whether the user has text selected in a panel label (a commit message, a
/// path). A label selection holds no keyboard focus, so
/// `egui_wants_keyboard_input` is false for it.
pub fn egui_text_selected(ctx: &egui::Context) -> bool {
    ctx.plugin_opt::<egui::text_selection::LabelSelectionState>().is_some_and(|state| state.lock().has_selection())
}

/// Drop the panel text selection once egui has copied it.
pub fn clear_egui_text_selection(ctx: &egui::Context) {
    if let Some(state) = ctx.plugin_opt::<egui::text_selection::LabelSelectionState>() {
        state.lock().clear_selection();
    }
}

/// Routing for a key *press* (releases always go to egui).
pub fn route_key_press(binding: Option<&Action>, focus: KeyFocus) -> KeyRoute {
    if let Some(action) = binding {
        if !action.is_terminal() {
            return KeyRoute::AppAction;
        }
    }
    if focus.egui_wants_keyboard {
        return KeyRoute::Egui;
    }
    // egui turns the press into its own copy event for the selected label text.
    // The terminal's copy keys would find no terminal selection and send ^C.
    if focus.egui_text_selected && binding.is_some_and(Action::copies_selection) {
        return KeyRoute::EguiCopy;
    }
    if focus.terminal_focused {
        return if binding.is_some() { KeyRoute::TerminalAction } else { KeyRoute::Terminal };
    }
    KeyRoute::Egui
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERM: KeyFocus = KeyFocus { egui_wants_keyboard: false, terminal_focused: true, egui_text_selected: false };
    const FIELD: KeyFocus = KeyFocus { egui_wants_keyboard: true, terminal_focused: true, egui_text_selected: false };
    const NOWHERE: KeyFocus =
        KeyFocus { egui_wants_keyboard: false, terminal_focused: false, egui_text_selected: false };
    const SELECTED: KeyFocus =
        KeyFocus { egui_wants_keyboard: false, terminal_focused: true, egui_text_selected: true };

    #[test]
    fn app_hotkeys_fire_from_anywhere() {
        assert_eq!(route_key_press(Some(&Action::NewTab), TERM), KeyRoute::AppAction);
        assert_eq!(route_key_press(Some(&Action::NewTab), FIELD), KeyRoute::AppAction);
        assert_eq!(route_key_press(Some(&Action::NewTab), NOWHERE), KeyRoute::AppAction);
    }

    #[test]
    fn text_fields_keep_their_keys() {
        assert_eq!(route_key_press(Some(&Action::Copy), FIELD), KeyRoute::Egui);
        assert_eq!(route_key_press(None, FIELD), KeyRoute::Egui);
    }

    #[test]
    fn terminal_gets_its_actions_and_raw_keys() {
        assert_eq!(route_key_press(Some(&Action::Copy), TERM), KeyRoute::TerminalAction);
        assert_eq!(route_key_press(Some(&Action::CtrlC), TERM), KeyRoute::TerminalAction);
        assert_eq!(route_key_press(None, TERM), KeyRoute::Terminal);
    }

    #[test]
    fn nothing_focused_goes_to_egui() {
        assert_eq!(route_key_press(Some(&Action::Paste), NOWHERE), KeyRoute::Egui);
        assert_eq!(route_key_press(None, NOWHERE), KeyRoute::Egui);
    }

    /// Selected panel text must be copied, not turned into an interrupt: with
    /// no terminal selection Ctrl+C is `^C`, which kills the agent in the pane.
    #[test]
    fn copy_keys_go_to_egui_while_panel_text_is_selected() {
        assert_eq!(route_key_press(Some(&Action::CtrlC), SELECTED), KeyRoute::EguiCopy);
        assert_eq!(route_key_press(Some(&Action::Copy), SELECTED), KeyRoute::EguiCopy);
    }

    #[test]
    fn selected_panel_text_leaves_every_other_key_alone() {
        assert_eq!(route_key_press(None, SELECTED), KeyRoute::Terminal);
        assert_eq!(route_key_press(Some(&Action::Paste), SELECTED), KeyRoute::TerminalAction);
        assert_eq!(route_key_press(Some(&Action::NewTab), SELECTED), KeyRoute::AppAction);
    }

    const LABEL: &str = "rercon <rercon@protonmail.com> 2026-10-12";

    fn copied(output: &egui::FullOutput) -> Vec<&str> {
        output
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Run one headless egui frame with a selectable label and the given input.
    fn frame(ctx: &egui::Context, time: f64, events: Vec<egui::Event>) -> egui::Rect {
        frame_output(ctx, time, events).0
    }

    fn frame_output(ctx: &egui::Context, time: f64, events: Vec<egui::Event>) -> (egui::Rect, egui::FullOutput) {
        let mut rect = egui::Rect::NOTHING;
        let input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))),
            events,
            ..Default::default()
        };
        let output = crate::test_ui(ctx, input, |ui| {
            rect = ui.label(LABEL).rect;
        });
        (rect, output)
    }

    /// Drag across the label like the mouse does in the commit view.
    fn select_label_text(ctx: &egui::Context) {
        let rect = frame(ctx, 0.0, Vec::new());
        let y = rect.center().y;
        let button = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(rect.left() + 2.0, y),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(ctx, 0.1, vec![egui::Event::PointerMoved(egui::pos2(rect.left() + 2.0, y))]);
        frame(ctx, 0.2, vec![button(true)]);
        frame(ctx, 0.3, vec![egui::Event::PointerMoved(egui::pos2(rect.right() - 2.0, y))]);
        frame(ctx, 0.4, vec![egui::Event::PointerMoved(egui::pos2(rect.right() - 1.0, y))]);
        frame(
            ctx,
            0.5,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(rect.right() - 1.0, y),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    /// Why the router needs its own flag: a label selection is invisible to
    /// `egui_wants_keyboard_input`, which stays false while text is selected.
    #[test]
    fn label_selection_does_not_claim_the_keyboard() {
        let ctx = egui::Context::default();
        select_label_text(&ctx);
        assert!(!ctx.egui_wants_keyboard_input());
        assert!(egui_text_selected(&ctx), "dragging over a label selects its text");
    }

    /// The copy key the router hands over really puts the selected text on the
    /// clipboard, and dropping the selection afterwards keeps that copy.
    #[test]
    fn egui_copies_the_selected_label_text_and_the_selection_can_then_be_dropped() {
        let ctx = egui::Context::default();
        select_label_text(&ctx);
        let (_, output) = frame_output(&ctx, 0.6, vec![egui::Event::Copy]);
        assert_eq!(copied(&output), [LABEL]);
        clear_egui_text_selection(&ctx);
        assert!(!egui_text_selected(&ctx), "the next Ctrl+C must interrupt the shell, not copy again");
        let (_, output) = frame_output(&ctx, 0.7, Vec::new());
        assert!(copied(&output).is_empty());
        assert!(!egui_text_selected(&ctx));
    }

    #[test]
    fn nothing_is_selected_before_the_user_selects() {
        let ctx = egui::Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert!(!egui_text_selected(&ctx));
    }

    /// A click anywhere else drops the selection, so a stale one cannot keep
    /// swallowing Ctrl+C that was meant to interrupt the shell.
    #[test]
    fn clicking_elsewhere_drops_the_selection() {
        let ctx = egui::Context::default();
        select_label_text(&ctx);
        let empty = egui::pos2(300.0, 150.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: empty,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, 0.6, vec![egui::Event::PointerMoved(empty)]);
        frame(&ctx, 0.7, vec![button(true)]);
        frame(&ctx, 0.8, vec![button(false)]);
        assert!(!egui_text_selected(&ctx));
    }
}
