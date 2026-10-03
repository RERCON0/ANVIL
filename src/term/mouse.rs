//! Mouse reports for applications that enabled mouse tracking, and click
//! counting for word/line selection.

use crate::hotkeys::Mods;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    NoButton,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseModes {
    /// `TermMode::MOUSE_REPORT_CLICK` (1000)
    pub click: bool,
    /// `TermMode::MOUSE_DRAG` (1002)
    pub drag: bool,
    /// `TermMode::MOUSE_MOTION` (1003)
    pub motion: bool,
    /// `TermMode::SGR_MOUSE` (1006)
    pub sgr: bool,
    /// `TermMode::UTF8_MOUSE` (1005)
    pub utf8: bool,
}

impl MouseModes {
    pub fn any(&self) -> bool {
        self.click || self.drag || self.motion
    }
}

/// Whether the application asked to hear about this event.
pub fn wants_report(action: MouseAction, button_held: bool, modes: MouseModes) -> bool {
    match action {
        MouseAction::Press | MouseAction::Release => modes.any(),
        MouseAction::Motion if button_held => modes.drag || modes.motion,
        MouseAction::Motion => modes.motion,
    }
}

/// Encodes one report. `col`/`line` are 0-based cell coordinates in the
/// visible screen. Returns None when the coordinates cannot be encoded in
/// the active format.
pub fn encode_report(
    button: MouseButton,
    action: MouseAction,
    col: usize,
    line: usize,
    mods: Mods,
    modes: MouseModes,
) -> Option<Vec<u8>> {
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::NoButton => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if action == MouseAction::Motion {
        code += 32;
    }
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }
    let (x, y) = (col as u32 + 1, line as u32 + 1);
    if modes.sgr {
        let suffix = if action == MouseAction::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes());
    }
    if action == MouseAction::Release {
        // Legacy encodings cannot say which button was released.
        code = 3 + (code & !0b11);
    }
    let mut out = b"\x1b[M".to_vec();
    out.push((32 + code) as u8);
    for v in [x, y] {
        let v = 32 + v;
        if modes.utf8 {
            out.extend_from_slice(char::from_u32(v).filter(|_| v < 2048)?.to_string().as_bytes());
        } else {
            out.push(u8::try_from(v).ok()?);
        }
    }
    Some(out)
}

/// Counts 1/2/3 clicks in the same cell within `interval_ms`, cycling 3 -> 1.
#[derive(Default)]
pub struct ClickCounter {
    last: Option<(u64, (usize, usize))>,
    count: u8,
}

impl ClickCounter {
    pub fn click(&mut self, now_ms: u64, cell: (usize, usize), interval_ms: u64) -> u8 {
        let repeat = matches!(self.last, Some((t, c)) if c == cell && now_ms.saturating_sub(t) <= interval_ms);
        self.count = if repeat && self.count < 3 { self.count + 1 } else { 1 };
        self.last = Some((now_ms, cell));
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SGR: MouseModes = MouseModes { click: true, drag: false, motion: false, sgr: true, utf8: false };
    const LEGACY: MouseModes = MouseModes { click: true, drag: false, motion: false, sgr: false, utf8: false };
    const NO_MODS: Mods = Mods { ctrl: false, alt: false, shift: false, meta: false };

    #[test]
    fn sgr_press_release_motion() {
        let enc = |b, a, m| String::from_utf8(encode_report(b, a, 29, 56, m, SGR).unwrap()).unwrap();
        assert_eq!(enc(MouseButton::Left, MouseAction::Press, NO_MODS), "\x1b[<0;30;57M");
        assert_eq!(enc(MouseButton::Left, MouseAction::Release, NO_MODS), "\x1b[<0;30;57m");
        assert_eq!(enc(MouseButton::NoButton, MouseAction::Motion, NO_MODS), "\x1b[<35;30;57M");
        assert_eq!(enc(MouseButton::WheelUp, MouseAction::Press, NO_MODS), "\x1b[<64;30;57M");
        let ctrl_shift = Mods { ctrl: true, shift: true, ..NO_MODS };
        assert_eq!(enc(MouseButton::Right, MouseAction::Press, ctrl_shift), "\x1b[<22;30;57M");
    }

    #[test]
    fn legacy_encoding_and_limits() {
        assert_eq!(
            encode_report(MouseButton::Left, MouseAction::Press, 0, 0, NO_MODS, LEGACY).unwrap(),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
        assert_eq!(
            encode_report(MouseButton::Left, MouseAction::Release, 0, 0, NO_MODS, LEGACY).unwrap(),
            vec![0x1b, b'[', b'M', 35, 33, 33]
        );
        assert_eq!(encode_report(MouseButton::Left, MouseAction::Press, 300, 0, NO_MODS, LEGACY), None);
        let utf8 = MouseModes { utf8: true, ..LEGACY };
        let out = encode_report(MouseButton::Left, MouseAction::Press, 300, 0, NO_MODS, utf8).unwrap();
        assert_eq!(&out[..4], &[0x1b, b'[', b'M', 32]);
        assert_eq!(String::from_utf8(out[4..].to_vec()).unwrap(), "\u{14d}!");
    }

    #[test]
    fn report_filter() {
        let drag = MouseModes { click: true, drag: true, ..MouseModes::default() };
        assert!(wants_report(MouseAction::Press, false, LEGACY));
        assert!(!wants_report(MouseAction::Motion, true, LEGACY));
        assert!(wants_report(MouseAction::Motion, true, drag));
        assert!(!wants_report(MouseAction::Motion, false, drag));
        let motion = MouseModes { motion: true, ..MouseModes::default() };
        assert!(wants_report(MouseAction::Motion, false, motion));
        assert!(!wants_report(MouseAction::Press, false, MouseModes::default()));
    }

    #[test]
    fn click_counter_cycles() {
        let mut c = ClickCounter::default();
        assert_eq!(c.click(0, (1, 1), 400), 1);
        assert_eq!(c.click(100, (1, 1), 400), 2);
        assert_eq!(c.click(200, (1, 1), 400), 3);
        assert_eq!(c.click(300, (1, 1), 400), 1);
        assert_eq!(c.click(1000, (1, 1), 400), 1, "too slow");
        assert_eq!(c.click(1100, (2, 1), 400), 1, "other cell");
    }
}
