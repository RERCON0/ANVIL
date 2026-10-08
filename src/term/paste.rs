//! Clipboard text -> bytes for the PTY.

/// Removes control characters in both modes, retaining TAB and normalized
/// line breaks. Only ANVIL's own bracket wrappers can contain ESC.
pub fn prepare_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + if bracketed { 12 } else { 0 });
    if bracketed {
        out.extend_from_slice(b"\x1b[200~");
    }
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(b'\r');
            }
            '\n' => out.push(b'\r'),
            '\t' => out.push(b'\t'),
            c if c.is_control() => {}
            c => {
                let mut encoded = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    if bracketed {
        out.extend_from_slice(b"\x1b[201~");
    }
    out
}

/// Even a trailing Enter can execute a command without bracketed paste.
pub fn needs_confirmation(text: &str, bracketed: bool) -> bool {
    !bracketed && text.contains(['\r', '\n'])
}

/// How the pane's shell reads a quoted path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathQuoting {
    /// bash and friends: `'...'`, a `'` inside becomes `'\''`.
    Unix,
    /// `'...'`, quote characters (PowerShell also treats the typographic ones
    /// as quotes) doubled.
    PowerShell,
    /// `"..."`. Windows paths cannot contain `"`, and inside quotes the
    /// interactive cmd prompt takes `^`, `!` and `%%` literally, so nothing is
    /// escaped (Helm's `^^`/`%%` doubling corrupted such paths).
    Cmd,
}

/// A dropped file or folder as text for the shell: control characters
/// removed, quoted for `quoting`, followed by a space so several drops (or
/// the next argument) stay separate — Helm's path drop.
pub fn quote_path(path: &str, quoting: PathQuoting) -> String {
    let path: String = path.chars().filter(|c| !c.is_control()).collect();
    let quoted = match quoting {
        PathQuoting::Unix => format!("'{}'", path.replace('\'', r"'\''")),
        PathQuoting::PowerShell => {
            let mut out = String::with_capacity(path.len() + 2);
            out.push('\'');
            for c in path.chars() {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    out.push(c);
                }
                out.push(c);
            }
            out.push('\'');
            out
        }
        PathQuoting::Cmd => format!("\"{path}\""),
    };
    quoted + " "
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_quoted_for_the_shell() {
        let dir = r"C:\Users\dev\project";
        assert_eq!(quote_path(dir, PathQuoting::PowerShell), r"'C:\Users\dev\project' ");
        assert_eq!(quote_path(dir, PathQuoting::Cmd), r#""C:\Users\dev\project" "#);
        assert_eq!(quote_path(dir, PathQuoting::Unix), r"'C:\Users\dev\project' ");
        assert_eq!(quote_path(r"C:\it's", PathQuoting::Unix), r"'C:\it'\''s' ");
        assert_eq!(quote_path("C:\\it\u{2019}s", PathQuoting::PowerShell), "'C:\\it\u{2019}\u{2019}s' ");
        assert_eq!(quote_path(r"C:\100%^!", PathQuoting::Cmd), r#""C:\100%^!" "#, "cmd takes these literally");
        assert_eq!(
            quote_path("C:\\a\r\nb\x1b[201~", PathQuoting::PowerShell),
            "'C:\\ab[201~' ",
            "no control characters"
        );
    }

    #[test]
    fn newlines_become_carriage_returns() {
        assert_eq!(prepare_paste("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn bracketed_wraps_and_strips_escapes() {
        assert_eq!(prepare_paste("ls\n", true), b"\x1b[200~ls\r\x1b[201~");
        assert_eq!(prepare_paste("x\x1b[201~rm -rf /\n", true), b"\x1b[200~x[201~rm -rf /\r\x1b[201~");
    }

    #[test]
    fn untrusted_controls_never_reach_either_paste_mode() {
        let text = "привет\t\x00\x03\x08\x1b[201~\x7f\u{0085}ok";
        assert_eq!(prepare_paste(text, false), "привет\t[201~ok".as_bytes());
        assert_eq!(prepare_paste(text, true), "\x1b[200~привет\t[201~ok\x1b[201~".as_bytes());
        for text in ["echo dangerous\n", "one\rtwo", "a\r\nb"] {
            assert!(needs_confirmation(text, false));
            assert!(!needs_confirmation(text, true));
        }
        assert!(!needs_confirmation("привет\tworld", false));
    }

    #[test]
    fn unicode_survives() {
        assert_eq!(prepare_paste("привет", true), "\x1b[200~привет\x1b[201~".as_bytes());
    }
}
