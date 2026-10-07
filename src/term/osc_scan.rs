//! Watches the raw PTY output for working-directory reports. alacritty_terminal
//! ignores these OSCs, so scanning a copy of the bytes is the only way to see
//! them. The scanner never changes the stream; the separate guard prevents
//! oversized OSC strings from reaching VTE's unbounded std buffer.

use std::path::PathBuf;

const MAX_PAYLOAD: usize = 4096;

/// VTE's std OSC buffer is an unbounded Vec. Hold an OSC until it terminates
/// so an oversized one can be discarded entirely, not applied as a truncated
/// title/link/clipboard request. This leaves ordinary screen output untouched.
pub const MAX_OSC_BYTES: usize = 1024 * 1024;

/// Longest title OSC (0 and 2) let through. The display clips a title to a few
/// hundred characters, but alacritty keeps every title it is given and each
/// `CSI 22 t` clones it onto a 4096-entry stack: a megabyte title and a few
/// kilobytes of pushes would cost gigabytes of memory.
pub const MAX_TITLE_OSC_BYTES: usize = 4096;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum GuardState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Osc,
    Discard,
}

/// What the OSC being held is, as far as its size limit is concerned.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum OscKind {
    /// Nothing but the `]` seen yet.
    #[default]
    Start,
    /// A first parameter of `0` or `2`, no `;` yet.
    TitleNumber,
    /// `0;` or `2;`: the window title.
    Title,
    Other,
}

impl OscKind {
    /// Advances over one OSC byte, ignoring the controls VTE ignores inside
    /// a string so the kind is the one VTE will dispatch.
    fn next(self, b: u8) -> OscKind {
        match (self, b) {
            (kind, 0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f) => kind,
            (OscKind::Start, b'0' | b'2') => OscKind::TitleNumber,
            (OscKind::TitleNumber, b';') => OscKind::Title,
            (OscKind::Start | OscKind::TitleNumber, _) => OscKind::Other,
            (kind, _) => kind,
        }
    }

    fn limit(self) -> usize {
        if self == OscKind::Title {
            MAX_TITLE_OSC_BYTES
        } else {
            MAX_OSC_BYTES
        }
    }
}

#[derive(Default)]
pub struct OscGuard {
    state: GuardState,
    kind: OscKind,
    osc: Vec<u8>,
    ready: Vec<u8>,
    offset: usize,
}

impl OscGuard {
    /// Copies pending filtered bytes without reading more PTY output.
    pub fn drain(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.ready.len() - self.offset);
        buf[..n].copy_from_slice(&self.ready[self.offset..self.offset + n]);
        self.offset += n;
        if self.offset == self.ready.len() {
            self.ready.clear();
            if self.ready.capacity() > 8192 {
                self.ready = Vec::new();
            }
            self.offset = 0;
        }
        n
    }

    /// False when the original bytes can be returned directly. The common
    /// CSI-only TUI redraw never allocates or copies through `ready`.
    pub fn filter(&mut self, bytes: &[u8]) -> bool {
        if !matches!(self.state, GuardState::Osc | GuardState::Discard) {
            let mut state = self.state;
            let starts_osc = bytes.iter().any(|&b| {
                let starts = state == GuardState::Escape && b == b']';
                state = escape_state(state, b);
                starts
            });
            if !starts_osc {
                self.state = state;
                return false;
            }
        }

        for &b in bytes {
            match self.state {
                GuardState::Osc => {
                    if matches!(b, 0x07 | 0x18 | 0x1a | 0x1b) {
                        self.ready.extend_from_slice(&self.osc);
                        self.ready.push(b);
                        self.osc.clear();
                        if self.osc.capacity() > 8192 {
                            self.osc = Vec::new();
                        }
                        self.state = if b == 0x1b { GuardState::Escape } else { GuardState::Ground };
                    } else if self.osc.len() < self.kind.next(b).limit() {
                        self.osc.push(b);
                        self.kind = self.kind.next(b);
                    } else {
                        self.osc = Vec::new();
                        // Only the initial ESC was forwarded. CAN resets that
                        // escape without dispatching any partial OSC to VTE.
                        self.ready.push(0x18);
                        self.state = GuardState::Discard;
                    }
                }
                GuardState::Discard => match b {
                    0x07 | 0x18 | 0x1a => self.state = GuardState::Ground,
                    0x1b => {
                        self.ready.push(b);
                        self.state = GuardState::Escape;
                    }
                    _ => {}
                },
                GuardState::Escape if b == b']' => {
                    self.osc.push(b);
                    self.kind = OscKind::Start;
                    self.state = GuardState::Osc;
                }
                state => {
                    self.ready.push(b);
                    self.state = escape_state(state, b);
                }
            }
        }
        true
    }
}

