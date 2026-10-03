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
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyFocus {
    /// `egui::Context::wants_keyboard_input()` from the last frame.
    pub egui_wants_keyboard: bool,
    /// A terminal pane owns the keyboard (ANVIL state, not an egui widget).
    pub terminal_focused: bool,
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
    if focus.terminal_focused {
        return if binding.is_some() { KeyRoute::TerminalAction } else { KeyRoute::Terminal };
    }
    KeyRoute::Egui
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERM: KeyFocus = KeyFocus { egui_wants_keyboard: false, terminal_focused: true };
    const FIELD: KeyFocus = KeyFocus { egui_wants_keyboard: true, terminal_focused: true };
    const NOWHERE: KeyFocus = KeyFocus { egui_wants_keyboard: false, terminal_focused: false };

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
}
