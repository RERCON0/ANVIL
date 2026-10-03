//! winit keyboard events -> the layout-independent types in hotkeys.rs/input.rs.

use winit::event::KeyEvent;
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use crate::hotkeys::{Chord, KeyName, Mods};
use crate::term::input::KeyPress;

pub fn mods(state: ModifiersState) -> Mods {
    Mods { ctrl: state.control_key(), alt: state.alt_key(), shift: state.shift_key(), meta: state.super_key() }
}

pub fn key_name(code: KeyCode) -> Option<KeyName> {
    use KeyCode::*;
    let letter = |c: char| Some(KeyName::Letter(c));
    Some(match code {
        KeyA => return letter('A'),
        KeyB => return letter('B'),
        KeyC => return letter('C'),
        KeyD => return letter('D'),
        KeyE => return letter('E'),
        KeyF => return letter('F'),
        KeyG => return letter('G'),
        KeyH => return letter('H'),
        KeyI => return letter('I'),
        KeyJ => return letter('J'),
        KeyK => return letter('K'),
        KeyL => return letter('L'),
        KeyM => return letter('M'),
        KeyN => return letter('N'),
        KeyO => return letter('O'),
        KeyP => return letter('P'),
        KeyQ => return letter('Q'),
        KeyR => return letter('R'),
        KeyS => return letter('S'),
        KeyT => return letter('T'),
        KeyU => return letter('U'),
        KeyV => return letter('V'),
        KeyW => return letter('W'),
        KeyX => return letter('X'),
        KeyY => return letter('Y'),
        KeyZ => return letter('Z'),
        Digit0 => KeyName::Digit(0),
        Digit1 => KeyName::Digit(1),
        Digit2 => KeyName::Digit(2),
        Digit3 => KeyName::Digit(3),
        Digit4 => KeyName::Digit(4),
        Digit5 => KeyName::Digit(5),
        Digit6 => KeyName::Digit(6),
        Digit7 => KeyName::Digit(7),
        Digit8 => KeyName::Digit(8),
        Digit9 => KeyName::Digit(9),
        F1 => KeyName::F(1),
        F2 => KeyName::F(2),
        F3 => KeyName::F(3),
        F4 => KeyName::F(4),
        F5 => KeyName::F(5),
        F6 => KeyName::F(6),
        F7 => KeyName::F(7),
        F8 => KeyName::F(8),
        F9 => KeyName::F(9),
        F10 => KeyName::F(10),
        F11 => KeyName::F(11),
        F12 => KeyName::F(12),
        ArrowLeft => KeyName::Left,
        ArrowRight => KeyName::Right,
        ArrowUp => KeyName::Up,
        ArrowDown => KeyName::Down,
        Home => KeyName::Home,
        End => KeyName::End,
        PageUp => KeyName::PageUp,
        PageDown => KeyName::PageDown,
        Insert => KeyName::Insert,
        Delete => KeyName::Delete,
        Backspace => KeyName::Backspace,
        Enter | NumpadEnter => KeyName::Enter,
        Tab => KeyName::Tab,
        Space => KeyName::Space,
        Escape => KeyName::Escape,
        Equal => KeyName::Equal,
        Minus => KeyName::Minus,
        Comma => KeyName::Comma,
        Period => KeyName::Period,
        BracketLeft => KeyName::BracketLeft,
        BracketRight => KeyName::BracketRight,
        Slash => KeyName::Slash,
        Backslash => KeyName::Backslash,
        Backquote => KeyName::Backquote,
        Semicolon => KeyName::Semicolon,
        Quote => KeyName::Quote,
        _ => return None,
    })
}

/// AltGr arrives with Alt (and on Windows also Ctrl) held, producing a
/// character the key does not have without modifiers (German AltGr+Q = '@').
/// Alt+Shift+a ("A" vs "a") is not AltGr.
pub fn is_altgr(m: Mods, text: Option<&str>, unmodified: Option<&str>) -> bool {
    let printable = |t: &str| !t.is_empty() && !t.chars().any(char::is_control);
    match (text, unmodified) {
        (Some(t), Some(u)) if m.alt && printable(t) => t.to_lowercase() != u.to_lowercase(),
        _ => false,
    }
}

pub fn chord(event: &KeyEvent, state: ModifiersState) -> Option<Chord> {
    let PhysicalKey::Code(code) = event.physical_key else { return None };
    Some(Chord { mods: mods(state), key: key_name(code)? })
}

pub fn key_press(event: &KeyEvent, state: ModifiersState) -> KeyPress {
    let m = mods(state);
    let key = match event.physical_key {
        PhysicalKey::Code(code) => key_name(code),
        PhysicalKey::Unidentified(_) => None,
    };
    let text = event.text.as_ref().map(|t| t.to_string());
    let unmodified = match event.key_without_modifiers() {
        Key::Character(s) => Some(s.to_string()),
        _ => None,
    };
    let altgr = is_altgr(m, text.as_deref(), unmodified.as_deref());
    KeyPress { key, mods: m, text, altgr }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT: Mods = Mods { ctrl: false, alt: true, shift: false, meta: false };
    const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false, meta: false };
    const ALT_SHIFT: Mods = Mods { ctrl: false, alt: true, shift: true, meta: false };

    #[test]
    fn physical_codes_map_to_key_names() {
        assert_eq!(key_name(KeyCode::KeyT), Some(KeyName::Letter('T')));
        assert_eq!(key_name(KeyCode::Digit0), Some(KeyName::Digit(0)));
        assert_eq!(key_name(KeyCode::Minus), Some(KeyName::Minus));
        assert_eq!(key_name(KeyCode::NumpadEnter), Some(KeyName::Enter));
        assert_eq!(key_name(KeyCode::F11), Some(KeyName::F(11)));
        assert_eq!(key_name(KeyCode::ShiftLeft), None);
    }

    #[test]
    fn altgr_detection() {
        assert!(is_altgr(CTRL_ALT, Some("@"), Some("q")), "German AltGr+Q");
        assert!(is_altgr(ALT, Some("€"), Some("e")), "winit reporting only Alt");
        assert!(!is_altgr(ALT_SHIFT, Some("A"), Some("a")), "Alt+Shift+a");
        assert!(!is_altgr(ALT, Some("ф"), Some("ф")), "Alt+letter on the Russian layout");
        assert!(!is_altgr(Mods::default(), Some("@"), Some("2")), "Shift+2 without Alt");
        assert!(!is_altgr(CTRL_ALT, None, Some("q")));
    }
}