// Mirror VTE's escape entry, including ignored controls and intermediates.
// ESC anywhere ends OSC/DCS/APC/PM and begins an escape in VTE; a literal
// ESC ] therefore starts an OSC even when it follows one of those strings.
// Bytes >= 0x80 are ignored inside an escape too, so `ESC 0x80 ]` is an OSC.
fn escape_state(state: GuardState, b: u8) -> GuardState {
    if b == 0x1b {
        return GuardState::Escape;
    }
    match state {
        GuardState::Escape | GuardState::EscapeIntermediate => match b {
            0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f..=0xff => state,
            0x20..=0x2f => GuardState::EscapeIntermediate,
            _ => GuardState::Ground,
        },
        _ => GuardState::Ground,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Osc,
}

pub struct OscScanner {
    state: State,
    payload: Vec<u8>,
    overflow: bool,
}

impl Default for OscScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OscScanner {
    pub fn new() -> Self {
        Self { state: State::Ground, payload: Vec::new(), overflow: false }
    }

    /// Feeds one chunk of output. Sequences may be split across chunks.
    /// Returns the last directory reported inside this chunk.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<PathBuf> {
        let mut found = None;
        for &b in bytes {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Escape;
                    }
                }
                State::Escape => self.after_escape(b),
                State::Osc => match b {
                    0x07 => {
                        if let Some(dir) = self.finish() {
                            found = Some(dir);
                        }
                    }
                    0x1b | 0x18 | 0x1a => {
                        // VTE dispatches OSC on ESC/CAN/SUB, not only BEL/ST.
                        if let Some(dir) = self.finish() {
                            found = Some(dir);
                        }
                        if b == 0x1b {
                            self.state = State::Escape;
                        }
                    }
                    0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f => {}
                    _ => {
                        if self.payload.len() < MAX_PAYLOAD {
                            self.payload.push(b);
                        } else {
                            self.overflow = true;
                        }
                    }
                },
            }
        }
        found
    }

    fn after_escape(&mut self, b: u8) {
        match b {
            b']' => {
                self.state = State::Osc;
                self.payload.clear();
                self.overflow = false;
            }
            0x1b => self.state = State::Escape,
            0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f..=0xff => {}
            _ => self.state = State::Ground,
        }
    }

    fn finish(&mut self) -> Option<PathBuf> {
        self.state = State::Ground;
        if self.overflow {
            return None;
        }
        parse_cwd(&self.payload)
    }
}

/// Recognizes `1337;CurrentDir=<path>`, `7;file://host/<path>` and `9;9;"<path>"`.
pub fn parse_cwd(payload: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(payload).ok()?;
    let path = if let Some(p) = text.strip_prefix("1337;CurrentDir=") {
        p.to_owned()
    } else if let Some(url) = text.strip_prefix("7;") {
        file_url_path(url)?
    } else if let Some(p) = text.strip_prefix("9;9;") {
        p.trim_matches('"').to_owned()
    } else {
        return None;
    };
    local_drive_path(&path).map(PathBuf::from)
}

/// `path` with `\` separators when it is an absolute path on a local drive
/// (`C:\...`). Anything a program prints can claim a directory: a UNC or
/// device path (`\\host\share`, `\\?\UNC\...`) would make the panel's git
/// and new tabs reach out to that host, and relative or drive-relative paths
/// mean nothing outside the program that sent them.
fn local_drive_path(path: &str) -> Option<String> {
    let path = path.replace('/', "\\");
    let bytes = path.as_bytes();
    let absolute = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    (absolute && !path.chars().any(char::is_control)).then_some(path)
}

fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let decoded = percent_decode(&rest[slash..])?;
    let bytes = decoded.as_bytes();
    // "/C:/Users" (Windows) or "/c/Users" (MSYS) -> "C:\Users"
    let windows = if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        decoded[1..].to_owned()
    } else if bytes.len() >= 2
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && (bytes.len() == 2 || bytes[2] == b'/')
    {
        let drive = (bytes[1] as char).to_ascii_uppercase();
        format!("{drive}:{}", if bytes.len() == 2 { "/" } else { &decoded[2..] })
    } else {
        decoded
    };
    Some(windows.replace('/', "\\"))
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_dispatch_matches_vte_at_every_chunk_boundary() {
        #[derive(Default)]
        struct Observer(Option<PathBuf>);
        impl alacritty_terminal::vte::Perform for Observer {
            fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
                let joined = params.join(&b';');
                if let Some(path) = parse_cwd(&joined) {
                    self.0 = Some(path);
                }
            }
        }
        for sequence in [
            &b"\x1b]1337;CurrentDir=C:\\test\x18"[..],
            b"\x1b]1337;CurrentDir=C:\\test\x1a",
            b"\x1b]1337;CurrentDir=C:\\test\x1b[31m",
            b"\x1b]1337;CurrentDir=C:\\te\x00st\x07",
            b"\x1b\x00]1337;CurrentDir=C:\\test\x07",
            // VTE ignores bytes >= 0x80 inside an escape and stays in it.
            b"\x1b\x80]1337;CurrentDir=C:\\test\x07",
            b"\x1b\xff\x9d]1337;CurrentDir=C:\\test\x07",
        ] {
            let mut observer = Observer::default();
            alacritty_terminal::vte::Parser::new().advance(&mut observer, sequence);
            assert!(observer.0.is_some(), "fixture must really dispatch a cwd");
            for cut in 0..=sequence.len() {
                assert_eq!(scan(&[&sequence[..cut], &sequence[cut..]]), observer.0, "cut at {cut}");
            }
        }
    }

    fn scan(chunks: &[&[u8]]) -> Option<PathBuf> {
        let mut s = OscScanner::new();
        let mut last = None;
        for c in chunks {
            if let Some(d) = s.feed(c) {
                last = Some(d);
            }
        }
        last
    }

    #[test]
    fn iterm_current_dir_with_bel() {
        let out = scan(&[b"prompt\x1b]1337;CurrentDir=C:\\Work\\proj\x07$ "]);
        assert_eq!(out, Some(PathBuf::from("C:\\Work\\proj")));
    }

    #[test]
    fn osc7_windows_and_msys_paths_with_st() {
        assert_eq!(
            scan(&[b"\x1b]7;file://host/C:/Program%20Files/Git\x1b\\"]),
            Some(PathBuf::from("C:\\Program Files\\Git"))
        );
        assert_eq!(scan(&[b"\x1b]7;file://host/c/Windows\x07"]), Some(PathBuf::from("C:\\Windows")));
        assert_eq!(scan(&[b"\x1b]7;file://host/d\x07"]), Some(PathBuf::from("D:\\")));
    }

    #[test]
    fn windows_terminal_9_9() {
        assert_eq!(scan(&[b"\x1b]9;9;\"C:\\Work\"\x07"]), Some(PathBuf::from("C:\\Work")));
    }

    #[test]
    fn survives_splits_at_every_byte() {
        let seq: &[u8] = b"xx\x1b]1337;CurrentDir=C:\\A\x1b\\yy";
        for cut in 0..seq.len() {
            let (a, b) = seq.split_at(cut);
            assert_eq!(scan(&[a, b]), Some(PathBuf::from("C:\\A")), "cut at {cut}");
        }
    }

    #[test]
    fn last_report_in_a_chunk_wins() {
        let out = scan(&[b"\x1b]1337;CurrentDir=C:\\A\x07\x1b]1337;CurrentDir=C:\\B\x07"]);
        assert_eq!(out, Some(PathBuf::from("C:\\B")));
    }

    #[test]
    fn ignores_other_oscs_and_garbage() {
        assert_eq!(scan(&[b"\x1b]0;title\x07\x1b[31mred\x1b]8;;https://x\x07"]), None);
        assert_eq!(scan(&[b"\x1b]1337;CurrentDir=\x07"]), None, "empty path");
        assert_eq!(scan(&[b"\x1b]7;file://host/%ZZ\x07"]), None, "bad escape");
    }

    #[test]
    fn overlong_payload_is_dropped() {
        let mut seq = b"\x1b]1337;CurrentDir=C:\\".to_vec();
        seq.extend(std::iter::repeat_n(b'a', 5000));
        seq.push(0x07);
        assert_eq!(scan(&[&seq]), None);
        assert_eq!(scan(&[&seq, b"\x1b]1337;CurrentDir=C:\\ok\x07"]), Some(PathBuf::from("C:\\ok")));
    }

    /// Any program's output can send these: a UNC report made the panel run git
    /// (and new tabs start) on \\host\share, an outgoing SMB login that leaks
    /// the user's NTLM hash. Only absolute paths on a local drive are taken.
    #[test]
    fn only_local_drive_paths_are_accepted() {
        for bad in [
            &b"\x1b]1337;CurrentDir=\\\\attacker\\share\x07"[..],
            b"\x1b]1337;CurrentDir=//attacker/share\x07",
            b"\x1b]1337;CurrentDir=\\\\?\\UNC\\attacker\\share\x07",
            b"\x1b]1337;CurrentDir=\\\\.\\pipe\\x\x07",
            b"\x1b]9;9;\"\\\\attacker\\share\"\x07",
            b"\x1b]7;file://attacker//share/x\x07",
            b"\x1b]1337;CurrentDir=relative\\dir\x07",
            b"\x1b]1337;CurrentDir=\\rooted\x07",
            b"\x1b]1337;CurrentDir=C:relative\x07",
            b"\x1b]7;file://host/home/user\x07",
        ] {
            assert_eq!(scan(&[bad]), None, "{:?}", String::from_utf8_lossy(bad));
        }
        assert_eq!(scan(&[b"\x1b]1337;CurrentDir=d:/work\x07"]), Some(PathBuf::from("d:\\work")));
    }

    #[test]
    fn escape_inside_osc_restarts() {
        let out = scan(&[b"\x1b]1337;CurrentDir=C:\\A\x1b]1337;CurrentDir=C:\\B\x07"]);
        assert_eq!(out, Some(PathBuf::from("C:\\B")));
    }

    fn guarded(chunks: &[&[u8]]) -> Vec<u8> {
        let mut guard = OscGuard::default();
        let mut output = Vec::new();
        let mut buf = [0u8; 17];
        for chunk in chunks {
            if guard.filter(chunk) {
                loop {
                    let n = guard.drain(&mut buf);
                    if n == 0 {
                        break;
                    }
                    output.extend_from_slice(&buf[..n]);
                }
            } else {
                output.extend_from_slice(chunk);
            }
        }
        output
    }

    #[test]
    fn osc_guard_preserves_split_sequences_and_string_transitions() {
        for seq in [
            &b"plain\x1b[31mred\x1b]0;title\x07done"[..],
            b"\x1b]0;title\x1b\\done",
            b"\x1b]0;title\x18done",
            b"\x1b]0;title\x1adone",
            b"\x1b]0;first\x1b]0;second\x07done",
            b"\x1bPignored\x1b]0;title\x07\x1b\\",
            b"\x1b_ignored\x1b\\\x1b^ignored\x1b\\",
            b"\x1b\x00]0;title\x07",
            b"\x1b ]not-an-osc",
            // VTE 0.15 executes (and ansi ignores) C1 OSC; neither form
            // enters OscString. Do not reinterpret either as ESC ].
            b"\x9d0;not-an-osc\x07",
            b"\xc2\x9d0;not-an-osc\x07",
        ] {
            for cut in 0..=seq.len() {
                assert_eq!(guarded(&[&seq[..cut], &seq[cut..]]), seq, "cut at {cut}");
            }
        }
    }

    /// VTE leaves an escape only on a final byte, CAN/SUB or ESC: bytes >= 0x80
    /// are ignored there, so `ESC 0x80 ]` still opens an OSC string. A guard that
    /// went back to ground on them let such an OSC grow VTE's unbounded buffer.
    #[test]
    fn osc_guard_stays_in_escape_through_high_bytes() {
        #[derive(Default)]
        struct Observer(usize);
        impl alacritty_terminal::vte::Perform for Observer {
            fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {
                self.0 += 1;
            }
        }
        let mut observer = Observer::default();
        alacritty_terminal::vte::Parser::new().advance(&mut observer, b"\x1b\x80\xff]0;title\x07");
        assert_eq!(observer.0, 1, "VTE must really open an OSC after ESC 0x80 0xFF");

        let mut hostile = b"\x1b\x80\xff]0;".to_vec();
        hostile.resize(MAX_OSC_BYTES + 8, b'a');
        for cut in [0, 1, 2, 3, 4, hostile.len() / 2, hostile.len()] {
            assert_eq!(
                guarded(&[&hostile[..cut], &hostile[cut..], b"\x07"]),
                b"\x1b\x80\xff\x18",
                "an overlong OSC behind high bytes is discarded, cut at {cut}"
            );
        }
        let short = b"\x1b\x80\xff]0;title\x07";
        assert_eq!(guarded(&[short]), short, "a short one is forwarded untouched");
    }

    /// Every `CSI 22 t` makes alacritty clone the current title onto a stack of
    /// 4096: with the 1 MiB OSC limit that is 4 GiB for about a megabyte of
    /// output. Titles get a far smaller limit, however VTE comes to read them.
    #[test]
    fn osc_guard_limits_titles_far_below_other_strings() {
        use alacritty_terminal::event::{Event, EventListener};
        use alacritty_terminal::term::{Config, Term};
        use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
        #[derive(Clone)]
        struct Titles(std::sync::mpsc::Sender<String>);
        impl EventListener for Titles {
            fn send_event(&self, event: Event) {
                if let Event::Title(title) = event {
                    let _ = self.0.send(title);
                }
            }
        }
        // VTE ignores the control in `ESC ] 0x01 2 ;` and still sets the title.
        let (tx, rx) = std::sync::mpsc::channel();
        let size = crate::term::pane::GridSize { columns: 20, lines: 3 };
        let mut term = Term::new(Config::default(), &size, Titles(tx));
        Processor::<StdSyncHandler>::new().advance(&mut term, b"\x1b]\x012;shown\x07");
        assert_eq!(rx.try_recv().as_deref(), Ok("shown"));
        let mut huge = b"\x1b]0;".to_vec();
        huge.resize(MAX_OSC_BYTES, b'a');
        huge.push(0x07);
        let mut parser = Processor::<StdSyncHandler>::new();
        parser.advance(&mut term, &huge);
        assert!(rx.try_recv().is_ok_and(|title| title.len() > MAX_OSC_BYTES / 2), "VTE takes a title of any size");
        parser.advance(&mut term, &guarded(&[&huge]));
        assert!(rx.try_recv().is_err(), "the guard keeps it from the terminal");

        for prefix in [&b"\x1b]0;"[..], b"\x1b]2;", b"\x1b]\x012;", b"\x1b]0\x02;", b"\x1b\x80]\x0c0;"] {
            // Bytes before the `]` are forwarded; the OSC itself is held.
            let held = prefix.iter().position(|&b| b == b']').unwrap();
            let mut exact = prefix.to_vec();
            exact.resize(held + MAX_TITLE_OSC_BYTES, b'a');
            let mut valid = exact.clone();
            valid.push(0x07);
            assert_eq!(guarded(&[&valid]), valid, "{prefix:?}");
            let mut too_long = exact.clone();
            too_long.extend_from_slice(b"b\x07x");
            let expected = [&prefix[..held], b"\x18", b"x"].concat();
            assert_eq!(guarded(&[&too_long]), expected, "{prefix:?}");
        }
        // Other strings keep the larger limit, and so do titles-by-name that are not titles.
        for prefix in [&b"\x1b]52;c;"[..], b"\x1b]00;", b"\x1b]20;", b"\x1b]0x;"] {
            let mut long = prefix.to_vec();
            long.resize(prefix.len() + MAX_TITLE_OSC_BYTES * 2, b'a');
            long.push(0x07);
            assert_eq!(guarded(&[&long]), long, "{prefix:?}");
        }
    }

    #[test]
    fn osc_guard_discards_overflow_and_recovers_without_payload_leak() {
        let mut exact = b"\x1b]52;c;".to_vec();
        exact.resize(MAX_OSC_BYTES + 1, b'a'); // ESC plus the allowed OSC bytes.
        for end in [&b"\x07"[..], b"\x1b\\", b"\x18", b"\x1a"] {
            let mut valid = exact.clone();
            valid.extend_from_slice(end);
            assert_eq!(guarded(&[&valid]), valid);
            let mut too_long = exact.clone();
            too_long.push(b'b');
            too_long.extend_from_slice(end);
            let suffix = b"\x1b]0;ok\x07visible";
            let expected = if end == b"\x1b\\" { &b"\x1b\x18\x1b\\"[..] } else { &b"\x1b\x18"[..] };
            let mut output = expected.to_vec();
            output.extend_from_slice(suffix);
            assert_eq!(guarded(&[&too_long, suffix]), output);
            assert_eq!(guarded(&[&too_long[..MAX_OSC_BYTES], &too_long[MAX_OSC_BYTES..], suffix]), output);
        }
        let mut guard = OscGuard::default();
        assert!(guard.filter(&exact));
        let mut buf = [0; 8];
        assert_eq!(guard.drain(&mut buf), 1); // Only the initial ESC.
        assert!(guard.filter(&[b'b'; 32]));
        assert_eq!(guard.osc.len(), 0);
        assert_eq!(guard.drain(&mut buf), 1); // CAN, not truncated payload.
        assert!(guard.filter(&[b'a'; 32]));
        assert_eq!(guard.drain(&mut buf), 0);
    }
}
