//! Watches the raw PTY output for working-directory reports. alacritty_terminal
//! ignores these OSCs, so scanning a copy of the bytes is the only way to see
//! them. The scanner never changes the stream.

use std::path::PathBuf;

const MAX_PAYLOAD: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Osc,
    OscEscape,
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
                    0x1b => self.state = State::OscEscape,
                    0x18 | 0x1a => self.state = State::Ground,
                    _ => {
                        if self.payload.len() < MAX_PAYLOAD {
                            self.payload.push(b);
                        } else {
                            self.overflow = true;
                        }
                    }
                },
                State::OscEscape => {
                    if b == b'\\' {
                        if let Some(dir) = self.finish() {
                            found = Some(dir);
                        }
                    } else {
                        // ESC inside an OSC aborts it and starts a new escape.
                        self.after_escape(b);
                    }
                }
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
    (!path.is_empty()).then(|| PathBuf::from(path))
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
        let out = scan(&[b"prompt\x1b]1337;CurrentDir=C:\\Users\\rerco\\Desktop\x07$ "]);
        assert_eq!(out, Some(PathBuf::from("C:\\Users\\rerco\\Desktop")));
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

    #[test]
    fn escape_inside_osc_restarts() {
        let out = scan(&[b"\x1b]1337;CurrentDir=C:\\A\x1b]1337;CurrentDir=C:\\B\x07"]);
        assert_eq!(out, Some(PathBuf::from("C:\\B")));
    }
}
