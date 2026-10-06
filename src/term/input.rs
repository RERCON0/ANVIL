//! Key press -> bytes for the PTY (xterm encoding, no kitty protocol).

use crate::hotkeys::{KeyName, Mods};

/// One key press as the terminal sees it. `key` is the physical key (None for
/// keys we do not know), `text` is what the keyboard layout produced, and
/// `altgr` says the Ctrl+Alt in `mods` actually came from AltGr.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyPress {
    pub key: Option<KeyName>,
    pub mods: Mods,
    pub text: Option<String>,
    pub altgr: bool,
}

impl KeyPress {
    /// Ctrl, Alt, Shift or Win pressed on its own: no key the terminal knows
    /// and no text.
    pub fn is_modifier_only(&self) -> bool {
        self.key.is_none() && self.text.as_deref().is_none_or(str::is_empty)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InputModes {
    /// DECCKM (`TermMode::APP_CURSOR`).
    pub app_cursor: bool,
}

/// Bytes to send for `press`, or None when the press produces nothing.
pub fn encode(press: &KeyPress, modes: InputModes) -> Option<Vec<u8>> {
    let m = press.mods;
    if press.altgr {
        if let Some(text) = printable(press) {
            return Some(text.as_bytes().to_vec());
        }
    }
    if let Some(key) = press.key {
        if let Some(bytes) = encode_special(key, m, modes) {
            return Some(bytes);
        }
        if m.ctrl {
            if let Some(code) = ctrl_code(key) {
                return Some(with_alt(m.alt, &[code]));
            }
        }
    }
    if !m.ctrl {
        if let Some(text) = printable(press) {
            if m.alt {
                if let Some(latin) = alt_latin_letter(press.key, text) {
                    return Some(with_alt(true, &[latin]));
                }
            }
            return Some(with_alt(m.alt, text.as_bytes()));
        }
    }
    None
}

/// Alt+letter on a non-Latin layout (Russian Alt+М): applications bind Meta
/// shortcuts to Latin letters (Claude Code's Alt+V image paste, readline's
/// Alt+B/F), so send the physical key's letter in the case the layout typed,
/// the way Ctrl+letter already uses the physical key. Latin layouts keep their
/// own letter (QWERTZ Alt+Y stays ESC z).
fn alt_latin_letter(key: Option<KeyName>, text: &str) -> Option<u8> {
    let KeyName::Letter(letter) = key? else { return None };
    let mut chars = text.chars();
    let typed = chars.next().filter(|c| chars.next().is_none() && c.is_alphabetic() && !c.is_ascii())?;
    let letter = letter as u8;
    Some(if typed.is_uppercase() { letter.to_ascii_uppercase() } else { letter.to_ascii_lowercase() })
}

fn printable(press: &KeyPress) -> Option<&str> {
    press.text.as_deref().filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
}

fn with_alt(alt: bool, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

fn modifier_param(m: Mods) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn encode_special(key: KeyName, m: Mods, modes: InputModes) -> Option<Vec<u8>> {
    let p = modifier_param(m);
    let cursor = |letter: char| -> Vec<u8> {
        if p > 1 {
            format!("\x1b[1;{p}{letter}").into_bytes()
        } else if modes.app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if p > 1 {
            format!("\x1b[{n};{p}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    Some(match key {
        KeyName::Up => cursor('A'),
        KeyName::Down => cursor('B'),
        KeyName::Right => cursor('C'),
        KeyName::Left => cursor('D'),
        KeyName::Home => cursor('H'),
        KeyName::End => cursor('F'),
        KeyName::Insert => tilde(2),
        KeyName::Delete => tilde(3),
        KeyName::PageUp => tilde(5),
        KeyName::PageDown => tilde(6),
        KeyName::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            if p > 1 {
                format!("\x1b[1;{p}{letter}").into_bytes()
            } else {
                format!("\x1bO{letter}").into_bytes()
            }
        }
        KeyName::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        KeyName::Enter => with_alt(m.alt, b"\r"),
        KeyName::Backspace if m.ctrl => with_alt(m.alt, b"\x08"),
        KeyName::Backspace => with_alt(m.alt, b"\x7f"),
        KeyName::Tab if m.shift => b"\x1b[Z".to_vec(),
        KeyName::Tab => with_alt(m.alt, b"\t"),
        KeyName::Escape => with_alt(m.alt, b"\x1b"),
        _ => return None,
    })
}

fn ctrl_code(key: KeyName) -> Option<u8> {
    Some(match key {
        KeyName::Letter(c) => c as u8 - b'A' + 1,
        KeyName::Space | KeyName::Digit(2) => 0x00,
        KeyName::BracketLeft | KeyName::Digit(3) => 0x1b,
        KeyName::Backslash | KeyName::Digit(4) => 0x1c,
        KeyName::BracketRight | KeyName::Digit(5) => 0x1d,
        KeyName::Digit(6) => 0x1e,
        KeyName::Slash | KeyName::Minus | KeyName::Digit(7) => 0x1f,
        KeyName::Digit(8) => 0x7f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false, meta: false };
    const SHIFT: Mods = Mods { ctrl: false, alt: false, shift: true, meta: false };
    const ALT: Mods = Mods { ctrl: false, alt: true, shift: false, meta: false };
    const CTRL: Mods = Mods { ctrl: true, alt: false, shift: false, meta: false };
    const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false, meta: false };
    const CTRL_SHIFT: Mods = Mods { ctrl: true, alt: false, shift: true, meta: false };
    const ALT_SHIFT: Mods = Mods { ctrl: false, alt: true, shift: true, meta: false };

    fn key(k: KeyName, mods: Mods) -> KeyPress {
        KeyPress { key: Some(k), mods, text: None, altgr: false }
    }

    fn text(k: Option<KeyName>, mods: Mods, t: &str) -> KeyPress {
        KeyPress { key: k, mods, text: Some(t.to_owned()), altgr: false }
    }

    fn enc(p: KeyPress) -> String {
        String::from_utf8(encode(&p, InputModes::default()).expect("bytes")).unwrap()
    }

    fn enc_app(p: KeyPress) -> String {
        String::from_utf8(encode(&p, InputModes { app_cursor: true }).expect("bytes")).unwrap()
    }

    #[test]
    fn arrows_home_end() {
        assert_eq!(enc(key(KeyName::Up, NONE)), "\x1b[A");
        assert_eq!(enc(key(KeyName::Down, NONE)), "\x1b[B");
        assert_eq!(enc(key(KeyName::Right, NONE)), "\x1b[C");
        assert_eq!(enc(key(KeyName::Left, NONE)), "\x1b[D");
        assert_eq!(enc(key(KeyName::Home, NONE)), "\x1b[H");
        assert_eq!(enc(key(KeyName::End, NONE)), "\x1b[F");
        assert_eq!(enc_app(key(KeyName::Up, NONE)), "\x1bOA");
        assert_eq!(enc_app(key(KeyName::Home, NONE)), "\x1bOH");
        assert_eq!(enc(key(KeyName::Left, CTRL)), "\x1b[1;5D");
        assert_eq!(enc(key(KeyName::Right, SHIFT)), "\x1b[1;2C");
        assert_eq!(enc(key(KeyName::Up, ALT)), "\x1b[1;3A");
        assert_eq!(enc_app(key(KeyName::Up, CTRL_SHIFT)), "\x1b[1;6A", "modifiers win over DECCKM");
    }

    #[test]
    fn tilde_keys() {
        assert_eq!(enc(key(KeyName::Insert, NONE)), "\x1b[2~");
        assert_eq!(enc(key(KeyName::Delete, NONE)), "\x1b[3~");
        assert_eq!(enc(key(KeyName::PageUp, NONE)), "\x1b[5~");
        assert_eq!(enc(key(KeyName::PageDown, NONE)), "\x1b[6~");
        assert_eq!(enc(key(KeyName::Delete, SHIFT)), "\x1b[3;2~");
        assert_eq!(enc(key(KeyName::Insert, CTRL)), "\x1b[2;5~");
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(key(KeyName::F(1), NONE)), "\x1bOP");
        assert_eq!(enc(key(KeyName::F(4), NONE)), "\x1bOS");
        assert_eq!(enc(key(KeyName::F(1), SHIFT)), "\x1b[1;2P");
        assert_eq!(enc(key(KeyName::F(5), NONE)), "\x1b[15~");
        assert_eq!(enc(key(KeyName::F(6), NONE)), "\x1b[17~");
        assert_eq!(enc(key(KeyName::F(10), NONE)), "\x1b[21~");
        assert_eq!(enc(key(KeyName::F(11), NONE)), "\x1b[23~");
        assert_eq!(enc(key(KeyName::F(12), CTRL)), "\x1b[24;5~");
        assert_eq!(encode(&key(KeyName::F(13), NONE), InputModes::default()), None);
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc(key(KeyName::Enter, NONE)), "\r");
        assert_eq!(enc(key(KeyName::Enter, SHIFT)), "\r");
        assert_eq!(enc(key(KeyName::Backspace, NONE)), "\x7f");
        assert_eq!(enc(key(KeyName::Backspace, ALT)), "\x1b\x7f");
        assert_eq!(enc(key(KeyName::Backspace, CTRL)), "\x08");
        assert_eq!(enc(key(KeyName::Tab, NONE)), "\t");
        assert_eq!(enc(key(KeyName::Tab, SHIFT)), "\x1b[Z");
        assert_eq!(enc(key(KeyName::Escape, NONE)), "\x1b");
    }

    #[test]
    fn control_codes() {
        assert_eq!(enc(key(KeyName::Letter('A'), CTRL)), "\x01");
        assert_eq!(enc(key(KeyName::Letter('C'), CTRL)), "\x03");
        assert_eq!(enc(key(KeyName::Letter('V'), CTRL)), "\x16");
        assert_eq!(enc(key(KeyName::Letter('X'), CTRL)), "\x18");
        assert_eq!(enc(key(KeyName::Letter('Z'), CTRL)), "\x1a");
        assert_eq!(enc(key(KeyName::Space, CTRL)), "\x00");
        assert_eq!(enc(key(KeyName::Digit(2), CTRL)), "\x00");
        assert_eq!(enc(key(KeyName::BracketLeft, CTRL)), "\x1b");
        assert_eq!(enc(key(KeyName::Backslash, CTRL)), "\x1c");
        assert_eq!(enc(key(KeyName::BracketRight, CTRL)), "\x1d");
        assert_eq!(enc(key(KeyName::Digit(6), CTRL)), "\x1e");
        assert_eq!(enc(key(KeyName::Slash, CTRL)), "\x1f");
        assert_eq!(enc(key(KeyName::Letter('B'), CTRL_ALT)), "\x1b\x02");
    }

    #[test]
    fn ctrl_letter_on_russian_layout_uses_physical_key() {
        // Ctrl+С on the Russian layout: the physical key is C, the text is Cyrillic.
        let press = KeyPress { key: Some(KeyName::Letter('C')), mods: CTRL, text: Some("с".into()), altgr: false };
        assert_eq!(enc(press), "\x03");
    }

    /// Alt+letter shortcuts (Claude Code's Alt+V image paste, readline's
    /// Alt+B/F) are Latin: on the Russian layout Alt+М must still send ESC v,
    /// exactly like Ctrl+М already sends ^V.
    #[test]
    fn alt_letter_on_russian_layout_uses_physical_key() {
        assert_eq!(enc(text(Some(KeyName::Letter('V')), ALT, "м")), "\x1bv");
        assert_eq!(enc(text(Some(KeyName::Letter('V')), ALT_SHIFT, "М")), "\x1bV", "Shift keeps the case");
        assert_eq!(enc(text(Some(KeyName::Letter('F')), ALT, "а")), "\x1bf");
        // Only letters: punctuation keys keep what the layout typed.
        assert_eq!(enc(text(Some(KeyName::Comma), ALT, "б")), "\x1bб");
        // Without Alt the layout's text is typed as is.
        assert_eq!(enc(text(Some(KeyName::Letter('V')), NONE, "м")), "м");
        // A Latin layout that moves letters (QWERTZ) keeps its own letter.
        assert_eq!(enc(text(Some(KeyName::Letter('Y')), ALT, "z")), "\x1bz");
    }

    #[test]
    fn text_and_alt_prefix() {
        assert_eq!(enc(text(Some(KeyName::Letter('A')), NONE, "a")), "a");
        assert_eq!(enc(text(Some(KeyName::Letter('A')), SHIFT, "A")), "A");
        assert_eq!(enc(text(Some(KeyName::Letter('F')), NONE, "а")), "а", "Cyrillic text passes through");
        assert_eq!(enc(text(Some(KeyName::Letter('B')), ALT, "b")), "\x1bb");
        assert_eq!(enc(text(Some(KeyName::Space), NONE, " ")), " ");
        assert_eq!(enc(text(None, NONE, "é")), "é", "dead-key result without a known key");
    }

    #[test]
    fn altgr_text_wins_over_ctrl_alt() {
        let press = KeyPress { key: Some(KeyName::Letter('Q')), mods: CTRL_ALT, text: Some("@".into()), altgr: true };
        assert_eq!(enc(press), "@");
        let not_altgr =
            KeyPress { key: Some(KeyName::Letter('Q')), mods: CTRL_ALT, text: Some("q".into()), altgr: false };
        assert_eq!(enc(not_altgr), "\x1b\x11");
    }

    /// "Press any key" after a failed exit must not count Ctrl or Alt alone:
    /// pressing Ctrl to start Ctrl+Shift+C closed the pane before its error
    /// text could be copied.
    #[test]
    fn modifier_keys_alone_are_not_key_presses() {
        assert!(KeyPress { key: None, mods: CTRL, text: None, altgr: false }.is_modifier_only());
        assert!(!key(KeyName::Letter('C'), CTRL_SHIFT).is_modifier_only());
        assert!(!text(None, NONE, "é").is_modifier_only(), "a dead-key result is typing");
    }

    #[test]
    fn control_text_is_ignored() {
        let press = KeyPress { key: None, mods: NONE, text: Some("\x01".into()), altgr: false };
        assert_eq!(encode(&press, InputModes::default()), None);
    }
}
